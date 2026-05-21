use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CandidateSource {
    PhraseIndex,
    ExactPhrase,
    Tfidf,
    Vector,
    Catalog,
    Metadata,
    PairAppearance,
}

impl CandidateSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            CandidateSource::PhraseIndex => "phrase_index",
            CandidateSource::ExactPhrase => "exact_phrase",
            CandidateSource::Tfidf => "tfidf",
            CandidateSource::Vector => "vector",
            CandidateSource::Catalog => "catalog",
            CandidateSource::Metadata => "metadata",
            CandidateSource::PairAppearance => "pair_appearance",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CandidateScore {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exact_score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lexical_score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proximity_score: Option<f32>,
    pub final_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Candidate {
    pub passage_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_id: Option<u32>,
    pub sources: Vec<CandidateSource>,
    pub score: CandidateScore,
    pub evidence_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct VerifiedHit {
    pub passage_id: String,
    pub verification: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ContextBlock {
    pub passage_id: String,
    pub text: String,
    pub selection_reason: String,
}
