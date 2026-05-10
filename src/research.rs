use crate::datafusion_store::{DataFusionStore, sql_literal, string_contains_sql};
use crate::document_table::DocumentTable;
use crate::normalize::normalize_zh;
use crate::phrase_index::{ids_to_sql_list, PhraseIndex};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

pub const CREATED_BY: &str = "graphdiscovery-rust";

#[derive(Debug, Clone)]
pub struct SearchSpec {
    pub normalized: String,
    pub canon: Vec<String>,
    pub limit: usize,
}

impl SearchSpec {
    pub fn exact_phrase(phrase: String, limit: usize) -> Self {
        Self {
            normalized: normalize_zh(&phrase),
            canon: Vec::new(),
            limit,
        }
    }
}

pub async fn exact_phrase_rows(
    store: &DataFusionStore,
    spec: &SearchSpec,
) -> Result<Vec<Value>> {
    exact_phrase_rows_with_index(store, spec, None).await
}

pub async fn exact_phrase_rows_with_index(
    store: &DataFusionStore,
    spec: &SearchSpec,
    phrase_index_path: Option<&Path>,
) -> Result<Vec<Value>> {
    if let Some(index_path) = phrase_index_path {
        if index_path.is_file() && !spec.normalized.is_empty() {
            let index = PhraseIndex::load(index_path)?;
            let doc_ids = index.candidate_ids_for_normalized(&spec.normalized, usize::MAX);

            let doc_table_path = index_path
                .parent()
                .map(|p| p.join("doc_table.bin"))
                .unwrap_or_else(|| PathBuf::from("doc_table.bin"));

            if doc_table_path.exists() {
                let doc_table = DocumentTable::load(&doc_table_path)?;
                let passage_ids: Vec<String> = doc_ids
                    .iter()
                    .filter_map(|&doc_id| doc_table.passage_ids.get(doc_id as usize).cloned())
                    .collect();
                return exact_phrase_rows_for_ids(store, spec, &passage_ids).await;
            } else {
                anyhow::bail!(
                    "doc_table.bin not found next to phrase index at {} \u{2014} run doc-table-build first",
                    doc_table_path.display()
                );
            }
        }
    }

    let mut where_parts = Vec::new();
    if !spec.normalized.is_empty() {
        where_parts.push(string_contains_sql("zh_text_normalized", &spec.normalized));
    }
    if !spec.canon.is_empty() {
        let quoted = spec
            .canon
            .iter()
            .map(|c| sql_literal(c))
            .collect::<Vec<_>>()
            .join(", ");
        where_parts.push(format!("canon IN ({quoted})"));
    }

    let where_sql = if where_parts.is_empty() {
        "true".to_string()
    } else {
        where_parts.join(" AND ")
    };
    let limit = spec.limit.max(1);
    let sql = format!(
        r#"
        SELECT passage_id, source_rel_path, xml_id, div_path, heading, heading_path,
               from_lb, to_lb, zh_text_raw, zh_text_normalized, text_type,
               contains_person, contains_term, contains_foreign,
               canon, canon_name, traditions, period, origin, author, main_title,
               period_rank, source_corpus, source_work_id, source_section_id, source_locator,
               source_url, edition_siglum, edition_label, rights_id, rights_notes,
               retrieval_method, snapshot_id, quality_flags_json
        FROM passages
        WHERE {where_sql}
        ORDER BY period_rank ASC, source_rel_path ASC, from_lb ASC, xml_id ASC
        LIMIT {limit}
        "#
    );
    store.query_json(&sql).await
}

async fn exact_phrase_rows_for_ids(
    store: &DataFusionStore,
    spec: &SearchSpec,
    ids: &[String],
) -> Result<Vec<Value>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    for chunk in ids.chunks(4000) {
        let id_list = ids_to_sql_list(chunk);
        let mut where_parts = vec![
            format!("passage_id IN ({id_list})"),
            string_contains_sql("zh_text_normalized", &spec.normalized),
        ];
        if !spec.canon.is_empty() {
            let quoted = spec
                .canon
                .iter()
                .map(|c| sql_literal(c))
                .collect::<Vec<_>>()
                .join(", ");
            where_parts.push(format!("canon IN ({quoted})"));
        }
        rows.extend(store.query_json(
            &format!(
                r#"
                SELECT passage_id, source_rel_path, xml_id, div_path, heading, heading_path,
                       from_lb, to_lb, zh_text_raw, zh_text_normalized, text_type,
                       contains_person, contains_term, contains_foreign,
                       canon, canon_name, traditions, period, origin, author, main_title,
                       period_rank, source_corpus, source_work_id, source_section_id, source_locator,
                       source_url, edition_siglum, edition_label, rights_id, rights_notes,
                       retrieval_method, snapshot_id, quality_flags_json
                FROM passages
                WHERE {}
                "#,
                where_parts.join(" AND ")
            ),
        ).await?);
    }
    rows.sort_by_key(row_sort_key);
    rows.truncate(spec.limit.max(1));
    Ok(rows)
}

pub fn evidence_from_row(row: &Value, phrase: &str, role: &str) -> Value {
    let obj = row.as_object();
    let raw = obj
        .and_then(|o| o.get("zh_text_raw"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let from_lb = obj
        .and_then(|o| o.get("from_lb"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let to_lb = obj
        .and_then(|o| o.get("to_lb"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let citation = format_citation(row, from_lb, to_lb);
    json!({
        "passage_id": field_str(row, "passage_id"),
        "source_rel_path": field_str(row, "source_rel_path"),
        "xml_id": field_str(row, "xml_id"),
        "lb_range": lb_range(from_lb, to_lb),
        "zh_quote": quote_for_phrase(raw, phrase),
        "evidence_role": role,
        "canon": field_str(row, "canon"),
        "canon_name": field_str(row, "canon_name"),
        "period": field_str(row, "period"),
        "author": field_str(row, "author"),
        "main_title": field_str(row, "main_title"),
        "source_corpus": field_str(row, "source_corpus"),
        "source_work_id": field_str(row, "source_work_id"),
        "source_section_id": field_str(row, "source_section_id"),
        "source_locator": field_str(row, "source_locator"),
        "source_url": field_str(row, "source_url"),
        "edition_siglum": field_str(row, "edition_siglum"),
        "edition_label": field_str(row, "edition_label"),
        "rights_id": field_str(row, "rights_id"),
        "retrieval_method": field_str(row, "retrieval_method"),
        "snapshot_id": field_str(row, "snapshot_id"),
        "citation": citation
    })
}

fn row_sort_key(row: &Value) -> (i64, String, String, String) {
    (
        row.get("period_rank")
            .and_then(|v| v.as_i64())
            .unwrap_or(99),
        field_str(row, "source_rel_path"),
        field_str(row, "from_lb"),
        field_str(row, "xml_id"),
    )
}

pub fn distribution(rows: &[Value], field: &str) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for row in rows {
        if let Some(value) = row.get(field).and_then(|v| v.as_str()) {
            if !value.is_empty() {
                *out.entry(value.to_string()).or_insert(0) += 1;
            }
        }
    }
    out
}

pub fn tradition_distribution(rows: &[Value]) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for row in rows {
        if let Some(values) = row.get("traditions").and_then(|v| v.as_array()) {
            for value in values {
                if let Some(value) = value.as_str() {
                    if !value.is_empty() {
                        *out.entry(value.to_string()).or_insert(0) += 1;
                    }
                }
            }
        }
    }
    out
}

pub fn base_payload(
    schema: &str,
    query: Value,
    method: Value,
    results: Value,
    evidence: Value,
    caveats: Vec<&str>,
    confidence: &str,
    next_steps: Vec<&str>,
    source_fingerprint: Value,
) -> Value {
    json!({
        "schema": schema,
        "query": query,
        "scope": {
            "corpora": ["cbeta"],
            "note": "Earliest means earliest in the loaded corpus, not absolute historical origin.",
            "cross_corpus_status": "single-corpus unless additional adapters were used"
        },
        "method": method,
        "results": results,
        "evidence": evidence,
        "caveats": caveats,
        "confidence": confidence,
        "next_steps": next_steps,
        "created_by": CREATED_BY,
        "source_fingerprint": source_fingerprint
    })
}

pub fn field_str(row: &Value, field: &str) -> String {
    row.get(field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

pub fn format_citation(row: &Value, from_lb: &str, to_lb: &str) -> String {
    let main_title = field_str(row, "main_title");
    let author = field_str(row, "author");
    let canon_name = field_str(row, "canon_name");
    let source_locator = field_str(row, "source_locator");
    let passage_id = field_str(row, "passage_id");
    let source_url = field_str(row, "source_url");
    
    let locator = if !source_locator.is_empty() {
        source_locator
    } else if !from_lb.is_empty() {
        lb_range(from_lb, to_lb)
    } else {
        String::new()
    };
    
    let mut parts = Vec::new();
    
    if !author.is_empty() {
        parts.push(author.clone());
    }
    if !main_title.is_empty() {
        parts.push(main_title.clone());
    }
    if !canon_name.is_empty() {
        parts.push(format!("[{}]", canon_name));
    }
    
    let citation = parts.join(", ");
    
    if !locator.is_empty() {
        format!("{} ({})", citation, locator)
    } else {
        citation
    }
}

pub fn lb_range(from_lb: &str, to_lb: &str) -> String {
    match (from_lb.is_empty(), to_lb.is_empty(), from_lb == to_lb) {
        (true, _, _) => String::new(),
        (_, true, _) | (_, _, true) => from_lb.to_string(),
        _ => format!("{from_lb}-{to_lb}"),
    }
}

pub fn quote_for_phrase(raw: &str, phrase: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    if phrase.is_empty() {
        return raw.chars().take(120).collect();
    }
    if let Some(byte_idx) = raw.find(phrase) {
        let start_char = raw[..byte_idx].chars().count().saturating_sub(24);
        let phrase_len = phrase.chars().count();
        let end_char = start_char + 48 + phrase_len;
        return raw
            .chars()
            .skip(start_char)
            .take(end_char.saturating_sub(start_char))
            .collect();
    }
    raw.chars().take(120).collect()
}

pub fn default_registry_for(parquet_path: &Path) -> PathBuf {
    parquet_path
        .parent()
        .map(|p| p.join("completions.duckdb"))
        .unwrap_or_else(|| PathBuf::from("GraphDiscovery/Runs/rust/completions.duckdb"))
}
