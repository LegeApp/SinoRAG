use crate::datafusion_store::DataFusionStore;
use crate::jsonout::write_or_print;
use crate::registry;
use crate::research::{
    base_payload, default_registry_for, evidence_from_row, exact_phrase_rows, field_str, SearchSpec,
};
use anyhow::Result;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub async fn run(
    phrase: String,
    parquet_path: PathBuf,
    _index: PathBuf,
    limit: usize,
    out: Option<PathBuf>,
) -> Result<()> {
    let store = DataFusionStore::open(&parquet_path).await?;
    let normalized = crate::normalize::normalize_zh(&phrase);
    let anchors = anchors(&normalized);
    let mut candidates = BTreeMap::new();
    let per_anchor_limit = limit.max(1);

    for anchor in &anchors {
        let rows = exact_phrase_rows(
            &store,
            &SearchSpec::exact_phrase(anchor.clone(), per_anchor_limit),
        ).await?;
        for row in rows {
            let passage_id = field_str(&row, "passage_id");
            candidates.entry(passage_id).or_insert_with(|| {
                let class = if anchor == &normalized {
                    "exact"
                } else if normalized.contains(anchor) {
                    "partial_anchor"
                } else {
                    "lexical_neighbor"
                };
                json!({
                    "match_class": class,
                    "anchor": anchor,
                    "passage": row
                })
            });
        }
    }

    let results = candidates
        .into_values()
        .take(limit.max(1))
        .collect::<Vec<_>>();
    let evidence = results
        .iter()
        .take(12)
        .filter_map(|candidate| candidate.get("passage"))
        .map(|row| evidence_from_row(row, &phrase, "similar_phrase_candidate"))
        .collect::<Vec<_>>();

    let payload = base_payload(
        "readzen-similar-phrase-v1",
        json!({
            "raw": phrase,
            "normalized": normalized,
            "query_type": "similar_phrase"
        }),
        json!({
            "command": "similar-phrase",
            "policy": "Conservative anchor search; candidates are not accepted reuse claims.",
            "anchors": anchors,
            "limit": limit.max(1)
        }),
        json!({
            "returned_count": results.len(),
            "candidates": results
        }),
        json!(evidence),
        vec![
            "Partial-anchor hits can be boilerplate or unrelated.",
            "Use frontier or manual adjudication before converting candidates into graph claims.",
        ],
        "low",
        vec!["Inspect candidates manually and run passage fetch for context."],
        store.source_fingerprint(),
    );

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

fn anchors(normalized: &str) -> Vec<String> {
    let chars = normalized.chars().collect::<Vec<_>>();
    let mut anchors = Vec::new();
    if !normalized.is_empty() {
        anchors.push(normalized.to_string());
    }
    for len in [10usize, 8, 6, 4] {
        if chars.len() >= len {
            for start in 0..=chars.len() - len {
                let anchor = chars[start..start + len].iter().collect::<String>();
                if !anchors.iter().any(|v| v == &anchor) {
                    anchors.push(anchor);
                }
                if anchors.len() >= 8 {
                    return anchors;
                }
            }
        }
    }
    anchors
}
