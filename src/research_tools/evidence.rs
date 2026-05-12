use crate::datafusion_store::DataFusionStore;
use crate::document_table::DocumentTable;
use crate::research::quote_for_phrase;
use crate::research_tools::common::EvidencePassage;
use anyhow::Result;

/// Load evidence passages for a set of doc_ids from parquet.
/// Preserves the input doc_id order.
pub async fn load_evidence_passages(
    store: &DataFusionStore,
    doc_table: &DocumentTable,
    doc_ids: &[u32],
    phrase: &str,
    limit_chars: usize,
) -> Result<Vec<EvidencePassage>> {
    if doc_ids.is_empty() {
        return Ok(Vec::new());
    }

    let passage_ids: Vec<String> = doc_ids
        .iter()
        .filter_map(|&did| doc_table.passage_id(did).map(String::from))
        .collect();

    if passage_ids.is_empty() {
        return Ok(Vec::new());
    }

    let rows = store
        .passages_by_ids(
            &passage_ids,
            "passage_id, source_work_id, main_title, author, period, period_rank, \
             canon, from_lb, to_lb, zh_text_raw",
        )
        .await?;

    // Build a map from passage_id → row for order-preserving lookup.
    let mut row_map: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::with_capacity(rows.len());
    for row in rows {
        let pid = crate::research::field_str(&row, "passage_id");
        row_map.insert(pid, row);
    }

    let mut out = Vec::with_capacity(doc_ids.len());
    for &did in doc_ids {
        if let Some(pid) = doc_table.passage_id(did) {
            if let Some(row) = row_map.remove(pid) {
                let raw = row
                    .get("zh_text_raw")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let quote = if limit_chars > 0 {
                    let q = quote_for_phrase(raw, phrase);
                    q.chars().take(limit_chars).collect()
                } else {
                    quote_for_phrase(raw, phrase)
                };
                out.push(crate::research_tools::common::evidence_from_row(
                    &row, did, quote, None,
                ));
            }
        }
    }
    Ok(out)
}
