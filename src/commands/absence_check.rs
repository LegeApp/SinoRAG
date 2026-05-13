//! `absence-check`: check whether a phrase is absent from a specific
//! catalog scope (work, canon, period). Returns the scope searched and
//! whether the phrase was found, with optional nearby matches.

use crate::catalog_index::CorpusCatalogIndex;
use crate::datafusion_store::DataFusionStore;
use crate::document_table::DocumentTable;
use crate::jsonout::write_or_print;
use crate::research_tools::phrase::phrase_rows_with_explicit_doc_table;
use anyhow::{anyhow, Result};
use serde_json::json;
use std::path::PathBuf;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    parquet: PathBuf,
    phrase_index: Option<PathBuf>,
    doc_table_path: PathBuf,
    catalog_path: PathBuf,
    phrase: String,
    scope_work_id: Option<String>,
    scope_canon: Option<String>,
    scope_period: Option<String>,
    scope_node_id: Option<u32>,
    limit: usize,
    out: Option<PathBuf>,
) -> Result<()> {
    let doc_table = DocumentTable::load(&doc_table_path)?;
    let catalog = CorpusCatalogIndex::load(&catalog_path)?;
    let store = DataFusionStore::open(&parquet).await?;

    // Determine the doc range for the scope.
    let doc_range: Option<(u32, u32)> = if let Some(nid) = scope_node_id {
        let node = catalog
            .get_node(nid)
            .ok_or_else(|| anyhow!("unknown node_id: {nid}"))?;
        node.first_doc_id.zip(node.last_doc_id)
    } else if let Some(wid) = &scope_work_id {
        let work = catalog
            .get_work(wid)
            .ok_or_else(|| anyhow!("unknown work_id: {wid}"))?;
        let root = catalog
            .get_node(work.root_node)
            .ok_or_else(|| anyhow!("work root node missing"))?;
        root.first_doc_id.zip(root.last_doc_id)
    } else {
        None
    };

    let (scoped_hits, phrase_strategy) = phrase_rows_with_explicit_doc_table(
        &store,
        &doc_table,
        phrase_index.as_deref(),
        &phrase,
        limit,
        doc_range,
        scope_canon.as_deref(),
        scope_period.as_deref(),
    )
    .await?;

    let found = !scoped_hits.is_empty();
    let hit_count = scoped_hits.len();

    let scope_desc = json!({
        "work_id": scope_work_id,
        "canon": scope_canon,
        "period": scope_period,
        "node_id": scope_node_id,
        "doc_range": doc_range.map(|(l,h)| json!([l, h])),
    });

    let payload = json!({
        "schema": "sinoragd-absence-check-v1",
        "phrase": phrase,
        "scope": scope_desc,
        "found": found,
        "hit_count": hit_count,
        "sample_hits": scoped_hits.into_iter().take(5).collect::<Vec<_>>(),
        "search_strategy": {
            "phrase": phrase_strategy,
            "limit": limit,
        }
    });
    write_or_print(&payload, out)
}
