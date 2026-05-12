use crate::catalog_index::{CorpusCatalogIndex, NodeId, OutlineNodeKind};
use crate::document_table::DocumentTable;
use crate::research_tools::common::ToolScope;
use anyhow::Result;
use rustc_hash::FxHashSet;

/// Compiled, fast predicate form of a ToolScope.
pub struct CompiledScope<'a> {
    pub catalog: &'a CorpusCatalogIndex,
    pub doc_table: &'a DocumentTable,
    pub allowed_doc_range: Option<(u32, u32)>,
    pub source_work_ids: Option<FxHashSet<u32>>,
    pub period_names: Option<FxHashSet<String>>,
    pub canon_names: Option<FxHashSet<String>>,
    pub author_names: Option<FxHashSet<String>>,
}

impl<'a> CompiledScope<'a> {
    pub fn compile(
        catalog: &'a CorpusCatalogIndex,
        doc_table: &'a DocumentTable,
        scope: ToolScope,
    ) -> Result<Self> {
        let allowed_doc_range = if let Some(node_id) = scope.catalog_node_id {
            let node = catalog
                .get_node(node_id)
                .ok_or_else(|| anyhow::anyhow!("unknown catalog node_id {}", node_id))?;
            match (node.first_doc_id, node.last_doc_id) {
                (Some(lo), Some(hi)) => Some((lo, hi)),
                _ => None,
            }
        } else {
            None
        };

        let source_work_ids = scope.source_work_id.map(|ids| {
            ids.iter()
                .filter_map(|w| doc_table.work_id(w))
                .collect::<FxHashSet<u32>>()
        });

        let period_names = scope.period.map(|v| v.into_iter().collect());
        let canon_names = scope.canon.map(|v| v.into_iter().collect());
        let author_names = scope.author.map(|v| v.into_iter().collect());

        Ok(Self {
            catalog,
            doc_table,
            allowed_doc_range,
            source_work_ids,
            period_names,
            canon_names,
            author_names,
        })
    }

    pub fn contains_doc(&self, doc_id: u32) -> bool {
        if let Some((lo, hi)) = self.allowed_doc_range {
            if doc_id < lo || doc_id > hi {
                return false;
            }
        }

        if let Some(ref work_ids) = self.source_work_ids {
            let wid = self.doc_table.source_work_ids.get(doc_id as usize).copied().unwrap_or(u32::MAX);
            if !work_ids.contains(&wid) {
                return false;
            }
        }

        // Period/canon/author checks require passage-level metadata from parquet.
        // These are applied post-hoc in the tool commands after fetching rows.
        // The doc_range + work_id checks above are the fast pre-filter.

        true
    }

    /// Get the doc range for a catalog node.
    pub fn scope_from_node(
        catalog: &CorpusCatalogIndex,
        node_id: NodeId,
    ) -> Result<(u32, u32)> {
        let node = catalog
            .get_node(node_id)
            .ok_or_else(|| anyhow::anyhow!("unknown catalog node_id {}", node_id))?;
        match (node.first_doc_id, node.last_doc_id) {
            (Some(lo), Some(hi)) => Ok((lo, hi)),
            _ => anyhow::bail!("node {} has no doc range", node_id),
        }
    }
}

/// Climb from a leaf node up to a target outline level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineSearchLevel {
    Work,
    Division,
    PassageRange,
}

pub fn climb_to_level(
    catalog: &CorpusCatalogIndex,
    mut node_id: NodeId,
    target: OutlineSearchLevel,
) -> Option<NodeId> {
    loop {
        let node = catalog.get_node(node_id)?;
        let matches = match (target, &node.node_kind) {
            (OutlineSearchLevel::Work, OutlineNodeKind::Work) => true,
            (OutlineSearchLevel::Division, OutlineNodeKind::Division) => true,
            (OutlineSearchLevel::PassageRange, OutlineNodeKind::PassageRange) => true,
            _ => false,
        };
        if matches {
            return Some(node_id);
        }
        node_id = node.parent_id?;
    }
}

pub fn group_hits_by_outline_node(
    catalog: &CorpusCatalogIndex,
    doc_ids: &[u32],
    target: OutlineSearchLevel,
) -> rustc_hash::FxHashMap<NodeId, u32> {
    let mut counts = rustc_hash::FxHashMap::default();
    for &doc_id in doc_ids {
        let Some(&leaf_node) = catalog.doc_parent.get(&doc_id) else {
            continue;
        };
        if let Some(group_node) = climb_to_level(catalog, leaf_node, target) {
            *counts.entry(group_node).or_insert(0) += 1;
        }
    }
    counts
}
