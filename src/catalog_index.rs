use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

pub type NodeId = u32;
pub type DocId = u32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusCatalogIndex {
    pub schema: String,
    pub source_fingerprint: Option<String>,
    pub doc_table_fingerprint: Option<String>,

    pub works: Vec<WorkRecord>,
    pub nodes: Vec<OutlineNode>,

    pub work_id_map: FxHashMap<String, usize>,
    pub doc_parent: FxHashMap<DocId, NodeId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkRecord {
    pub work_id: String,
    pub source_corpus: String,
    pub canon: String,
    pub canon_name: String,
    pub main_title: String,
    pub author: String,
    pub period: String,
    pub period_rank: i32,
    pub origin: String,
    pub traditions: Vec<String>,
    pub source_rel_paths: Vec<String>,
    pub root_node: NodeId,
    pub passage_count: u32,
    pub cjk_char_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutlineNode {
    pub node_id: NodeId,
    pub parent_id: Option<NodeId>,
    pub children: Vec<NodeId>,

    pub source_corpus: String,
    pub work_id: String,
    pub source_rel_path: String,

    pub node_kind: OutlineNodeKind,
    pub label: String,
    pub heading_path: String,
    pub div_path: String,

    pub first_doc_id: Option<DocId>,
    pub last_doc_id: Option<DocId>,
    pub passage_count: u32,
    pub cjk_char_count: u32,

    pub from_lb: Option<String>,
    pub to_lb: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutlineNodeKind {
    Corpus,
    Canon,
    Work,
    Volume,
    Fascicle,
    Chapter,
    Section,
    Division,
    PassageRange,
}

impl CorpusCatalogIndex {
    pub fn new() -> Self {
        Self {
            schema: "readzen-corpus-catalog-v2".to_string(),
            source_fingerprint: None,
            doc_table_fingerprint: None,
            works: Vec::new(),
            nodes: Vec::new(),
            work_id_map: FxHashMap::default(),
            doc_parent: FxHashMap::default(),
        }
    }

    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bincode::serialize(self)?)?;
        Ok(())
    }

    pub fn save_atomic(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let bytes = bincode::serialize(self)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!(
            "{}.tmp",
            path.extension()
                .and_then(|s| s.to_str())
                .unwrap_or("bin")
        ));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        Ok(bincode::deserialize(&std::fs::read(path)?)?)
    }

    pub fn info_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": self.schema,
            "works": self.works.len(),
            "nodes": self.nodes.len(),
            "source_fingerprint": self.source_fingerprint,
        })
    }

    pub fn get_work(&self, work_id: &str) -> Option<&WorkRecord> {
        self.work_id_map.get(work_id).and_then(|idx| self.works.get(*idx))
    }

    pub fn get_node(&self, node_id: NodeId) -> Option<&OutlineNode> {
        self.nodes.get(node_id as usize)
    }

    pub fn get_children(&self, node_id: NodeId) -> Option<Vec<&OutlineNode>> {
        let node = self.get_node(node_id)?;
        Some(
            node.children
                .iter()
                .filter_map(|&child_id| self.get_node(child_id))
                .collect()
        )
    }
}
