//! `outline-search`: search for a phrase within a catalog outline node
//! (work, division, etc.) and return hits grouped by child outline nodes.

use crate::catalog_index::CorpusCatalogIndex;
use crate::datafusion_store::DataFusionStore;
use crate::document_table::DocumentTable;
use crate::jsonout::write_or_print;
use crate::research_tools::phrase::phrase_rows_with_explicit_doc_table;
use crate::research_tools::scopes::{group_hits_by_outline_node, OutlineSearchLevel};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    parquet: PathBuf,
    phrase_index: Option<PathBuf>,
    doc_table_path: PathBuf,
    catalog_path: PathBuf,
    phrase: String,
    node_id: Option<u32>,
    work_id: Option<String>,
    group_by: String,
    limit_total: usize,
    limit_per_group: usize,
    out: Option<PathBuf>,
) -> Result<()> {
    let doc_table = DocumentTable::load(&doc_table_path)?;
    let catalog = CorpusCatalogIndex::load(&catalog_path)?;
    let store = DataFusionStore::open(&parquet).await?;

    // Resolve starting node: explicit node_id or work_id → root_node.
    let start_node = if let Some(nid) = node_id {
        nid
    } else if let Some(wid) = &work_id {
        let work = catalog.get_work(wid)
            .ok_or_else(|| anyhow!("unknown work_id: {wid}"))?;
        work.root_node
    } else {
        return Err(anyhow!("either --node-id or --work-id is required"));
    };

    let target = match group_by.as_str() {
        "division" => OutlineSearchLevel::Division,
        "work" => OutlineSearchLevel::Work,
        "passage" => OutlineSearchLevel::PassageRange,
        other => return Err(anyhow!("unknown --group-by `{other}`; expected division|work|passage")),
    };

    let start_node_obj = catalog.get_node(start_node)
        .ok_or_else(|| anyhow!("unknown node_id: {start_node}"))?;
    let (lo, hi) = match (start_node_obj.first_doc_id, start_node_obj.last_doc_id) {
        (Some(l), Some(h)) => (l, h),
        _ => return Err(anyhow!("node {start_node} has no doc range")),
    };

    let (hits, phrase_strategy) = phrase_rows_with_explicit_doc_table(
        &store,
        &doc_table,
        phrase_index.as_deref(),
        &phrase,
        limit_total,
        Some((lo, hi)),
        None,
        None,
    ).await?;

    let filtered_doc_ids: Vec<u32> = hits.iter()
        .filter_map(|row| {
            let pid = row.get("passage_id").and_then(|v| v.as_str())?;
            doc_table.doc_id(pid)
        })
        .collect();

    let total_hits = filtered_doc_ids.len();

    // Group by the target outline level.
    let group_counts = group_hits_by_outline_node(&catalog, &filtered_doc_ids, target);

    let mut sorted_groups: Vec<(u32, u32)> = group_counts.into_iter().collect();
    sorted_groups.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let mut groups: Vec<Value> = Vec::new();
    for (node_id, count) in sorted_groups.iter().take(limit_per_group) {
        let node = catalog.get_node(*node_id);
        groups.push(json!({
            "node_id": node_id,
            "label": node.map(|n| n.label.as_str()).unwrap_or(""),
            "heading_path": node.map(|n| n.heading_path.as_str()).unwrap_or(""),
            "node_kind": node.map(|n| format!("{:?}", &n.node_kind)).unwrap_or_default(),
            "hit_count": count,
        }));
    }

    let payload = json!({
        "schema": "sinoragd-outline-search-v1",
        "phrase": phrase,
        "start_node_id": start_node,
        "start_label": start_node_obj.label,
        "group_by": group_by,
        "total_hits": total_hits,
        "group_count": sorted_groups.len(),
        "groups": groups,
        "search_strategy": {
            "phrase": phrase_strategy,
            "limit_total": limit_total,
            "limit_per_group": limit_per_group,
        }
    });
    write_or_print(&payload, out)
}
