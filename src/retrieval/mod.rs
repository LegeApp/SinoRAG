#![allow(dead_code, unused_imports)]

pub mod candidate;
pub mod pipeline;
pub mod ranking;
pub mod request;
pub mod scope;

pub use candidate::{Candidate, CandidateScore, CandidateSource, ContextBlock, VerifiedHit};
pub use pipeline::{RetrievalPipelineResult, RetrievalStageReport, RetrievalTraceBuilder};
pub use ranking::{
    candidate_sources, final_score, rank_score, refresh_hybrid_scores, HybridRankProfile,
};
pub use request::{QuerySpec, ResearchTask, RetrievalBudget, RetrievalRequest};
pub use scope::ScopeSpec;
