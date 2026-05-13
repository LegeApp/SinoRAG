//! `expand-context-adaptive`: pick the smallest catalog node that contains
//! the seed passage and fits the char budget, then return every passage
//! inside it. Climbs the tree from the leaf PassageRange up through
//! Division → Work, stopping at Work. Never returns whole corpora.

use crate::catalog_index::{CorpusCatalogIndex, OutlineNodeKind};
use crate::datafusion_store::DataFusionStore;
use crate::jsonout::write_or_print;
use anyhow::{anyhow, Result};
use serde_json::json;
use std::path::PathBuf;

pub async fn run(
    parquet: PathBuf,
    catalog_path: PathBuf,
    passage_id: String,
    max_chars: usize,
    out: Option<PathBuf>,
) -> Result<()> {
    let catalog = CorpusCatalogIndex::load(&catalog_path)?;

    // doc_id lookup: the catalog itself doesn't expose passage_id → doc_id,
    // so we hop through doc_parent which is keyed by doc_id. The doc_table
    // is the authoritative source — load it from the catalog's sibling path.
    let doc_table_path = catalog_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("doc_table.bin");
    let doc_table = crate::document_table::DocumentTable::load(&doc_table_path)?;
    let doc_id = doc_table
        .doc_id(&passage_id)
        .ok_or_else(|| anyhow!("passage not found in doc_table: {passage_id}"))?;
    let mut node_id = *catalog
        .doc_parent
        .get(&doc_id)
        .ok_or_else(|| anyhow!("doc_id {doc_id} has no catalog node (rebuild catalog?)"))?;

    let leaf_kind = catalog
        .get_node(node_id)
        .map(|n| format!("{:?}", n.node_kind))
        .unwrap_or_default();
    let mut climbed = 0u32;
    // Climb until the node's cjk_char_count fits the budget or we reach Work.
    loop {
        let node = catalog
            .get_node(node_id)
            .ok_or_else(|| anyhow!("bad node_id"))?;
        let fits = (node.cjk_char_count as usize) <= max_chars;
        let at_work = matches!(node.node_kind, OutlineNodeKind::Work);
        if fits || at_work {
            break;
        }
        match node.parent_id {
            Some(parent) => {
                node_id = parent;
                climbed += 1;
                let parent_node = catalog
                    .get_node(node_id)
                    .ok_or_else(|| anyhow!("bad parent_id"))?;
                if matches!(
                    parent_node.node_kind,
                    OutlineNodeKind::Canon | OutlineNodeKind::Corpus
                ) {
                    // Don't go above Work; revert to the prior node.
                    node_id = parent_node.children.first().copied().unwrap_or(node_id);
                    break;
                }
            }
            None => break,
        }
    }

    let selected = catalog
        .get_node(node_id)
        .ok_or_else(|| anyhow!("no selected node"))?;
    let first = selected
        .first_doc_id
        .ok_or_else(|| anyhow!("node has no doc range"))?;
    let last = selected
        .last_doc_id
        .ok_or_else(|| anyhow!("node has no doc range"))?;

    // Fetch every passage with doc_id in [first, last] for the selected work.
    let mut passage_ids: Vec<String> = Vec::with_capacity((last - first + 1) as usize);
    for did in first..=last {
        if let Some(pid) = doc_table.passage_id(did) {
            passage_ids.push(pid.to_string());
        }
    }
    let store = DataFusionStore::open(&parquet).await?;
    let rows = store
        .passages_by_ids(
            &passage_ids,
            "passage_id, main_title, source_work_id, source_rel_path, \
         from_lb, to_lb, period, zh_text_normalized as zh_text",
        )
        .await?;

    let char_count: usize = rows
        .iter()
        .filter_map(|r| r.get("zh_text").and_then(|v| v.as_str()))
        .map(|t| t.chars().count())
        .sum();

    let payload = json!({
        "schema": "sinoragd-expand-context-adaptive-v1",
        "seed_passage_id": passage_id,
        "selected_node_id": selected.node_id,
        "selected_node_kind": format!("{:?}", selected.node_kind),
        "selected_label": selected.label,
        "heading_path": selected.heading_path,
        "work_id": selected.work_id,
        "passage_count": rows.len(),
        "char_count": char_count,
        "passages": rows,
        "search_strategy": {
            "budget": max_chars,
            "climbed_levels": climbed,
            "leaf_kind": leaf_kind,
            "mode": "auto",
        }
    });
    write_or_print(&payload, out)
}
