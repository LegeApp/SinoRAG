use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScopeSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canon: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_work_id: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_node_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_corpus: Option<Vec<String>>,
}
