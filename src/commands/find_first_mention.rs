//! `find-first-mention`: earliest attestation of a phrase, ordered by
//! `period_rank` from doc_table, with optional scope filters.

use crate::datafusion_store::DataFusionStore;
use crate::document_table::DocumentTable;
use crate::jsonout::write_or_print;
use crate::research::{exact_phrase_rows_with_index, SearchSpec};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::PathBuf;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    parquet: PathBuf,
    phrase_index: Option<PathBuf>,
    doc_table_path: PathBuf,
    phrase: String,
    scope_canon: Vec<String>,
    scope_period: Vec<String>,
    scope_source_work_id: Option<String>,
    limit: usize,
    out: Option<PathBuf>,
) -> Result<()> {
    let doc_table = DocumentTable::load(&doc_table_path)?;
    let store = DataFusionStore::open(&parquet).await?;

    let mut spec = SearchSpec::exact_phrase(phrase.clone(), limit.max(200));
    spec.canon = scope_canon.clone();

    let candidate_estimate = phrase_index.is_some();
    let raw_hits = exact_phrase_rows_with_index(&store, &spec, phrase_index.as_deref()).await?;
    // Apply scope_period and scope_source_work_id post-hoc since SearchSpec
    // doesn't carry those fields.
    let hits: Vec<Value> = raw_hits
        .into_iter()
        .filter(|row| {
            if !scope_period.is_empty() {
                let p = row.get("period").and_then(|v| v.as_str()).unwrap_or("");
                if !scope_period.iter().any(|s| s == p) {
                    return false;
                }
            }
            if let Some(work) = &scope_source_work_id {
                let w = row
                    .get("source_work_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if w != work {
                    return false;
                }
            }
            true
        })
        .collect();
    let verified = hits.len();

    // Sort by (period_rank, doc_id) — both come from the row payload + doc_table.
    let mut scored: Vec<(i32, u32, Value)> = Vec::with_capacity(hits.len());
    for row in hits {
        let pid = row.get("passage_id").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(did) = doc_table.doc_id(pid) {
            let pr = doc_table
                .period_ranks
                .get(did as usize)
                .copied()
                .unwrap_or(0);
            scored.push((pr, did, row));
        }
    }
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    let total = scored.len();
    let take = limit.min(total);
    let mut iter = scored.into_iter().take(take);
    let first = iter.next().map(|(pr, did, mut row)| {
        if let Some(obj) = row.as_object_mut() {
            obj.insert("period_rank".to_string(), json!(pr));
            obj.insert("doc_id".to_string(), json!(did));
        }
        row
    });
    let next_earlier: Vec<Value> = iter
        .map(|(pr, did, mut row)| {
            if let Some(obj) = row.as_object_mut() {
                obj.insert("period_rank".to_string(), json!(pr));
                obj.insert("doc_id".to_string(), json!(did));
            }
            row
        })
        .collect();

    let payload = json!({
        "schema": "sinoragd-first-mention-v1",
        "phrase": phrase,
        "first": first,
        "next_earlier": next_earlier,
        "scope": {
            "canon": scope_canon,
            "period": scope_period,
            "source_work_id": scope_source_work_id,
        },
        "search_strategy": {
            "used_phrase_index": candidate_estimate,
            "candidates_verified": verified,
            "after_scope_and_sort": total,
            "limit": limit,
        }
    });
    write_or_print(&payload, out)
}
