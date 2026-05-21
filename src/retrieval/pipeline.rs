use crate::retrieval::{Candidate, ContextBlock, VerifiedHit};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RetrievalStageReport {
    pub name: String,
    pub candidate_count: usize,
    pub returned_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl RetrievalStageReport {
    pub fn new(name: impl Into<String>, candidate_count: usize, returned_count: usize) -> Self {
        Self {
            name: name.into(),
            candidate_count,
            returned_count,
            verified_count: None,
            elapsed_ms: None,
            details: None,
            warnings: Vec::new(),
        }
    }

    pub fn with_verified_count(mut self, verified_count: usize) -> Self {
        self.verified_count = Some(verified_count);
        self
    }

    pub fn with_elapsed_ms(mut self, elapsed_ms: u128) -> Self {
        self.elapsed_ms = Some(elapsed_ms);
        self
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct RetrievalTraceBuilder {
    stages: Vec<RetrievalStageReport>,
}

impl RetrievalTraceBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, stage: RetrievalStageReport) -> &mut Self {
        self.stages.push(stage);
        self
    }

    pub fn stage(
        &mut self,
        name: impl Into<String>,
        candidate_count: usize,
        returned_count: usize,
    ) -> &mut Self {
        self.push(RetrievalStageReport::new(
            name,
            candidate_count,
            returned_count,
        ))
    }

    pub fn finish(self) -> Vec<RetrievalStageReport> {
        self.stages
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RetrievalPipelineResult {
    pub candidates: Vec<Candidate>,
    pub verified: Vec<VerifiedHit>,
    pub selected_context: Vec<ContextBlock>,
    pub stages: Vec<RetrievalStageReport>,
    pub warnings: Vec<String>,
}
