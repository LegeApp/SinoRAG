use crate::datafusion_store::{sql_literal, string_contains_sql, DataFusionStore};
use crate::document_table::DocumentTable;
use crate::jsonout;
use crate::registry;
use crate::tfidf::ngram::char_ngrams;
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const SEED_PHRASE_CAP: usize = 40;

pub async fn run(
    seed: String,
    parquet_path: PathBuf,
    index_path: PathBuf,
    limit: usize,
    phrase_limit: usize,
    out: Option<PathBuf>,
    registry_path: PathBuf,
) -> Result<()> {
    let store = DataFusionStore::open(&parquet_path).await?;
    
    // Load DocumentTable for doc_id resolution
    let doc_table_path = parquet_path.join("doc_table.bin");
    let doc_table = if doc_table_path.exists() {
        DocumentTable::load(&doc_table_path)?
    } else {
        anyhow::bail!("DocumentTable not found at {}. Run doc-table-build first.", doc_table_path.display());
    };
    
    let seed_row = store.get_passage(&seed).await?;
    let similar = crate::commands::tfidf::similar_passages(
        &store,
        index_path.clone(),
        &seed,
        limit,
        12,
        8,
        4,
        &doc_table,
    ).await?;
    let phrase_frontiers = phrase_frontiers(&store, &seed_row, phrase_limit).await?;
    let prior_work = if registry_path.exists() {
        registry::prior_work(&registry_path, &seed, 10)?
    } else {
        Vec::new()
    };
    let payload = json!({
        "schema": "readzen-graphdiscovery-frontier-v1",
        "seed_passage_id": seed,
        "seed": seed_row,
        "inputs": {
            "parquet": parquet_path.display().to_string(),
            "index": index_path.display().to_string(),
            "similar_limit": limit.max(1),
            "phrase_limit": phrase_limit.max(1),
        },
        "similar_passages": similar,
        "phrase_frontiers": phrase_frontiers,
        "facet_summary": facet_summary(&similar),
        "next_seed_candidates": next_seed_candidates(&similar),
        "prior_work": prior_work,
        "agent_guidance": [
            "Use exact Chinese evidence from seed, similar_passages, or phrase_frontiers only.",
            "Prefer frontier phrases with recurring hits across more than one source path or metadata facet.",
            "Use similar_passages as candidate graph neighbors, not as accepted edges without review.",
            "Downrank famous or same-file hubs unless they clarify the local graph structure."
        ],
    });
    let out_ref = out.as_deref();
    let seed_passage_id = payload
        .get("seed_passage_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    registry::record_payload(
        &registry_path,
        "frontier_packet",
        &payload,
        out_ref,
        seed_passage_id,
        "",
    )?;
    jsonout::write_or_print(&payload, out)
}

pub async fn phrase_frontiers(store: &DataFusionStore, seed: &Value, limit: usize) -> Result<Vec<Value>> {
    let candidates = seed_phrases(value_str(seed, "zh_text_normalized"));
    let mut frontiers = Vec::new();
    for phrase in candidates {
        let where_clause = string_contains_sql("zh_text_normalized", &phrase);
        let rows = store.query_json(
            &format!(
                r#"
                SELECT passage_id, source_rel_path, xml_id, heading, from_lb, to_lb,
                       zh_text_raw, canon, traditions, period, origin, author, main_title
                FROM passages
                WHERE {where_clause}
                ORDER BY period_rank, source_rel_path, from_lb, xml_id
                LIMIT 8
                "#
            ),
        ).await?;
        let count_rows = store.query_json(
            &format!(
                r#"
                SELECT count(*) AS count
                FROM passages
                WHERE {where_clause}
                "#
            ),
        ).await?;
        let total = count_rows
            .first()
            .and_then(|row| row.get("count"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let _ = sql_literal;
        if total <= 1 || total > 200 {
            continue;
        }
        let sources: BTreeSet<String> = rows
            .iter()
            .map(|row| value_str(row, "source_rel_path"))
            .filter(|value| !value.is_empty())
            .collect();
        let periods: BTreeSet<String> = rows
            .iter()
            .map(|row| value_str(row, "period"))
            .filter(|value| !value.is_empty())
            .collect();
        let length = phrase.chars().count();
        let graph_value = 1.0f64.min(
            (length as f64 / 8.0) + (total.min(20) as f64 / 40.0) + (sources.len() as f64 / 20.0),
        );
        frontiers.push(json!({
            "phrase": phrase,
            "length": length,
            "total_hits": total,
            "sample_count": rows.len(),
            "source_path_count_in_sample": sources.len(),
            "period_count_in_sample": periods.len(),
            "graph_value_score": round_f64(graph_value, 6),
            "sample_passages": rows,
        }));
    }
    frontiers.sort_by(|a, b| frontier_key(b).cmp(&frontier_key(a)));
    frontiers.truncate(limit.max(1));
    Ok(frontiers)
}

fn seed_phrases(normalized: String) -> Vec<String> {
    let mut phrases: BTreeSet<String> = BTreeSet::new();
    for gram in char_ngrams(&normalized, 4, 10) {
        if !looks_low_value_phrase(&gram) {
            phrases.insert(gram);
        }
    }
    let mut phrases: Vec<String> = phrases.into_iter().collect();
    phrases.sort_by(|a, b| {
        b.chars()
            .count()
            .cmp(&a.chars().count())
            .then_with(|| b.cmp(a))
    });
    phrases.truncate(SEED_PHRASE_CAP);
    phrases
}

fn looks_low_value_phrase(phrase: &str) -> bool {
    if phrase.is_empty() {
        return true;
    }
    let chars: Vec<char> = phrase.chars().collect();
    let unique: BTreeSet<char> = chars.iter().copied().collect();
    if unique.len() <= 1 {
        return true;
    }
    if chars.len() >= 4 && chars.len() % 2 == 0 {
        let half = chars.len() / 2;
        if chars[..half] == chars[half..] {
            return true;
        }
    }
    false
}

pub fn facet_summary(rows: &[Value]) -> Value {
    let mut periods = BTreeMap::new();
    let mut canons = BTreeMap::new();
    let mut origins = BTreeMap::new();
    let mut traditions = BTreeMap::new();
    for row in rows {
        bump(&mut periods, value_str(row, "period"));
        bump(&mut canons, value_str(row, "canon"));
        bump(&mut origins, value_str(row, "origin"));
        if let Some(items) = row.get("traditions").and_then(Value::as_array) {
            for item in items {
                bump(
                    &mut traditions,
                    item.as_str().unwrap_or_default().to_string(),
                );
            }
        }
    }
    json!({
        "periods": periods,
        "canons": canons,
        "origins": origins,
        "traditions": traditions,
    })
}

pub fn next_seed_candidates(rows: &[Value]) -> Vec<Value> {
    let mut candidates = rows.to_vec();
    candidates.sort_by(|a, b| {
        value_f64(b, "tfidf_cosine")
            .partial_cmp(&value_f64(a, "tfidf_cosine"))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates
        .into_iter()
        .take(5)
        .map(|row| {
            json!({
                "passage_id": value_str(&row, "passage_id"),
                "tfidf_cosine": row.get("tfidf_cosine").cloned().unwrap_or_else(|| json!(0)),
                "heading": value_str(&row, "heading"),
                "source_rel_path": value_str(&row, "source_rel_path"),
                "reason": "high lexical similarity candidate for follow-up review",
            })
        })
        .collect()
}

fn bump(map: &mut BTreeMap<String, usize>, value: String) {
    if !value.is_empty() {
        *map.entry(value).or_insert(0) += 1;
    }
}

fn frontier_key(value: &Value) -> (i64, usize, usize, i64) {
    let graph = (value_f64(value, "graph_value_score") * 1_000_000.0).round() as i64;
    let sources = value
        .get("source_path_count_in_sample")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let length = value.get("length").and_then(Value::as_u64).unwrap_or(0) as usize;
    let total = value.get("total_hits").and_then(Value::as_i64).unwrap_or(0);
    (graph, sources, length, -total)
}

fn value_str(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn value_f64(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn round_f64(value: f64, places: i32) -> f64 {
    let factor = 10f64.powi(places);
    (value * factor).round() / factor
}
