use crate::retrieval::ScopeSpec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResearchTask {
    ExactEvidence,
    FirstAttestation,
    PairAppearance,
    CitationVerify,
    ScopeCompare,
    SimilarityDiscovery,
    SourceRead,
    AbsenceCheck,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RetrievalBudget {
    pub requested: usize,
    pub returned_limit: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_elapsed_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_component_ms: Option<u64>,
}

impl RetrievalBudget {
    pub fn new(limit: usize, max_candidates: usize) -> Self {
        let returned_limit = limit.max(1);
        let requested = max_candidates.max(returned_limit);
        Self {
            requested,
            returned_limit,
            max_elapsed_ms: None,
            max_component_ms: None,
        }
    }

    pub fn with_time_limits(mut self, max_elapsed_ms: Option<u64>, max_component_ms: Option<u64>) -> Self {
        self.max_elapsed_ms = max_elapsed_ms;
        self.max_component_ms = max_component_ms;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QuerySpec {
    pub text: String,
    #[serde(default)]
    pub expanded_terms: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RetrievalRequest {
    pub task: ResearchTask,
    pub query: QuerySpec,
    pub scope: ScopeSpec,
    pub budget: RetrievalBudget,
    pub rank_profile: String,
}
