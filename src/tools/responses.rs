use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

use crate::tools::errors::ToolErrorBody;

/// Response from the search tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SearchResponse {
    pub schema: &'static str,
    pub phrase: String,
    pub mode: String,
    pub brief: bool,
    pub expanded_phrases: Vec<String>,
    pub hits: Vec<SearchHit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clusters: Option<Vec<ClusterHitsCluster>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_groups: Option<Vec<TermUsageGroup>>,
    pub search_strategy: SearchStrategy,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SearchHit {
    pub passage_id: String,
    pub source_work_id: Option<String>,
    pub main_title: Option<String>,
    pub heading: Option<String>,
    pub zh_quote: String,
    pub score: Option<f32>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SearchStrategy {
    pub method: String,
    pub filters: Value,
    pub candidate_count: Option<usize>,
    pub verified_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_phrase_index: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComponentStatus {
    Ok,
    SkippedNotRequested,
    SkippedUnavailable,
    Failed,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct WorkflowComponent {
    pub name: String,
    pub tool: String,
    pub status: ComponentStatus,
    pub used: bool,
    pub elapsed_ms: Option<u128>,
    pub summary: Option<String>,
    pub error: Option<ToolErrorBody>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SuggestedToolCall {
    pub tool: String,
    pub args: serde_json::Value,
    pub reason: String,
}

/// Response from the heading-search tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct HeadingSearchResponse {
    pub schema: &'static str,
    pub query: String,
    pub brief: bool,
    pub returned_count: usize,
    pub sections: Vec<HeadingSearchHit>,
    pub search_strategy: SearchStrategy,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct HeadingSearchHit {
    pub source_work_id: Option<String>,
    pub main_title: Option<String>,
    pub heading: Option<String>,
    pub heading_path: Option<String>,
    pub passage_id: String,
    pub sample: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Response from the tool-docs tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ToolDocsResponse {
    pub schema: &'static str,
    pub tool: Option<String>,
    pub docs: serde_json::Value,
}

/// Response from the passage tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct PassageResponse {
    pub schema: &'static str,
    pub passage_id: String,
    pub zh_quote: String,
    pub source_work_id: Option<String>,
    pub main_title: Option<String>,
    pub heading: Option<String>,
}

/// Response from the canonical-source tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CanonicalSourceResponse {
    pub schema: &'static str,
    pub phrase: String,
    pub hits: Vec<CanonicalSourceHit>,
    pub search_strategy: SearchStrategy,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CanonicalSourceHit {
    pub passage_id: String,
    pub source_work_id: Option<String>,
    pub main_title: Option<String>,
    pub heading: Option<String>,
    pub zh_quote: String,
    pub is_canon_side: bool,
}

/// Response from the status tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct StatusResponse {
    pub schema: &'static str,
    pub data_root: String,
    pub passages_parquet_exists: bool,
    pub phrase_index_exists: bool,
    pub tfidf_index_exists: bool,
    pub vector_index_exists: bool,
    pub catalog_index_exists: bool,
    pub doc_table_exists: bool,
    pub registry_exists: bool,
}

/// Response from the validate-adjudication tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ValidateAdjudicationResponse {
    pub schema: &'static str,
    pub path: PathBuf,
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Response from the graph-build tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct GraphBuildResponse {
    pub schema: &'static str,
    pub out: PathBuf,
    pub node_count: usize,
    pub edge_count: usize,
}

/// Response from the report-build tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ReportBuildResponse {
    pub schema: &'static str,
    pub out: PathBuf,
    pub section_count: usize,
}

/// Response from the works tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct WorksResponse {
    pub schema: &'static str,
    pub works: Vec<WorkInfo>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct WorkInfo {
    pub work_id: String,
    pub main_title: String,
    pub author: Option<String>,
    pub period: Option<String>,
    pub canon: Option<String>,
    pub traditions: Vec<String>,
    pub passage_count: usize,
}

/// Response from the catalog-index-info tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CatalogIndexInfoResponse {
    pub schema: &'static str,
    #[serde(flatten)]
    pub info: serde_json::Value,
}

/// Response from the similar tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SimilarResponse {
    pub schema: &'static str,
    pub seed: String,
    pub similar_passages: Vec<serde_json::Value>,
}

/// Response from the frontier tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct FrontierResponse {
    pub schema: &'static str,
    pub seed_passage_id: String,
    #[serde(flatten)]
    pub payload: serde_json::Value,
}

/// Response from the first-attestation tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct FirstAttestationResponse {
    pub schema: &'static str,
    pub phrase: String,
    pub first: Option<serde_json::Value>,
    pub next_earlier: Vec<serde_json::Value>,
    pub scope: ScopeInfo,
    pub search_strategy: SearchStrategyInfo,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ScopeInfo {
    pub canon: Vec<String>,
    pub period: Vec<String>,
    pub source_work_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SearchStrategyInfo {
    pub used_phrase_index: bool,
    pub candidates_verified: usize,
    pub after_scope_and_sort: usize,
    pub limit: usize,
}

/// Response from the phrase-history tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct PhraseHistoryResponse {
    pub schema: &'static str,
    #[serde(flatten)]
    pub payload: serde_json::Value,
}

/// Response from the phrase-index-search tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct PhraseIndexSearchResponse {
    pub schema: &'static str,
    pub phrase: String,
    pub returned_count: usize,
    pub limit: usize,
    pub search_strategy: serde_json::Value,
    pub results: Vec<serde_json::Value>,
}

/// Response from the seed-pick tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SeedPickResponse {
    pub schema: &'static str,
    pub limit: usize,
    pub already_worked_count: usize,
    pub filters: FilterInfo,
    pub candidates: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct FilterInfo {
    pub tradition: Vec<String>,
    pub period: Vec<String>,
}

/// Response from the expand-context-adaptive tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ExpandContextAdaptiveResponse {
    pub schema: &'static str,
    pub seed_passage_id: String,
    pub selected_node_id: u32,
    pub selected_node_kind: String,
    pub selected_label: String,
    pub heading_path: Vec<String>,
    pub work_id: Option<String>,
    pub passage_count: usize,
    pub char_count: usize,
    pub passages: Vec<serde_json::Value>,
    pub search_strategy: SearchStrategyInfoAdaptive,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SearchStrategyInfoAdaptive {
    pub budget: usize,
    pub climbed_levels: u32,
    pub leaf_kind: String,
    pub mode: String,
}

/// Response from the trace-term-usage tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct TraceTermUsageResponse {
    pub schema: &'static str,
    pub phrase: String,
    pub group_by: String,
    pub groups: Vec<TermUsageGroup>,
    pub search_strategy: TermUsageSearchStrategy,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct TermUsageGroup {
    pub key: String,
    pub hit_count: u32,
    pub work_count: usize,
    pub top_works: Vec<String>,
    pub representative_passages: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct TermUsageSearchStrategy {
    pub used_phrase_index: bool,
    pub total_hits: usize,
    pub limit_total: usize,
    pub limit_per_group: usize,
}

/// Response from the query-expand-terms tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct QueryExpandTermsResponse {
    pub schema: &'static str,
    pub input: String,
    pub expanded: Vec<String>,
    pub by_source: ExpandTermsBySource,
    pub search_strategy: ExpandTermsSearchStrategy,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ExpandTermsBySource {
    pub variants: Vec<String>,
    pub orthographic: Vec<String>,
    pub persons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ExpandTermsSearchStrategy {
    pub mode: String,
    pub max: usize,
    pub input_lang_guess: String,
}

/// Response from the compare-usage tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CompareUsageResponse {
    pub schema: &'static str,
    pub scope_a: CompareUsageScope,
    pub scope_b: CompareUsageScope,
    pub distinctive_to_a: Vec<CompareUsageTerm>,
    pub distinctive_to_b: Vec<CompareUsageTerm>,
    pub search_strategy: CompareUsageSearchStrategy,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CompareUsageScope {
    pub node_id: Option<u32>,
    pub work_id: Option<String>,
    pub canon: Option<String>,
    pub period: Option<String>,
    pub passage_count: usize,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CompareUsageTerm {
    pub term: Option<String>,
    pub term_hash: u64,
    pub score: f32,
    pub a_count: u32,
    pub b_count: u32,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CompareUsageSearchStrategy {
    pub gram_len: usize,
    pub limit_passages: usize,
    pub limit_terms: usize,
}

/// Response from the collocation-search tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CollocationSearchResponse {
    pub schema: &'static str,
    pub phrase: String,
    pub window_chars: usize,
    pub gram_len: usize,
    pub total_passages: usize,
    pub near_ngram_count: u32,
    pub background_ngram_count: u32,
    pub collocates: Vec<CollocateTerm>,
    pub search_strategy: CollocationSearchStrategy,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CollocateTerm {
    pub term: Option<String>,
    pub term_hash: u64,
    pub score: f32,
    pub near_count: u32,
    pub background_count: u32,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CollocationSearchStrategy {
    pub phrase: serde_json::Value,
    pub limit_total: usize,
    pub limit_collocates: usize,
}

/// Response from the outline-search tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct OutlineSearchResponse {
    pub schema: &'static str,
    pub phrase: String,
    pub start_node_id: u32,
    pub start_label: String,
    pub group_by: String,
    pub total_hits: usize,
    pub group_count: usize,
    pub groups: Vec<OutlineSearchGroup>,
    pub search_strategy: OutlineSearchStrategy,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct OutlineSearchGroup {
    pub node_id: u32,
    pub label: String,
    pub heading_path: String,
    pub node_kind: String,
    pub hit_count: u32,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct OutlineSearchStrategy {
    pub phrase: serde_json::Value,
    pub limit_total: usize,
    pub limit_per_group: usize,
}

/// Response from the cluster-hits tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ClusterHitsResponse {
    pub schema: &'static str,
    pub phrase: String,
    pub cluster_by: String,
    pub total_hits: usize,
    pub cluster_count: usize,
    pub clusters: Vec<ClusterHitsCluster>,
    pub search_strategy: ClusterHitsSearchStrategy,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ClusterHitsCluster {
    pub node_id: u32,
    pub label: String,
    pub heading_path: String,
    pub node_kind: String,
    pub hit_count: u32,
    pub representative_passages: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ClusterHitsSearchStrategy {
    pub phrase: serde_json::Value,
    pub limit_total: usize,
    pub limit_per_cluster: usize,
}

/// Response from the absence-check tool
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct AbsenceCheckResponse {
    pub schema: &'static str,
    pub phrase: String,
    pub scope: AbsenceCheckScope,
    pub found: bool,
    pub hit_count: usize,
    pub sample_hits: Vec<serde_json::Value>,
    pub search_strategy: AbsenceCheckSearchStrategy,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct AbsenceCheckScope {
    pub work_id: Option<String>,
    pub canon: Vec<String>,
    pub period: Option<String>,
    pub node_id: Option<u32>,
    pub doc_range: Option<Vec<u32>>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct AbsenceCheckSearchStrategy {
    pub phrase: serde_json::Value,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct VectorInfoResponse {
    pub schema: &'static str,
    pub index_path: String,
    pub info: serde_json::Value,
    pub doc_table_fingerprint_match: bool,
    pub coverage: VectorCoverage,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct VectorCoverage {
    pub kind: String,
    pub covered_docs: u32,
    pub total_docs: u32,
    pub coverage_ratio: f64,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct VectorNeighborsResponse {
    pub schema: &'static str,
    pub query_mode: String,
    pub seed_passage_id: Option<String>,
    pub model_id: String,
    pub model_revision: String,
    pub embedding_dim: u32,
    pub distance: String,
    pub normalized: bool,
    pub rerank_requested: bool,
    pub rerank_applied: bool,
    pub score_interpretation: String,
    pub loading_index_ms: Option<u128>,
    pub hnsw_build_ms: Option<u128>,
    pub warnings: Vec<String>,
    pub hits: Vec<VectorNeighborHit>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct VectorNeighborHit {
    pub passage_id: String,
    pub doc_id: u32,
    pub ann_distance: f32,
    pub ann_score: f32,
    pub vector_score: f32,
    pub source_work_id: Option<String>,
    pub main_title: Option<String>,
    pub heading: Option<String>,
    pub period: Option<String>,
    pub snippet: Option<String>,
    pub warning: String,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct EvidenceSearchResponse {
    pub schema: &'static str,
    pub workflow: &'static str,
    pub phrase: String,
    pub expanded_terms: Vec<String>,
    pub expanded_terms_used: bool,
    pub variant_policy: String,
    pub exact: SearchResponse,
    pub absence_check: Option<AbsenceCheckResponse>,
    pub first_attestation: Option<FirstAttestationResponse>,
    pub phrase_history: Option<PhraseHistoryResponse>,
    pub usage: Option<TraceTermUsageResponse>,
    pub clusters: Option<ClusterHitsResponse>,
    pub components: Vec<WorkflowComponent>,
    pub suggested_next_tools: Vec<SuggestedToolCall>,
    pub indexes_used: Vec<String>,
    pub fallbacks: Vec<String>,
    pub evidence_status: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct HybridDiscoverResponse {
    pub schema: &'static str,
    pub workflow: &'static str,
    pub seed_passage_id: Option<String>,
    pub vector_neighbors: Option<VectorNeighborsResponse>,
    pub tfidf_similar: Option<SimilarResponse>,
    pub context: Option<ExpandContextAdaptiveResponse>,
    pub groups: HybridDiscoverGroups,
    pub merged_hits: Vec<HybridDiscoverHit>,
    pub components: Vec<WorkflowComponent>,
    pub suggested_next_tools: Vec<SuggestedToolCall>,
    pub indexes_used: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct HybridDiscoverGroups {
    pub semantic_candidates: Vec<String>,
    pub lexical_parallels: Vec<String>,
    pub overlap_candidates: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct HybridDiscoverHit {
    pub passage_id: String,
    pub labels: Vec<String>,
    pub evidence_status: String,
    pub vector_score: Option<f32>,
    pub vector_rank: Option<usize>,
    pub tfidf_score: Option<f32>,
    pub tfidf_rank: Option<usize>,
    pub merged_rank_reason: String,
    pub title: Option<String>,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SourceInvestigateResponse {
    pub schema: &'static str,
    pub workflow: &'static str,
    pub seed_passage_id: String,
    pub seed: PassageResponse,
    pub summary: SourceInvestigationSummary,
    pub context: Option<ExpandContextAdaptiveResponse>,
    pub frontier: Option<FrontierResponse>,
    pub similar: Option<SimilarResponse>,
    pub vector_neighbors: Option<VectorNeighborsResponse>,
    pub phrase_histories: Vec<PhraseHistoryResponse>,
    pub components: Vec<WorkflowComponent>,
    pub suggested_next_tools: Vec<SuggestedToolCall>,
    pub risk_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SourceInvestigationSummary {
    pub seed: String,
    pub best_next_actions: Vec<SuggestedToolCall>,
    pub risk_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ScopeProfileResponse {
    pub schema: &'static str,
    pub workflow: &'static str,
    pub phrase: Option<String>,
    pub comparison: CompareUsageResponse,
    pub term_usage: Option<TraceTermUsageResponse>,
    pub components: Vec<WorkflowComponent>,
    pub suggested_next_tools: Vec<SuggestedToolCall>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ReportFromEvidenceResponse {
    pub schema: &'static str,
    pub workflow: &'static str,
    pub validation: ValidateAdjudicationResponse,
    pub graph: Option<GraphBuildResponse>,
    pub report: Option<ReportBuildResponse>,
    pub components: Vec<WorkflowComponent>,
    pub suggested_next_tools: Vec<SuggestedToolCall>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct PlanToolsResponse {
    pub schema: &'static str,
    pub recommended_workflow: String,
    pub steps: Vec<SuggestedToolCall>,
    pub notes: Vec<String>,
}
