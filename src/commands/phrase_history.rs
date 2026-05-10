use crate::datafusion_store::DataFusionStore;
use crate::jsonout::write_or_print;
use crate::registry;
use crate::research::{
    base_payload, default_registry_for, distribution, evidence_from_row,
    exact_phrase_rows_with_index, tradition_distribution, SearchSpec,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub async fn phrase_history(
    phrase: String,
    store: &DataFusionStore,
    include_variants: bool,
    timeline: bool,
    phrase_index: Option<PathBuf>,
) -> Result<serde_json::Value> {
    let spec = SearchSpec::exact_phrase(phrase.clone(), 200);
    let rows = exact_phrase_rows_with_index(store, &spec, phrase_index.as_deref()).await?;
    let earliest = rows.first().cloned();
    let evidence = rows
        .iter()
        .take(12)
        .map(|row| evidence_from_row(row, &phrase, "exact_phrase_hit"))
        .collect::<Vec<_>>();
    let timeline_buckets = if timeline {
        timeline_from_rows(&rows)
    } else {
        BTreeMap::new()
    };

    let variant_phrases = if include_variants {
        conservative_variants(&spec.normalized)
            .into_iter()
            .map(|variant| json!({"phrase": variant, "status": "suggested_search_anchor"}))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let hits_by_period = distribution(&rows, "period");
    let hits_by_canon = distribution(&rows, "canon");
    let hits_by_tradition = tradition_distribution(&rows);

    let source_fingerprint = store.source_fingerprint();
    let mut payload = base_payload(
        "readzen-phrase-history-v2",
        json!({
            "raw": phrase,
            "normalized": spec.normalized,
            "query_type": "exact_phrase",
            "include_variants": include_variants,
            "timeline": timeline
        }),
        json!({
            "command": "phrase-history",
            "match_type": "normalized exact phrase containment",
            "search_backend": if phrase_index.is_some() { "phrase_index_verified_by_datafusion" } else { "datafusion_strpos" },
            "variant_policy": "v1 suggests conservative anchors only; suggested variants are not accepted evidence",
            "limit": 200
        }),
        json!({
            "earliest_loaded_attestation": earliest,
            "first_chan_attestation": first_chan_attestation(&rows),
            "returned_count": rows.len(),
            "hits_by_period": hits_by_period,
            "hits_by_canon": hits_by_canon,
            "hits_by_tradition": hits_by_tradition,
            "variant_phrases": if include_variants { json!(variant_phrases) } else { Value::Null },
            "candidate_lineage": Vec::<Value>::new(),
            "timeline_buckets": if timeline { json!(timeline_buckets) } else { Value::Null }
        }),
        json!(evidence),
        vec![
            "Earliest means earliest in loaded corpus, not absolute origin.",
            "Suggested variants are search anchors, not evidence.",
            "Chinese classical literary allusion checks are incomplete until non-Buddhist corpora are loaded.",
        ],
        if rows.is_empty() { "none" } else { "medium" },
        vec![
            "Inspect earliest evidence manually before making an origin claim.",
            "Run canonical-source if the phrase may be sutra or sastra wording.",
        ],
        source_fingerprint,
    );

    if let Some(obj) = payload.as_object_mut() {
        let legacy = obj
            .get("results")
            .and_then(|v| v.as_object())
            .map(|results| {
                [
                    "earliest_loaded_attestation",
                    "first_chan_attestation",
                    "hits_by_period",
                    "hits_by_canon",
                    "hits_by_tradition",
                    "variant_phrases",
                    "candidate_lineage",
                    "timeline_buckets",
                ]
                .into_iter()
                .map(|key| (key, results.get(key).cloned().unwrap_or(Value::Null)))
                .collect::<Vec<_>>()
            });
        if let Some(legacy) = legacy {
            for (key, value) in legacy {
                obj.insert(key.to_string(), value);
            }
        }
    }
    
    Ok(payload)
}

pub async fn run(
    phrase: String,
    parquet_path: PathBuf,
    include_variants: bool,
    timeline: bool,
    phrase_index: Option<PathBuf>,
    out: Option<PathBuf>,
) -> Result<()> {
    let store = DataFusionStore::open(&parquet_path).await?;
    let payload = phrase_history(phrase, &store, include_variants, timeline, phrase_index).await?;

    let registry_path = default_registry_for(&parquet_path);
    let _ = registry::record_payload(
        &registry_path,
        "semantic_research",
        &payload,
        out.as_deref(),
        "",
        payload
            .get("query")
            .and_then(|q| q.get("raw"))
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );

    write_or_print(&payload, out)
}

fn first_chan_attestation(rows: &[Value]) -> Value {
    rows.iter()
        .find(|row| {
            row.get("traditions")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().any(|v| v.as_str() == Some("Chan/Zen")))
                .unwrap_or(false)
        })
        .cloned()
        .unwrap_or(Value::Null)
}

fn timeline_from_rows(rows: &[Value]) -> BTreeMap<String, Value> {
    let mut counts: BTreeMap<String, (usize, Option<Value>)> = BTreeMap::new();
    for row in rows {
        let period = row
            .get("period")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("Unknown");
        let entry = counts.entry(period.to_string()).or_insert((0, None));
        entry.0 += 1;
        if entry.1.is_none() {
            entry.1 = Some(row.clone());
        }
    }
    counts
        .into_iter()
        .map(|(period, (count, representative))| {
            (
                period,
                json!({
                    "count": count,
                    "representative": representative
                }),
            )
        })
        .collect()
}

fn conservative_variants(normalized: &str) -> Vec<String> {
    let chars = normalized.chars().collect::<Vec<_>>();
    let mut variants = Vec::new();
    for len in [4usize, 6, 8, 10] {
        if chars.len() >= len {
            let value = chars.iter().take(len).collect::<String>();
            if !variants.iter().any(|v| v == &value) {
                variants.push(value);
            }
        }
    }
    variants
}
