//! `cluster-hits`: cluster phrase search hits by catalog outline
//! (work/division), returning hit counts per cluster with top
//! representative passages.

use crate::catalog_index::CorpusCatalogIndex;
use crate::datafusion_store::DataFusionStore;
use crate::document_table::DocumentTable;
use crate::jsonout::write_or_print;
use crate::research_tools::phrase::phrase_rows_with_explicit_doc_table;
use crate::research_tools::scopes::group_hits_by_outline_node;
use crate::research_tools::scopes::OutlineSearchLevel;
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
    cluster_by: String,
    limit_total: usize,
    limit_per_cluster: usize,
    out: Option<PathBuf>,
) -> Result<()> {
    let doc_table = DocumentTable::load(&doc_table_path)?;
    let catalog = CorpusCatalogIndex::load(&catalog_path)?;
    let store = DataFusionStore::open(&parquet).await?;

    let target = match cluster_by.as_str() {
        "work" => OutlineSearchLevel::Work,
        "division" => OutlineSearchLevel::Division,
        other => return Err(anyhow!("unknown --cluster-by `{other}`; expected work|division")),
    };

    let (hits, phrase_strategy) = phrase_rows_with_explicit_doc_table(
        &store,
        &doc_table,
        phrase_index.as_deref(),
        &phrase,
        limit_total,
        None,
        None,
        None,
    ).await?;

    // Collect (doc_id, row) pairs.
    let mut doc_rows: Vec<(u32, Value)> = Vec::with_capacity(hits.len());
    for row in hits {
        let pid = row.get("passage_id").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(did) = doc_table.doc_id(pid) {
            doc_rows.push((did, row));
        }
    }

    let doc_ids: Vec<u32> = doc_rows.iter().map(|(d, _)| *d).collect();
    let group_counts = group_hits_by_outline_node(&catalog, &doc_ids, target);

    // Sort groups by hit_count descending.
    let mut sorted_groups: Vec<(u32, u32)> = group_counts.into_iter().collect();
    sorted_groups.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let mut clusters: Vec<Value> = Vec::new();
    for (node_id, count) in sorted_groups.iter().take(limit_per_cluster) {
        let node = catalog.get_node(*node_id);
        let node_doc_range = node.and_then(|n| {
            n.first_doc_id.zip(n.last_doc_id)
        });

        // Pick top representative passages within this cluster.
        let mut reps: Vec<Value> = doc_rows.iter()
            .filter(|(did, _)| {
                if let Some((lo, hi)) = node_doc_range {
                    *did >= lo && *did <= hi
                } else {
                    false
                }
            })
            .take(3)
            .map(|(did, row)| {
                let mut r = row.clone();
                if let Some(obj) = r.as_object_mut() {
                    obj.insert("doc_id".to_string(), json!(*did));
                }
                r
            })
            .collect();
        reps.truncate(3);

        clusters.push(json!({
            "node_id": node_id,
            "label": node.map(|n| n.label.as_str()).unwrap_or(""),
            "heading_path": node.map(|n| n.heading_path.as_str()).unwrap_or(""),
            "node_kind": node.map(|n| format!("{:?}", &n.node_kind)).unwrap_or_default(),
            "hit_count": count,
            "representative_passages": reps,
        }));
    }

    let payload = json!({
        "schema": "sinoragd-cluster-hits-v1",
        "phrase": phrase,
        "cluster_by": cluster_by,
        "total_hits": doc_rows.len(),
        "cluster_count": sorted_groups.len(),
        "clusters": clusters,
        "search_strategy": {
            "phrase": phrase_strategy,
            "limit_total": limit_total,
            "limit_per_cluster": limit_per_cluster,
        }
    });
    write_or_print(&payload, out)
}
