use crate::datafusion_store::DataFusionStore;
use crate::document_table::DocumentTable;
use crate::normalize::normalize_zh;
use crate::phrase_index::PhraseIndex;
use crate::research::{exact_phrase_rows, SearchSpec};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

pub async fn phrase_rows_with_explicit_doc_table(
    store: &DataFusionStore,
    doc_table: &DocumentTable,
    phrase_index_path: Option<&Path>,
    phrase: &str,
    limit: usize,
    doc_range: Option<(u32, u32)>,
    canon: Option<&str>,
    period: Option<&str>,
) -> Result<(Vec<Value>, Value)> {
    let normalized = normalize_zh(phrase);
    let limit = limit.max(1);

    if let Some(index_path) = phrase_index_path {
        if index_path.is_file() && !normalized.is_empty() {
            let index = PhraseIndex::load(index_path)?;
            let result = index.candidate_ids_for_normalized_streaming(&normalized);
            let (candidate_doc_ids, candidate_stats) = match result {
                Some(r) => (r.doc_ids, Some(json!(r.stats))),
                None => (Vec::new(), None),
            };

            let scoped_doc_ids: Vec<u32> = candidate_doc_ids
                .into_iter()
                .filter(|did| {
                    if let Some((lo, hi)) = doc_range {
                        *did >= lo && *did <= hi
                    } else {
                        true
                    }
                })
                .collect();

            let passage_ids: Vec<String> = scoped_doc_ids
                .iter()
                .filter_map(|&did| doc_table.passage_id(did).map(String::from))
                .collect();

            let mut rows = store.passages_by_ids(&passage_ids, PASSAGE_SELECT).await?;
            rows.retain(|row| {
                let text = row.get("zh_text_normalized").and_then(|v| v.as_str()).unwrap_or("");
                if !text.contains(&normalized) { return false; }
                if let Some(canon) = canon {
                    if row.get("canon").and_then(|v| v.as_str()).unwrap_or("") != canon { return false; }
                }
                if let Some(period) = period {
                    if row.get("period").and_then(|v| v.as_str()).unwrap_or("") != period { return false; }
                }
                true
            });
            rows.sort_by_key(|row| {
                let pid = row.get("passage_id").and_then(|v| v.as_str()).unwrap_or("");
                doc_table.doc_id(pid).unwrap_or(u32::MAX)
            });
            rows.truncate(limit);

            return Ok((rows, json!({
                "used_phrase_index": true,
                "candidate_stats": candidate_stats,
                "after_doc_scope": scoped_doc_ids.len(),
                "limit": limit,
            })));
        }
    }

    if let Some((lo, hi)) = doc_range {
        let passage_ids: Vec<String> = (lo..=hi)
            .filter_map(|did| doc_table.passage_id(did).map(String::from))
            .collect();
        let mut rows = store.passages_by_ids(&passage_ids, PASSAGE_SELECT).await?;
        rows.retain(|row| {
            let text = row.get("zh_text_normalized").and_then(|v| v.as_str()).unwrap_or("");
            if !text.contains(&normalized) { return false; }
            if let Some(canon) = canon {
                if row.get("canon").and_then(|v| v.as_str()).unwrap_or("") != canon { return false; }
            }
            if let Some(period) = period {
                if row.get("period").and_then(|v| v.as_str()).unwrap_or("") != period { return false; }
            }
            true
        });
        rows.sort_by_key(|row| {
            let pid = row.get("passage_id").and_then(|v| v.as_str()).unwrap_or("");
            doc_table.doc_id(pid).unwrap_or(u32::MAX)
        });
        rows.truncate(limit);
        return Ok((rows, json!({
            "used_phrase_index": false,
            "scope_scan": "doc_range",
            "limit": limit,
        })));
    }

    let mut spec = SearchSpec::exact_phrase(phrase.to_string(), limit);
    if let Some(canon) = canon {
        spec.canon = vec![canon.to_string()];
    }
    let mut rows = exact_phrase_rows(store, &spec).await?;
    if let Some(period) = period {
        rows.retain(|row| row.get("period").and_then(|v| v.as_str()).unwrap_or("") == period);
        rows.truncate(limit);
    }
    Ok((rows, json!({
        "used_phrase_index": false,
        "scope_scan": "parquet_global",
        "limit": limit,
    })))
}

const PASSAGE_SELECT: &str = "passage_id, source_rel_path, xml_id, div_path, heading, heading_path, \
from_lb, to_lb, zh_text_raw, zh_text_normalized, text_type, contains_person, \
contains_term, contains_foreign, canon, canon_name, traditions, period, origin, \
author, main_title, period_rank, source_corpus, source_work_id, source_section_id, \
source_locator, source_url, edition_siglum, edition_label, rights_id, rights_notes, \
retrieval_method, snapshot_id, quality_flags_json";
