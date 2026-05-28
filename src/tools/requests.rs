use serde::Deserialize;
use std::path::PathBuf;

/// Request for the search tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SearchRequest {
    pub phrase: String,

    #[serde(default = "default_limit")]
    pub limit: usize,

    #[serde(default = "default_search_mode")]
    pub mode: String,

    #[serde(default = "default_search_depth")]
    pub depth: String,

    #[serde(default = "default_cluster_by")]
    pub group_by: String,

    #[serde(default)]
    pub include_variants: bool,

    #[serde(default = "default_limit_per_group")]
    pub limit_per_group: usize,

    #[serde(default)]
    pub brief: bool,

    #[serde(default)]
    pub canon: Option<String>,

    #[serde(default)]
    pub source_work_id: Option<String>,

    #[serde(default)]
    pub tradition: Option<String>,

    #[serde(default)]
    pub period: Option<String>,

    #[serde(default)]
    pub origin: Option<String>,

    #[serde(default)]
    pub author: Option<String>,

    #[serde(default)]
    pub title: Option<String>,

    #[serde(default)]
    pub heading_path_prefix: Option<String>,
}

fn default_limit() -> usize {
    20
}

/// Request for the heading-search tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct HeadingSearchRequest {
    pub query: String,

    #[serde(default = "default_limit")]
    pub limit: usize,

    #[serde(default)]
    pub canon: Option<String>,

    #[serde(default)]
    pub source_work_id: Option<String>,

    #[serde(default)]
    pub period: Option<String>,

    #[serde(default)]
    pub brief: bool,
}

/// Request for the tool-docs tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ToolDocsRequest {
    #[serde(default)]
    pub tool: Option<String>,
}

fn default_search_mode() -> String {
    "hits".to_string()
}

fn default_search_depth() -> String {
    "exact".to_string()
}

/// Request for the passage tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct PassageRequest {
    #[serde(alias = "passage_id")]
    pub id: String,
}

/// Request for the source-read tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SourceReadRequest {
    #[serde(default)]
    pub source_work_id: Option<String>,

    #[serde(default)]
    pub passage_id: Option<String>,

    #[serde(default)]
    pub node_id: Option<u32>,

    #[serde(default)]
    pub cursor: Option<String>,

    #[serde(default = "default_read_direction")]
    pub direction: String,

    #[serde(default = "default_read_unit")]
    pub unit: String,

    #[serde(default = "default_source_read_max_chars")]
    pub max_chars: usize,

    #[serde(default = "default_source_read_overlap_chars")]
    pub overlap_chars: usize,

    #[serde(default)]
    pub before_chars: Option<usize>,

    #[serde(default)]
    pub after_chars: Option<usize>,

    #[serde(default = "default_true")]
    pub include_previous_tail: bool,

    #[serde(default = "default_true")]
    pub include_next_head: bool,

    #[serde(default = "default_true")]
    pub include_metadata: bool,
}

fn default_read_direction() -> String {
    "start".to_string()
}

fn default_read_unit() -> String {
    "chunk".to_string()
}

fn default_source_read_max_chars() -> usize {
    4000
}

fn default_source_read_overlap_chars() -> usize {
    400
}

/// Request for the canonical-source tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct CanonicalSourceRequest {
    pub phrase: String,

    #[serde(default = "default_limit")]
    pub limit: usize,

    #[serde(default)]
    pub canon: Option<String>,
}

/// Request for the status tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct StatusRequest {}

/// Request for the validate-adjudication tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ValidateAdjudicationRequest {
    pub path: PathBuf,
}

/// Request for the graph-build tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct GraphBuildRequest {
    pub input: PathBuf,
    pub kind: String,
    pub name: String,
    pub out: PathBuf,
}

/// Request for the report-build tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ReportBuildRequest {
    pub inputs: Vec<PathBuf>,
    pub out: PathBuf,

    #[serde(default)]
    pub title: Option<String>,

    #[serde(default = "default_essay_max_pages")]
    pub essay_max_pages: usize,
}

fn default_essay_max_pages() -> usize {
    5
}

/// Request for the pdf-build tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct PdfBuildRequest {
    #[serde(default)]
    pub input_markdown: Option<PathBuf>,

    #[serde(default)]
    pub input_json: Option<PathBuf>,

    pub out: PathBuf,

    #[serde(default)]
    pub side_by_side: bool,

    #[serde(default)]
    pub title: Option<String>,

    #[serde(default = "default_essay_max_pages")]
    pub essay_max_pages: usize,
}

/// Request for the works tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct WorksRequest {
    #[serde(default)]
    pub tradition: Option<String>,

    #[serde(default)]
    pub period: Option<String>,

    #[serde(default)]
    pub canon: Option<String>,

    #[serde(default)]
    pub author: Option<String>,

    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// Request for the catalog-index-info tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct CatalogIndexInfoRequest {}

/// Request for the similar tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SimilarRequest {
    pub seed: String,

    #[serde(default = "default_limit")]
    pub limit: usize,

    #[serde(default = "default_shared_ngram_limit")]
    pub shared_ngram_limit: usize,

    #[serde(default = "default_shared_phrase_limit")]
    pub shared_phrase_limit: usize,

    #[serde(default = "default_min_shared_phrase_len")]
    pub min_shared_phrase_len: usize,
}

fn default_shared_ngram_limit() -> usize {
    12
}

fn default_shared_phrase_limit() -> usize {
    8
}

fn default_min_shared_phrase_len() -> usize {
    4
}

/// Request for the frontier tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct FrontierRequest {
    pub seed: String,

    #[serde(default = "default_limit")]
    pub limit: usize,

    #[serde(default = "default_phrase_limit")]
    pub phrase_limit: usize,
}

fn default_phrase_limit() -> usize {
    20
}

/// Request for the first-attestation tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct FirstAttestationRequest {
    pub phrase: String,

    #[serde(default)]
    pub scope_canon: Vec<String>,

    #[serde(default)]
    pub scope_period: Vec<String>,

    #[serde(default)]
    pub scope_source_work_id: Option<String>,

    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// Request for the phrase-history tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct PhraseHistoryRequest {
    pub phrase: String,

    #[serde(default)]
    pub include_variants: bool,

    #[serde(default)]
    pub timeline: bool,
}

/// Request for the phrase-index-search tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct PhraseIndexSearchRequest {
    pub phrase: String,

    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// Request for the seed-pick tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SeedPickRequest {
    #[serde(default)]
    pub tradition: Vec<String>,

    #[serde(default)]
    pub period: Vec<String>,

    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// Request for the expand-context-adaptive tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ExpandContextAdaptiveRequest {
    pub passage_id: String,

    #[serde(default = "default_max_chars")]
    pub max_chars: usize,
}

fn default_max_chars() -> usize {
    5000
}

/// Request for the trace-term-usage tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct TraceTermUsageRequest {
    pub phrase: String,

    #[serde(default = "default_group_by")]
    pub group_by: String,

    #[serde(default = "default_limit_total")]
    pub limit_total: usize,

    #[serde(default = "default_limit_per_group")]
    pub limit_per_group: usize,
}

fn default_group_by() -> String {
    "period".to_string()
}

fn default_outline_group_by() -> String {
    "division".to_string()
}

/// Request for the cluster-hits tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ClusterHitsRequest {
    pub phrase: String,

    #[serde(default = "default_cluster_by")]
    pub cluster_by: String,

    #[serde(default = "default_limit_total")]
    pub limit_total: usize,

    #[serde(default = "default_limit_per_cluster")]
    pub limit_per_cluster: usize,
}

fn default_cluster_by() -> String {
    "work".to_string()
}

fn default_limit_per_cluster() -> usize {
    20
}

/// Request for the absence-check tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct AbsenceCheckRequest {
    pub phrase: String,

    #[serde(default)]
    pub scope_work_id: Option<String>,

    #[serde(default)]
    pub scope_canon: Vec<String>,

    #[serde(default)]
    pub scope_period: Option<String>,

    #[serde(default)]
    pub scope_node_id: Option<u32>,

    #[serde(default = "default_absence_limit")]
    pub limit: usize,
}

fn default_absence_limit() -> usize {
    100
}

fn default_limit_total() -> usize {
    200
}

fn default_limit_per_group() -> usize {
    5
}

/// Request for the query-expand-terms tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct QueryExpandTermsRequest {
    pub phrase: String,

    #[serde(default = "default_expand_mode")]
    pub mode: String,

    #[serde(default)]
    pub person_aliases: Vec<String>,

    #[serde(default = "default_max")]
    pub max: usize,
}

fn default_expand_mode() -> String {
    "all".to_string()
}

fn default_max() -> usize {
    20
}

/// Request for the compare-usage tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct CompareUsageRequest {
    #[serde(default)]
    pub scope_a_node_id: Option<u32>,

    #[serde(default)]
    pub scope_a_work_id: Option<String>,

    #[serde(default)]
    pub scope_a_canon: Option<String>,

    #[serde(default)]
    pub scope_a_period: Option<String>,

    #[serde(default)]
    pub scope_b_node_id: Option<u32>,

    #[serde(default)]
    pub scope_b_work_id: Option<String>,

    #[serde(default)]
    pub scope_b_canon: Option<String>,

    #[serde(default)]
    pub scope_b_period: Option<String>,

    #[serde(default = "default_gram_len")]
    pub gram_len: usize,

    #[serde(default = "default_limit_passages")]
    pub limit_passages: usize,

    #[serde(default = "default_limit_terms")]
    pub limit_terms: usize,
}

fn default_gram_len() -> usize {
    1
}

fn default_limit_passages() -> usize {
    1000
}

fn default_limit_terms() -> usize {
    50
}

/// Request for the collocation-search tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct CollocationSearchRequest {
    pub phrase: String,

    #[serde(default = "default_window_chars")]
    pub window_chars: usize,

    #[serde(default = "default_gram_len")]
    pub gram_len: usize,

    #[serde(default = "default_limit_total")]
    pub limit_total: usize,

    #[serde(default = "default_limit_collocates")]
    pub limit_collocates: usize,

    /// Restrict search to a specific canon (e.g. "T" for Taishō).
    #[serde(default)]
    pub scope_canon: Option<String>,

    /// Restrict search to a specific period (e.g. "Song", "Tang").
    #[serde(default)]
    pub scope_period: Option<String>,

    /// Restrict search to a specific work by source_work_id.
    #[serde(default)]
    pub scope_source_work_id: Option<String>,

    /// Restrict search to passages under a catalog node_id.
    #[serde(default)]
    pub scope_node_id: Option<u32>,
}

/// Request for the pair-appearance tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct PairAppearanceRequest {
    pub term1: String,
    pub term2: String,

    #[serde(default = "default_pair_unit")]
    pub unit: String,

    #[serde(default = "default_pair_window_chars")]
    pub window_chars: usize,

    #[serde(default)]
    pub ordered: bool,

    #[serde(default)]
    pub allow_variants: bool,

    #[serde(default)]
    pub scope_canon: Option<String>,

    #[serde(default)]
    pub scope_period: Option<String>,

    #[serde(default)]
    pub scope_source_work_id: Option<String>,

    #[serde(default)]
    pub scope_node_id: Option<u32>,

    #[serde(default = "default_limit")]
    pub limit: usize,

    #[serde(default = "default_pair_candidate_limit")]
    pub max_candidates_per_term: usize,

    #[serde(default = "default_true")]
    pub include_snippets: bool,

    #[serde(default)]
    pub include_negative_summary: bool,
}

fn default_pair_unit() -> String {
    "passage".to_string()
}

fn default_pair_window_chars() -> usize {
    80
}

fn default_pair_candidate_limit() -> usize {
    10_000
}

fn default_window_chars() -> usize {
    20
}

fn default_limit_collocates() -> usize {
    30
}

/// Request for the outline-search tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct OutlineSearchRequest {
    pub phrase: String,

    #[serde(default)]
    pub node_id: Option<u32>,

    #[serde(default)]
    pub work_id: Option<String>,

    #[serde(default = "default_outline_group_by")]
    pub group_by: String,

    #[serde(default = "default_limit_total")]
    pub limit_total: usize,

    #[serde(default = "default_limit_per_group")]
    pub limit_per_group: usize,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct VectorInfoRequest {}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct VectorNeighborsRequest {
    #[serde(default)]
    pub seed_passage_id: Option<String>,

    #[serde(default)]
    pub query_embedding: Option<Vec<f32>>,

    #[serde(default)]
    pub query_text: Option<String>,

    #[serde(default = "default_vector_k")]
    pub k: usize,

    #[serde(default = "default_ef_search")]
    pub ef_search: usize,

    #[serde(default = "default_true")]
    pub include_text: bool,

    #[serde(default = "default_true")]
    pub rerank: bool,
}

fn default_vector_k() -> usize {
    25
}

fn default_ef_search() -> usize {
    64
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct EvidenceSearchRequest {
    pub phrase: String,

    #[serde(default = "default_limit")]
    pub limit: usize,

    #[serde(default)]
    pub scope_canon: Option<String>,

    #[serde(default)]
    pub scope_period: Option<String>,

    #[serde(default)]
    pub scope_source_work_id: Option<String>,

    #[serde(default)]
    pub scope_node_id: Option<u32>,

    #[serde(default)]
    pub author: Option<String>,

    #[serde(default)]
    pub title: Option<String>,

    #[serde(default)]
    pub heading_path_prefix: Option<String>,

    #[serde(default)]
    pub include_attestation: bool,

    #[serde(default)]
    pub include_history: bool,

    #[serde(default)]
    pub include_usage: bool,

    #[serde(default)]
    pub include_clusters: bool,

    #[serde(default)]
    pub include_absence_check: bool,

    #[serde(default = "default_variant_policy")]
    pub variant_policy: String,

    #[serde(default = "default_workflow_quality")]
    pub quality: String,

    #[serde(default)]
    pub max_elapsed_ms: Option<u64>,

    #[serde(default)]
    pub max_component_ms: Option<u64>,

    #[serde(default = "default_max_candidates")]
    pub max_candidates: usize,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct HybridDiscoverRequest {
    #[serde(default)]
    pub seed_passage_id: Option<String>,

    #[serde(default)]
    pub query_embedding: Option<Vec<f32>>,

    #[serde(default = "default_limit")]
    pub limit: usize,

    #[serde(default)]
    pub include_context: bool,

    #[serde(default = "default_workflow_quality")]
    pub quality: String,

    #[serde(default)]
    pub max_elapsed_ms: Option<u64>,

    #[serde(default)]
    pub max_component_ms: Option<u64>,

    #[serde(default = "default_max_candidates")]
    pub max_candidates: usize,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SourceInvestigateRequest {
    pub seed_passage_id: String,

    #[serde(default)]
    pub phrases: Vec<String>,

    #[serde(default = "default_limit")]
    pub limit: usize,

    #[serde(default = "default_max_chars")]
    pub max_context_chars: usize,

    #[serde(default = "default_true")]
    pub include_context: bool,

    #[serde(default = "default_true")]
    pub include_frontier: bool,

    #[serde(default = "default_true")]
    pub include_similar: bool,

    #[serde(default = "default_true")]
    pub include_vector: bool,

    #[serde(default = "default_workflow_quality")]
    pub quality: String,

    #[serde(default)]
    pub max_elapsed_ms: Option<u64>,

    #[serde(default)]
    pub max_component_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ScopeProfileRequest {
    #[serde(default)]
    pub phrase: Option<String>,

    #[serde(default)]
    pub scope_a_node_id: Option<u32>,

    #[serde(default)]
    pub scope_a_work_id: Option<String>,

    #[serde(default)]
    pub scope_a_canon: Option<String>,

    #[serde(default)]
    pub scope_a_period: Option<String>,

    #[serde(default)]
    pub scope_b_node_id: Option<u32>,

    #[serde(default)]
    pub scope_b_work_id: Option<String>,

    #[serde(default)]
    pub scope_b_canon: Option<String>,

    #[serde(default)]
    pub scope_b_period: Option<String>,

    #[serde(default = "default_gram_len")]
    pub gram_len: usize,

    #[serde(default = "default_limit_passages")]
    pub limit_passages: usize,

    #[serde(default = "default_limit_terms")]
    pub limit_terms: usize,

    #[serde(default = "default_workflow_quality")]
    pub quality: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct PlanToolsRequest {
    pub task: String,

    #[serde(default)]
    pub known_phrase: Option<String>,

    #[serde(default)]
    pub seed_passage_id: Option<String>,
}

/// Request for the batch-evidence-search tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct BatchEvidenceSearchRequest {
    pub phrases: Vec<String>,

    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_variant_policy() -> String {
    "suggest_only".to_string()
}

fn default_workflow_quality() -> String {
    "balanced".to_string()
}

fn default_max_candidates() -> usize {
    200
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ReportFromEvidenceRequest {
    pub adjudication: PathBuf,
    pub graph_out: PathBuf,
    pub report_out: PathBuf,

    #[serde(default = "default_report_kind")]
    pub kind: String,

    #[serde(default = "default_report_name")]
    pub name: String,

    #[serde(default)]
    pub title: Option<String>,
}

fn default_report_kind() -> String {
    "evidence".to_string()
}

fn default_report_name() -> String {
    "evidence-report".to_string()
}

/// Request for the pair-profile tool: co-occurrence statistics grouped by period/canon/work.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct PairProfileRequest {
    pub term1: String,
    pub term2: String,

    /// Dimension to group by: "period", "canon", "work", or "author".
    #[serde(default = "default_pair_profile_group_by")]
    pub group_by: String,

    /// Co-occurrence unit: "passage", "window", or "sentence".
    #[serde(default = "default_pair_unit")]
    pub unit: String,

    #[serde(default = "default_pair_window_chars")]
    pub window_chars: usize,

    #[serde(default)]
    pub allow_variants: bool,

    #[serde(default)]
    pub scope_canon: Option<String>,

    #[serde(default)]
    pub scope_period: Option<String>,

    #[serde(default)]
    pub scope_source_work_id: Option<String>,

    /// Maximum passages to scan per term (caps memory/time).
    #[serde(default = "default_pair_candidate_limit")]
    pub max_candidates_per_term: usize,

    /// Maximum number of groups returned.
    #[serde(default = "default_pair_profile_limit_groups")]
    pub limit_groups: usize,

    /// Number of representative pair hits to include per group.
    #[serde(default = "default_pair_profile_sample_hits")]
    pub sample_hits_per_group: usize,
}

fn default_pair_profile_group_by() -> String {
    "period".to_string()
}

fn default_pair_profile_limit_groups() -> usize {
    20
}

fn default_pair_profile_sample_hits() -> usize {
    3
}

/// Request for the person-resolve tool.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct PersonResolveRequest {
    /// Primary name form to resolve.
    pub name: String,

    /// Additional alias forms to search alongside the primary name.
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Request for the place-resolve tool.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct PlaceResolveRequest {
    /// Primary place name to resolve.
    pub name: String,

    /// Additional alternate name forms to search alongside the primary name.
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Request for the person-history tool.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct PersonHistoryRequest {
    /// Primary name form to search for.
    pub name: String,

    /// Additional alias forms to search alongside the primary name.
    #[serde(default)]
    pub aliases: Vec<String>,

    /// Maximum number of mentions to return.
    #[serde(default = "default_person_history_limit")]
    pub limit: usize,
}

fn default_person_history_limit() -> usize {
    200
}

/// Request for the citation-verify tool.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct CitationVerifyRequest {
    /// The claimed quotation text to verify.
    pub quote: String,

    /// Scope to a specific work by source_work_id (recommended when known).
    #[serde(default)]
    pub scope_source_work_id: Option<String>,

    /// Scope to a specific catalog node_id.
    #[serde(default)]
    pub scope_node_id: Option<u32>,

    /// Scope to a canon (e.g. "T" for Taishō).
    #[serde(default)]
    pub scope_canon: Option<String>,

    /// Claimed author or work title (informational; used in the response summary).
    #[serde(default)]
    pub claimed_attribution: Option<String>,

    /// Maximum exact hits to return.
    #[serde(default = "default_limit")]
    pub limit: usize,

    /// Try variant-expanded near-matches when exact search finds nothing.
    #[serde(default = "default_true")]
    pub include_near_matches: bool,

    /// Maximum near-match candidates to consider (TF-IDF based).
    #[serde(default = "default_citation_near_limit")]
    pub near_match_limit: usize,
}

fn default_citation_near_limit() -> usize {
    10
}
