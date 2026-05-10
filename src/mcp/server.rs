//! MCP server exposing read-only SinoRAGD research tools over stdio.
//!
//! Tools are implemented as methods on `GraphDiscoveryServer` decorated with
//! `#[tool]` from the `rmcp` SDK. Each tool opens a `DataFusionStore` against
//! the configured parquet root (cached for the process lifetime) and calls the
//! existing async research helpers.
//!
//! The server provides comprehensive instructions to LLM agents based on the
//! GraphDiscovery documentation, including scope checks, query classification,
//! research lenses, and tool plans.

use anyhow::Result;
use rmcp::{
    Json, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::OnceCell;

use crate::cli::Command;
use crate::cli::Command::*;
use crate::commands;
use crate::commands::cef;
use crate::commands::expand_context;
use crate::commands::first_attestation;
use crate::commands::frontier;
use crate::commands::phrase_history;
use crate::commands::search;
use crate::commands::tfidf as commands_tfidf;
use crate::context_expand;
use crate::datafusion_store::DataFusionStore;
use crate::phrase_index;
use crate::registry;
use crate::search_packet::{SearchResultPacket, SearchHit};
use crate::tfidf::TfidfIndex;
use crate::tfidf;
use serde::Serialize;
use serde_json::json;

/// Typed research result structure following the sragd-research-result-v1 schema
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ResearchResult {
    pub schema: String,
    pub query: String,
    pub normalized_query: String,
    pub classification: String,
    pub tool_trace: Vec<String>,
    pub candidate_sets: serde_json::Value,
    pub accepted_evidence: Vec<EvidenceItem>,
    pub rejected_evidence: Vec<EvidenceItem>,
    pub uncertainties: Vec<String>,
    pub answer_draft: String,
    pub next_actions: Vec<String>,
    pub source_fingerprint: Option<String>,
}

/// Typed evidence item with match spans and risk flags
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceItem {
    pub evidence_id: String,
    pub claim_supported: String,
    pub passage_id: String,
    pub quote_raw: String,
    pub quote_normalized: String,
    pub match_span_raw: Vec<usize>,
    pub match_span_normalized: Vec<usize>,
    pub citation: String,
    pub confidence: String,
    pub risk_flags: Vec<String>,
}

#[derive(Clone)]
pub struct GraphDiscoveryServer {
    parquet_path: PathBuf,
    registry_path: PathBuf,
    tfidf_index: PathBuf,
    catalog_index: PathBuf,
    readonly: bool,
    allow_admin_tools: bool,
    store: Arc<OnceCell<DataFusionStore>>,
    tool_router: ToolRouter<Self>,
}

impl GraphDiscoveryServer {
    pub fn new(
        parquet_path: PathBuf,
        tfidf_index: PathBuf,
        registry_path: PathBuf,
        catalog_index: PathBuf,
        readonly: bool,
        allow_admin_tools: bool,
    ) -> Self {
        Self {
            parquet_path,
            registry_path,
            tfidf_index,
            catalog_index,
            readonly,
            allow_admin_tools,
            store: Arc::new(OnceCell::new()),
            tool_router: Self::tool_router(),
        }
    }

    async fn store(&self) -> Result<&DataFusionStore, String> {
        self.store
            .get_or_try_init(|| async { DataFusionStore::open(&self.parquet_path).await })
            .await
            .map_err(|e| format!("open parquet store: {e}"))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Optional phrase to search for in normalized Chinese text.
    #[serde(default)]
    pub phrase: Option<String>,
    #[serde(default)]
    pub tradition: Vec<String>,
    #[serde(default)]
    pub period: Vec<String>,
    #[serde(default)]
    pub origin: Vec<String>,
    #[serde(default)]
    pub canon: Vec<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// Optional catalog index scope: filter to a specific work by source_work_id.
    #[serde(default)]
    pub source_work_id: Option<String>,
    /// Optional catalog index scope: filter to passages within a section by heading_path prefix.
    #[serde(default)]
    pub heading_path_prefix: Option<String>,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}

fn default_search_limit() -> usize {
    20
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PassageParams {
    pub passage_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PhraseParams {
    pub phrase: String,
    #[serde(default = "default_phrase_limit")]
    pub limit: usize,
}

fn default_phrase_limit() -> usize {
    100
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PhraseHistoryParams {
    pub phrase: String,
    #[serde(default)]
    pub include_variants: bool,
    #[serde(default)]
    pub timeline: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PriorWorkParams {
    pub seed_passage_id: String,
    #[serde(default = "default_prior_work_limit")]
    pub limit: usize,
}

fn default_prior_work_limit() -> usize {
    20
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PhraseStatusParams {
    pub phrase: String,
    #[serde(default = "default_prior_work_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkSummaryParams {
    #[serde(default = "default_work_summary_limit")]
    pub limit: usize,
}

fn default_work_summary_limit() -> usize {
    50
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SimilarPassagesParams {
    pub seed: String,
    #[serde(default = "default_similar_limit")]
    pub limit: usize,
    #[serde(default = "default_shared_ngram_limit")]
    pub shared_ngram_limit: usize,
    #[serde(default = "default_shared_phrase_limit")]
    pub shared_phrase_limit: usize,
    #[serde(default = "default_min_shared_phrase_len")]
    pub min_shared_phrase_len: usize,
}

fn default_similar_limit() -> usize {
    20
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FrontierParams {
    pub seed: String,
    #[serde(default = "default_frontier_limit")]
    pub limit: usize,
    #[serde(default = "default_frontier_phrase_limit")]
    pub phrase_limit: usize,
}

fn default_frontier_limit() -> usize {
    20
}

fn default_frontier_phrase_limit() -> usize {
    10
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClassifyQueryParams {
    pub query: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResearchFirstAttestationParams {
    pub query: String,
    #[serde(default = "default_research_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResearchPhraseGenesisParams {
    pub query: String,
    #[serde(default = "default_research_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResolveEntityParams {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

fn default_research_limit() -> usize {
    20
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResearchTemplateParams {
    #[serde(default)]
    pub template_type: Option<String>,
    #[serde(default)]
    pub include_diagrams: bool,
    #[serde(default)]
    pub diagram_types: Vec<String>,
}

// Catalog index parameter structs
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CatalogOverviewParams {
    /// Optional path to catalog index file. If not provided, uses default.
    #[serde(default)]
    pub index_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListWorksParams {
    /// Optional path to catalog index file. If not provided, uses default.
    #[serde(default)]
    pub index_path: Option<String>,
    /// Filter by tradition (e.g., "Buddhist", "Confucian").
    #[serde(default)]
    pub tradition: Option<String>,
    /// Filter by period (e.g., "Han", "Tang").
    #[serde(default)]
    pub period: Option<String>,
    /// Filter by canon (e.g., "Tripitaka", "Sishu").
    #[serde(default)]
    pub canon: Option<String>,
    /// Filter by author name.
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default = "default_catalog_limit")]
    pub limit: usize,
}

fn default_catalog_limit() -> usize {
    100
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkOutlineParams {
    /// Optional path to catalog index file. If not provided, uses default.
    #[serde(default)]
    pub index_path: Option<String>,
    /// Work ID to get outline for. Either work or node must be specified.
    #[serde(default)]
    pub work: Option<String>,
    /// Node ID to get outline for. Either work or node must be specified.
    #[serde(default)]
    pub node: Option<u32>,
    #[serde(default = "default_outline_depth")]
    pub max_depth: usize,
}

fn default_outline_depth() -> usize {
    3
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SectionScopeParams {
    /// Optional path to catalog index file. If not provided, uses default.
    #[serde(default)]
    pub index_path: Option<String>,
    /// Node ID to get scope for.
    pub node_id: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExpandContextParams {
    /// Passage ID to expand context for.
    pub passage_id: String,
    /// Optional hit ID from a previous search result.
    #[serde(default)]
    pub hit_id: Option<String>,
    /// Number of passages to include before the center passage.
    #[serde(default = "default_context_before")]
    pub before: usize,
    /// Number of passages to include after the center passage.
    #[serde(default = "default_context_after")]
    pub after: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExpandHitParams {
    /// Hit ID from a previous search result.
    pub hit_id: String,
    /// Path to the search result packet file.
    pub session_path: String,
    /// Number of passages to include before the center passage.
    #[serde(default = "default_context_before")]
    pub before: usize,
    /// Number of passages to include after the center passage.
    #[serde(default = "default_context_after")]
    pub after: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CefSchemaParams {
    /// Which schema to return: "corpus", "work", "passage", or "all"
    #[serde(default = "default_schema_type")]
    pub schema_type: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CefValidateParams {
    /// Path to the CEF corpus directory
    pub corpus_path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CefGuideParams {
    /// Type of guidance: "overview", "corpus_toml", "works_jsonl", "passages_jsonl", "best_practices"
    pub topic: String,
}

fn default_context_before() -> usize {
    5
}

fn default_context_after() -> usize {
    5
}

fn default_schema_type() -> String {
    "all".to_string()
}

#[tool_router(router = tool_router)]
impl GraphDiscoveryServer {
    #[tool(
        name = "search",
        description = "Full-text search over CBETA + Kanripo passages with optional phrase, tradition, period, origin, canon, author, title, and catalog index scope filters (source_work_id, heading_path_prefix)."
    )]
    pub async fn search_tool(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<Json<Value>, String> {
        let store = self.store().await?;
        search::search(
            store,
            params.phrase,
            params.tradition,
            params.period,
            params.origin,
            params.canon,
            params.author,
            params.title,
            params.source_work_id,
            params.heading_path_prefix,
            params.limit,
        )
        .await
        .map(Json)
        .map_err(|e| format!("search failed: {e}"))
    }

    #[tool(
        name = "passage",
        description = "Fetch a single passage by its passage_id (typically `<rel_path>#<xml_id>`)."
    )]
    pub async fn passage_tool(
        &self,
        Parameters(params): Parameters<PassageParams>,
    ) -> Result<Json<Value>, String> {
        let store = self.store().await?;
        store
            .get_passage(&params.passage_id)
            .await
            .map(Json)
            .map_err(|e| format!("get_passage failed: {e}"))
    }

    #[tool(
        name = "first_attestation",
        description = "Earliest passages containing a phrase, ordered by period_rank then locator."
    )]
    pub async fn first_attestation_tool(
        &self,
        Parameters(params): Parameters<PhraseParams>,
    ) -> Result<Json<Value>, String> {
        let store = self.store().await?;
        first_attestation::first_attestation(params.phrase, store, params.limit, None)
            .await
            .map(Json)
            .map_err(|e| format!("first_attestation failed: {e}"))
    }

    #[tool(
        name = "phrase_history",
        description = "All passages containing a phrase with optional period-bucketed timeline aggregation."
    )]
    pub async fn phrase_history_tool(
        &self,
        Parameters(params): Parameters<PhraseHistoryParams>,
    ) -> Result<Json<Value>, String> {
        let store = self.store().await?;
        phrase_history::phrase_history(
            params.phrase,
            store,
            params.include_variants,
            params.timeline,
            None,
        )
        .await
        .map(Json)
        .map_err(|e| format!("phrase_history failed: {e}"))
    }

    #[tool(
        name = "prior_work",
        description = "Registry-tracked prior work for a seed passage id (uses the local registry SQLite)."
    )]
    pub async fn prior_work_tool(
        &self,
        Parameters(params): Parameters<PriorWorkParams>,
    ) -> Result<Json<Value>, String> {
        let items = registry::prior_work(&self.registry_path, &params.seed_passage_id, params.limit)
            .map_err(|e| format!("prior_work failed: {e}"))?;
        Ok(Json(serde_json::json!({
            "registry": self.registry_path.display().to_string(),
            "seed_passage_id": params.seed_passage_id,
            "items": items,
        })))
    }

    #[tool(
        name = "phrase_status",
        description = "Registry-tracked usage status for a phrase (which works have already cited it)."
    )]
    pub async fn phrase_status_tool(
        &self,
        Parameters(params): Parameters<PhraseStatusParams>,
    ) -> Result<Json<Value>, String> {
        let mut payload =
            registry::phrase_status(&self.registry_path, &params.phrase, params.limit)
                .map_err(|e| format!("phrase_status failed: {e}"))?;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "registry".to_string(),
                serde_json::json!(self.registry_path.display().to_string()),
            );
        }
        Ok(Json(payload))
    }

    #[tool(
        name = "work_summary",
        description = "Recent registry-tracked work summary entries (research runs, generated artifacts)."
    )]
    pub async fn work_summary_tool(
        &self,
        Parameters(params): Parameters<WorkSummaryParams>,
    ) -> Result<Json<Value>, String> {
        let items = registry::work_summary(&self.registry_path, params.limit)
            .map_err(|e| format!("work_summary failed: {e}"))?;
        Ok(Json(serde_json::json!({
            "registry": self.registry_path.display().to_string(),
            "items": items,
        })))
    }

    #[tool(
        name = "get_research_template",
        description = "Get a structured markdown research output template with paragraph sections, citations, and optional diagrams for formatting answers."
    )]
    pub async fn get_research_template_tool(
        &self,
        Parameters(params): Parameters<ResearchTemplateParams>,
    ) -> Result<Json<Value>, String> {
        let template_type = params.template_type.as_deref().unwrap_or("basic");
        let include_diagrams = params.include_diagrams;
        let diagram_types = params.diagram_types;

        let mut template = String::new();

        // Paragraph sections
        template.push_str("## Summary/Answer\n[Direct response to the question]\n\n");
        template.push_str("## Evidence Breakdown\n[Grouped evidence by theme or aspect]\n\n");
        template.push_str("## Analysis\n[Interpretation and synthesis of evidence]\n\n");
        template.push_str("## Conclusion\n[Final assessment and implications]\n\n");

        // Citation section
        template.push_str("## Citations\n- Author, Title [Canon] (locator)\n- Author, Title [Canon] (locator)\n\n");

        // Diagram section (optional)
        if include_diagrams {
            template.push_str("## Diagrams\n");
            if diagram_types.is_empty() {
                template.push_str("[Optional: Add timelines, relationship diagrams, tables, or other visualizations]\n\n");
            } else {
                for dt in &diagram_types {
                    match dt.as_str() {
                        "timeline" => template.push_str("### Timeline\n[Text-based timeline or ASCII diagram]\n\n"),
                        "relationship" => template.push_str("### Relationship Diagram\n[Mermaid diagram or ASCII art]\n\n"),
                        "table" => template.push_str("### Table\n[Markdown table]\n\n"),
                        "lineage" => template.push_str("### Lineage Graph\n[Lineage visualization - future PDF integration]\n\n"),
                        _ => template.push_str(&format!("### {}\n[Diagram placeholder]\n\n", dt)),
                    }
                }
            }
        }

        Ok(Json(serde_json::json!({
            "template_type": template_type,
            "markdown_template": template,
            "include_diagrams": include_diagrams,
            "diagram_types": diagram_types,
        })))
    }

    #[tool(
        name = "similar_passages",
        description = "Find passages textually similar to a seed passage using TF-IDF similarity. Useful for finding related passages even when wording changes."
    )]
    pub async fn similar_passages_tool(
        &self,
        Parameters(params): Parameters<SimilarPassagesParams>,
    ) -> Result<Json<Value>, String> {
        let store = self.store().await?;
        
        // Load DocumentTable for doc_id resolution
        let doc_table_path = self.parquet_path.join("doc_table.bin");
        let doc_table = if doc_table_path.exists() {
            crate::document_table::DocumentTable::load(&doc_table_path)
                .map_err(|e| format!("failed to load DocumentTable: {e}"))?
        } else {
            return Err(format!("DocumentTable not found at {}. Run doc-table-build first.", doc_table_path.display()));
        };
        
        let results = commands_tfidf::similar_passages(
            &store,
            self.tfidf_index.clone(),
            &params.seed,
            params.limit,
            params.shared_ngram_limit,
            params.shared_phrase_limit,
            params.min_shared_phrase_len,
            &doc_table,
        )
        .await
        .map_err(|e| format!("similar_passages failed: {e}"))?;
        Ok(Json(serde_json::json!({
            "seed": params.seed,
            "returned_count": results.len(),
            "limit": params.limit,
            "results": results,
        })))
    }

    #[tool(
        name = "frontier",
        description = "Find frontier passages and phrase frontiers from a seed passage. Combines similar passages with phrase-based exploration to discover related content."
    )]
    pub async fn frontier_tool(
        &self,
        Parameters(params): Parameters<FrontierParams>,
    ) -> Result<Json<Value>, String> {
        let store = self.store().await?;
        
        // Load DocumentTable for doc_id resolution
        let doc_table_path = self.parquet_path.join("doc_table.bin");
        let doc_table = if doc_table_path.exists() {
            crate::document_table::DocumentTable::load(&doc_table_path)
                .map_err(|e| format!("failed to load DocumentTable: {e}"))?
        } else {
            return Err(format!("DocumentTable not found at {}. Run doc-table-build first.", doc_table_path.display()));
        };
        
        let seed_row = store
            .get_passage(&params.seed)
            .await
            .map_err(|e| format!("get_passage failed: {e}"))?;
        let similar = commands_tfidf::similar_passages(
            &store,
            self.tfidf_index.clone(),
            &params.seed,
            params.limit,
            12,
            8,
            4,
            &doc_table,
        )
        .await
        .map_err(|e| format!("similar_passages failed: {e}"))?;

        // Phrase frontiers extraction
        let phrase_frontiers = frontier::phrase_frontiers(&store, &seed_row, params.phrase_limit)
            .await
            .map_err(|e| format!("phrase_frontiers failed: {e}"))?;

        let prior_work = if self.registry_path.exists() {
            registry::prior_work(&self.registry_path, &params.seed, 10)
                .map_err(|e| format!("prior_work failed: {e}"))?
        } else {
            Vec::new()
        };

        Ok(Json(serde_json::json!({
            "schema": "readzen-graphdiscovery-frontier-v1",
            "seed_passage_id": params.seed,
            "seed": seed_row,
            "similar_passages": similar,
            "phrase_frontiers": phrase_frontiers,
            "prior_work": prior_work,
        })))
    }

    #[tool(
        name = "classify_query",
        description = "Classify a research query and return a research plan with recommended tools and approach."
    )]
    pub async fn classify_query_tool(
        &self,
        Parameters(params): Parameters<ClassifyQueryParams>,
    ) -> Result<Json<Value>, String> {
        let query = params.query.to_lowercase();
        let query_type = classify_query_type(&query);
        let entities = extract_entities(&query);
        let recommended_tools = get_recommended_tools(&query_type);
        let required_checks = get_required_checks(&query_type);
        let failure_modes = get_failure_modes(&query_type);

        Ok(Json(serde_json::json!({
            "schema": "sragd-research-plan-v1",
            "query": params.query,
            "query_type": query_type,
            "entities": entities,
            "recommended_tools": recommended_tools,
            "required_checks": required_checks,
            "failure_modes": failure_modes,
        })))
    }

    #[tool(
        name = "research_first_attestation",
        description = "Workflow tool for first attestation research. Internally calls primitives and returns structured research results with evidence classification and confidence assessment."
    )]
    pub async fn research_first_attestation_tool(
        &self,
        Parameters(params): Parameters<ResearchFirstAttestationParams>,
    ) -> Result<Json<Value>, String> {
        let store = self.store().await?;
        let normalized_query = crate::normalize::normalize_zh(&params.query);

        // Build tool trace
        let mut tool_trace = Vec::new();

        // Step 1: Check phrase status
        tool_trace.push("phrase_status".to_string());
        let prior_work = if self.registry_path.exists() {
            registry::phrase_status(&self.registry_path, &params.query, 10)
                .map_err(|e| format!("phrase_status failed: {e}"))?
        } else {
            serde_json::json!([])
        };

        // Step 2: Search for phrase
        tool_trace.push("search".to_string());
        let search_results = search::search(
            store.clone(),
            Some(params.query.clone()),
            vec![],
            vec![],
            vec![],
            vec![],
            None,
            None,
            None,
            None,
            params.limit * 5,
        )
        .await
        .map_err(|e| format!("search failed: {e}"))?;

        // Step 3: First attestation
        tool_trace.push("first_attestation".to_string());
        let first_attestation_results = first_attestation::first_attestation(params.query.clone(), store.clone(), params.limit, None)
            .await
            .map_err(|e| format!("first_attestation failed: {e}"))?;

        // Step 4: Phrase history for context
        tool_trace.push("phrase_history".to_string());
        let phrase_history_results = phrase_history::phrase_history(
            params.query.clone(),
            store.clone(),
            true,
            true,
            None,
        )
        .await
        .map_err(|e| format!("phrase_history failed: {e}"))?;

        // Classify evidence
        let accepted_evidence = if let Some(evidence) = first_attestation_results.get("evidence") {
            if let Some(evidence_array) = evidence.as_array() {
                evidence_array.clone()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let rejected_evidence: Vec<serde_json::Value> = Vec::new();
        let uncertainties = vec![
            "This should be treated as the earliest attestation in the loaded corpus, not proof that it is the absolute historical origin".to_string(),
            "Kanripo passages have uncertain dating and should be treated with caution".to_string(),
        ];

        // Build answer draft
        let answer_draft = if !accepted_evidence.is_empty() {
            let earliest = accepted_evidence.first().and_then(|e| {
                e.get("citation").and_then(|c| c.as_str())
            }).unwrap_or("Unknown");
            format!("The earliest attestation found in the corpus is: {}", earliest)
        } else {
            format!("No attestation found for '{}' in the loaded corpus", params.query)
        };

        Ok(Json(serde_json::json!({
            "schema": "sragd-research-result-v1",
            "query": params.query,
            "normalized_query": normalized_query,
            "classification": "first_attestation",
            "tool_trace": tool_trace,
            "candidate_sets": serde_json::json!({
                "search_results": search_results,
                "first_attestation": first_attestation_results,
                "phrase_history": phrase_history_results
            }),
            "accepted_evidence": accepted_evidence,
            "rejected_evidence": rejected_evidence,
            "uncertainties": uncertainties,
            "answer_draft": answer_draft,
            "next_actions": vec![
                "Review evidence for formulaic phrases that may indicate common expressions rather than unique attestation".to_string(),
                "Check if earliest attestation is from Kanripo and verify dating if possible".to_string(),
                "Consider using similar_passages to find related content that may provide context".to_string()
            ],
            "prior_work": prior_work,
        })))
    }

    #[tool(
        name = "research_phrase_genesis",
        description = "Workflow tool for phrase genesis research (origin of sayings, where they come from). Internally calls primitives and returns structured research results with evidence classification and confidence assessment."
    )]
    pub async fn research_phrase_genesis_tool(
        &self,
        Parameters(params): Parameters<ResearchPhraseGenesisParams>,
    ) -> Result<Json<Value>, String> {
        let store = self.store().await?;
        let normalized_query = crate::normalize::normalize_zh(&params.query);

        // Build tool trace
        let mut tool_trace = Vec::new();

        // Step 1: Search for phrase
        tool_trace.push("search".to_string());
        let search_results = search::search(
            store.clone(),
            Some(params.query.clone()),
            vec![],
            vec![],
            vec![],
            vec![],
            None,
            None,
            None,
            None,
            params.limit * 5,
        )
        .await
        .map_err(|e| format!("search failed: {e}"))?;

        // Step 2: First attestation
        tool_trace.push("first_attestation".to_string());
        let first_attestation_results = first_attestation::first_attestation(params.query.clone(), store.clone(), params.limit, None)
            .await
            .map_err(|e| format!("first_attestation failed: {e}"))?;

        // Step 3: Similar passages for context
        tool_trace.push("similar_passages".to_string());
        
        // Load DocumentTable for doc_id resolution
        let doc_table_path = self.parquet_path.join("doc_table.bin");
        let doc_table = if doc_table_path.exists() {
            crate::document_table::DocumentTable::load(&doc_table_path)
                .map_err(|e| format!("failed to load DocumentTable: {e}"))?
        } else {
            return Err(format!("DocumentTable not found at {}. Run doc-table-build first.", doc_table_path.display()));
        };
        
        let similar_results = if let Some(first_passage_id) = first_attestation_results.get("evidence")
            .and_then(|e| e.as_array())
            .and_then(|arr| arr.first())
            .and_then(|e| e.get("passage_id"))
            .and_then(|id| id.as_str())
        {
            commands_tfidf::similar_passages(
                store.clone(),
                self.tfidf_index.clone(),
                first_passage_id,
                params.limit,
                12,
                8,
                4,
                &doc_table,
            )
            .await
            .map_err(|e| format!("similar_passages failed: {e}"))
        } else {
            Ok(Vec::new())
        };

        let similar_passages_results = match similar_results {
            Ok(results) => results,
            Err(_) => Vec::new(),
        };

        // Step 4: Phrase history for timeline
        tool_trace.push("phrase_history".to_string());
        let phrase_history_results = phrase_history::phrase_history(
            params.query.clone(),
            store.clone(),
            true,
            true,
            None,
        )
        .await
        .map_err(|e| format!("phrase_history failed: {e}"))?;

        // Classify evidence
        let accepted_evidence = if let Some(evidence) = first_attestation_results.get("evidence") {
            if let Some(evidence_array) = evidence.as_array() {
                evidence_array.clone()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let rejected_evidence: Vec<serde_json::Value> = Vec::new();
        let uncertainties = vec![
            "Phrase genesis is complex and may have multiple independent origins".to_string(),
            "Translation artifacts may confuse analysis of original Chinese sources".to_string(),
            "Kanripo passages have uncertain dating and should be treated with caution".to_string(),
            "Formulaic phrases may indicate common expressions rather than unique genesis".to_string(),
        ];

        // Build answer draft
        let answer_draft = if !accepted_evidence.is_empty() {
            let earliest = accepted_evidence.first().and_then(|e| {
                e.get("citation").and_then(|c| c.as_str())
            }).unwrap_or("Unknown");
            let period = accepted_evidence.first().and_then(|e| {
                e.get("period").and_then(|p| p.as_str())
            }).unwrap_or("unknown period");
            format!("The phrase '{}' appears to originate from {} ({}), based on the earliest evidence in the loaded corpus. This should be treated as the earliest attestation in the corpus, not proof of absolute historical origin.", params.query, earliest, period)
        } else {
            format!("No evidence found for the genesis of '{}' in the loaded corpus", params.query)
        };

        Ok(Json(serde_json::json!({
            "schema": "sragd-research-result-v1",
            "query": params.query,
            "normalized_query": normalized_query,
            "classification": "phrase_genesis",
            "tool_trace": tool_trace,
            "candidate_sets": serde_json::json!({
                "search_results": search_results,
                "first_attestation": first_attestation_results,
                "similar_passages": similar_passages_results,
                "phrase_history": phrase_history_results
            }),
            "accepted_evidence": accepted_evidence,
            "rejected_evidence": rejected_evidence,
            "uncertainties": uncertainties,
            "answer_draft": answer_draft,
            "next_actions": vec![
                "Review similar_passages to identify potential textual relationships or borrowing patterns".to_string(),
                "Check if earliest evidence is from Kanripo and verify dating if possible".to_string(),
                "Analyze phrase_history timeline to identify patterns of usage over time".to_string(),
                "Consider whether the phrase may be formulaic or common expression rather than unique genesis".to_string()
            ],
        })))
    }

    #[tool(
        name = "resolve_entity",
        description = "Resolve a person or entity name to find candidates with Chinese aliases. Useful for finding mentions of masters, monks, or other figures with multiple name variants."
    )]
    pub async fn resolve_entity_tool(
        &self,
        Parameters(params): Parameters<ResolveEntityParams>,
    ) -> Result<Json<Value>, String> {
        let store = self.store().await?;

        // Build list of name forms to search
        let mut forms = vec![params.name.clone()];
        for alias in &params.aliases {
            if !forms.iter().any(|v| v == alias) {
                forms.push(alias.clone());
            }
        }

        let mut candidates = Vec::new();
        for form in &forms {
            let spec = crate::research::SearchSpec::exact_phrase(form.clone(), 50);
            let rows = crate::research::exact_phrase_rows(&store, &spec)
                .await
                .map_err(|e| format!("exact_phrase_rows failed: {e}"))?;

            candidates.push(serde_json::json!({
                "form": form,
                "normalized": spec.normalized,
                "hit_count_sample": rows.len(),
                "first_hit": rows.first().cloned().unwrap_or(serde_json::Value::Null),
                "ambiguity": if form.chars().count() <= 1 { "high" } else { "unknown" }
            }));
        }

        Ok(Json(serde_json::json!({
            "schema": "readzen-person-resolve-v1",
            "raw": params.name,
            "aliases": forms.iter().skip(1).cloned().collect::<Vec<_>>(),
            "query_type": "person_resolve",
            "canonical_candidate": forms.first().cloned().unwrap_or_default(),
            "name_forms": candidates,
            "ambiguity_notes": [
                "Short aliases and honorific titles may refer to more than one person.",
                "Use person-history to inspect earliest and contextualized mentions."
            ],
            "caveats": [
                "This is a corpus-local resolver, not a historical authority file.",
                "Aliases must be supplied explicitly until a persons/aliases table is added."
            ],
        })))
    }

    // Catalog index tools
    #[tool(
        name = "catalog_overview",
        description = "Get an overview of the corpus catalog index including total works, nodes, and metadata statistics. This provides a structural map of the corpus for faster navigation and filtering."
    )]
    pub async fn catalog_overview_tool(
        &self,
        Parameters(params): Parameters<CatalogOverviewParams>,
    ) -> Result<Json<Value>, String> {
        let index_path = params.index_path.map(PathBuf::from).unwrap_or_else(|| self.catalog_index.clone());
        let catalog = crate::catalog_index::CorpusCatalogIndex::load(&index_path)
            .map_err(|e| format!("failed to load catalog index: {e}"))?;
        Ok(Json(catalog.info_payload()))
    }

    #[tool(
        name = "list_works",
        description = "List works in the corpus catalog with optional filters for tradition, period, canon, and author. Returns work metadata including title, author, period, and passage counts."
    )]
    pub async fn list_works_tool(
        &self,
        Parameters(params): Parameters<ListWorksParams>,
    ) -> Result<Json<Value>, String> {
        let index_path = params.index_path.map(PathBuf::from).unwrap_or_else(|| self.catalog_index.clone());
        let catalog = crate::catalog_index::CorpusCatalogIndex::load(&index_path)
            .map_err(|e| format!("failed to load catalog index: {e}"))?;

        let mut filtered: Vec<_> = catalog.works.iter().collect();

        if let Some(t) = &params.tradition {
            filtered = filtered.into_iter().filter(|w| w.traditions.iter().any(|tr| tr.as_str() == t.as_str())).collect();
        }

        if let Some(p) = &params.period {
            filtered = filtered.into_iter().filter(|w| w.period.as_str() == p.as_str()).collect();
        }

        if let Some(c) = &params.canon {
            filtered = filtered.into_iter().filter(|w| w.canon.as_str() == c.as_str()).collect();
        }

        if let Some(a) = &params.author {
            filtered = filtered.into_iter().filter(|w| w.author.as_str() == a.as_str()).collect();
        }

        filtered.truncate(params.limit);

        let results: Vec<serde_json::Value> = filtered.iter().map(|w| {
            serde_json::json!({
                "work_id": w.work_id,
                "main_title": w.main_title,
                "author": w.author,
                "period": w.period,
                "canon": w.canon,
                "traditions": w.traditions,
                "passage_count": w.passage_count,
            })
        }).collect();

        Ok(Json(serde_json::json!(results)))
    }

    #[tool(
        name = "work_outline",
        description = "Get the hierarchical outline tree structure for a work or node. Shows the structural organization (sections, headings) of a work up to a specified depth."
    )]
    pub async fn work_outline_tool(
        &self,
        Parameters(params): Parameters<WorkOutlineParams>,
    ) -> Result<Json<Value>, String> {
        let index_path = params.index_path.map(PathBuf::from).unwrap_or_else(|| self.catalog_index.clone());
        let catalog = crate::catalog_index::CorpusCatalogIndex::load(&index_path)
            .map_err(|e| format!("failed to load catalog index: {e}"))?;

        let root_node_id = if let Some(work_id) = &params.work {
            catalog.get_work(work_id)
                .map(|w| w.root_node)
                .ok_or_else(|| "Work not found".to_string())?
        } else if let Some(node_id) = params.node {
            node_id
        } else {
            return Err("Must specify either work or node".to_string());
        };

        let tree = build_outline_tree(&catalog, root_node_id, params.max_depth, 0);
        Ok(Json(tree))
    }

    #[tool(
        name = "section_scope",
        description = "Get the scope and passage filters for a specific node in the catalog. Returns node metadata and filter criteria for querying passages within that section."
    )]
    pub async fn section_scope_tool(
        &self,
        Parameters(params): Parameters<SectionScopeParams>,
    ) -> Result<Json<Value>, String> {
        let index_path = params.index_path.map(PathBuf::from).unwrap_or_else(|| self.catalog_index.clone());
        let catalog = crate::catalog_index::CorpusCatalogIndex::load(&index_path)
            .map_err(|e| format!("failed to load catalog index: {e}"))?;

        let node = catalog.get_node(params.node_id)
            .ok_or_else(|| "Node not found".to_string())?;

        let scope = serde_json::json!({
            "node_id": node.node_id,
            "work_id": node.work_id,
            "passage_count": node.passage_count,
            "first_doc_id": node.first_doc_id,
            "last_doc_id": node.last_doc_id,
            "filter": {
                "source_work_id": node.work_id,
                "heading_path_prefix": node.heading_path,
            }
        });

        Ok(Json(scope))
    }

    #[tool(
        name = "expand_context",
        description = "Expand context around a passage ID. Returns the center passage plus specified number of passages before and after."
    )]
    pub async fn expand_context_tool(
        &self,
        Parameters(params): Parameters<ExpandContextParams>,
    ) -> Result<Json<Value>, String> {
        let store = self.store().await?;
        let context = expand_context::run(
            self.parquet_path.clone(),
            Some(params.passage_id.clone()),
            None,
            None,
            params.before,
            params.after,
            None,
        )
        .await
        .map_err(|e| format!("expand_context failed: {e}"))?;
        Ok(Json(serde_json::to_value(context).map_err(|e| format!("failed to serialize context: {e}"))?))
    }

    #[tool(
        name = "expand_hit",
        description = "Expand context for a hit ID from a previous search result packet. Loads the packet, resolves the hit to a passage ID, and expands context."
    )]
    pub async fn expand_hit_tool(
        &self,
        Parameters(params): Parameters<ExpandHitParams>,
    ) -> Result<Json<Value>, String> {
        let session_path = PathBuf::from(&params.session_path);
        let packet = SearchResultPacket::load(&session_path)
            .map_err(|e| format!("failed to load search packet: {e}"))?;
        
        let hit = packet.find_hit(&params.hit_id)
            .map_err(|e| format!("failed to find hit: {e}"))?;
        
        let store = self.store().await?;
        let context = expand_context::run(
            self.parquet_path.clone(),
            None,
            Some(session_path.clone()),
            Some(params.hit_id.clone()),
            params.before,
            params.after,
            None,
        )
        .await
        .map_err(|e| format!("expand_context failed: {e}"))?;
        
        Ok(Json(serde_json::to_value(context).map_err(|e| format!("failed to serialize context: {e}"))?))
    }

    #[tool(
        name = "cef_schema",
        description = "Get schema information for the GraphDiscovery Corpus Exchange Format (GD-CEF). Returns field definitions, required fields, and examples."
    )]
    pub async fn cef_schema_tool(
        &self,
        Parameters(params): Parameters<CefSchemaParams>,
    ) -> Result<Json<Value>, String> {
        let schema_type = params.schema_type.as_str();
        
        let result = match schema_type {
            "corpus" => json!({
                "schema": "gd-cef-schema-v1",
                "type": "corpus_toml",
                "description": "Corpus-level metadata file (corpus.toml)",
                "required_fields": ["schema", "corpus_id", "name", "language", "snapshot_id", "rights_id"],
                "optional_fields": [
                    "script", "description", "source_url", "source_type", "rights_notes",
                    "default_period", "default_period_rank", "default_origin", "default_traditions",
                    "conversion.converter_name", "conversion.converter_version", "conversion.conversion_date", "conversion.notes"
                ],
                "example": r#"schema = "gd-cef-v1"
corpus_id = "my-corpus"
name = "My Corpus"
language = "zh-Hant"
snapshot_id = "2026-05-09"
rights_id = "CC-BY-SA-4.0""#
            }),
            "work" => json!({
                "schema": "gd-cef-schema-v1",
                "type": "works_jsonl",
                "description": "One row per work/text/book in works.jsonl",
                "required_fields": ["work_id", "title_zh"],
                "optional_fields": [
                    "title_en", "author", "dynasty", "period", "period_rank",
                    "date_start", "date_end", "date_certainty", "traditions", "genre",
                    "source_rel_path", "source_url", "rights_id", "quality_flags"
                ],
                "example": r#"{"work_id":"work-0001","title_zh":"論語","title_en":"Analects","author":"孔子","period":"Pre-Qin","period_rank":0}"#
            }),
            "passage" => json!({
                "schema": "gd-cef-schema-v1",
                "type": "passages_jsonl",
                "description": "One row per searchable passage in passages.jsonl",
                "required_fields": ["passage_id", "work_id", "text"],
                "optional_fields": [
                    "rights_id", "section_id", "section_title", "locator", "source_rel_path",
                    "source_url", "text_normalized", "text_type", "heading_path",
                    "line_start", "line_end", "contains_person", "contains_term", "quality_flags"
                ],
                "example": r#"{"passage_id":"work-0001#p000001","work_id":"work-0001","text":"子曰學而時習之不亦說乎"}"#
            }),
            "all" | _ => json!({
                "schema": "gd-cef-schema-v1",
                "types": ["corpus_toml", "works_jsonl", "passages_jsonl"],
                "description": "GraphDiscovery Corpus Exchange Format v1",
                "required_files": ["corpus.toml", "works.jsonl", "passages.jsonl"],
                "minimum_viable": {
                    "corpus_toml": ["schema", "corpus_id", "name", "language", "snapshot_id", "rights_id"],
                    "works_jsonl": ["work_id", "title_zh"],
                    "passages_jsonl": ["passage_id", "work_id", "text"]
                }
            })
        };
        
        Ok(Json(result))
    }

    #[tool(
        name = "cef_validate",
        description = "Validate a GD-CEF corpus directory. Checks required files, field presence, data consistency, and CJK content."
    )]
    pub async fn cef_validate_tool(
        &self,
        Parameters(params): Parameters<CefValidateParams>,
    ) -> Result<Json<Value>, String> {
        let corpus_path = PathBuf::from(&params.corpus_path);
        let report = cef::validate(corpus_path)
            .map_err(|e| format!("validation failed: {e}"))?;
        Ok(Json(serde_json::to_value(report).map_err(|e| format!("failed to serialize report: {e}"))?))
    }

    #[tool(
        name = "cef_guide",
        description = "Get guidance for converting datasets to GD-CEF format. Provides step-by-step instructions, best practices, and examples for different conversion topics."
    )]
    pub async fn cef_guide_tool(
        &self,
        Parameters(params): Parameters<CefGuideParams>,
    ) -> Result<Json<Value>, String> {
        let topic = params.topic.as_str();
        
        let guidance = match topic {
            "overview" => json!({
                "schema": "gd-cef-guide-v1",
                "topic": "overview",
                "steps": [
                    "1. Create a directory for your corpus",
                    "2. Create corpus.toml with corpus metadata",
                    "3. Create works.jsonl with one row per work/book",
                    "4. Create passages.jsonl with one row per searchable passage",
                    "5. Run validation: graphdiscovery cef-validate --input /path/to/corpus",
                    "6. Ingest: graphdiscovery ingest-cef --input /path/to/corpus --out-parquet /path/to/passages.parquet"
                ],
                "minimum_viable": {
                    "corpus.toml": "schema, corpus_id, name, language, snapshot_id, rights_id",
                    "works.jsonl": "work_id, title_zh",
                    "passages.jsonl": "passage_id, work_id, text"
                },
                "best_practices": [
                    "Passage length: 50-800 CJK characters recommended",
                    "Use consistent work_id references across works.jsonl and passages.jsonl",
                    "Include period_rank for first-attestation queries",
                    "Set quality_flags to indicate data confidence"
                ]
            }),
            "corpus_toml" => json!({
                "schema": "gd-cef-guide-v1",
                "topic": "corpus_toml",
                "description": "Corpus-level metadata in TOML format",
                "required_fields": {
                    "schema": "Must be 'gd-cef-v1'",
                    "corpus_id": "Unique identifier for your corpus (e.g., 'my-corpus')",
                    "name": "Human-readable name (e.g., 'My Chinese Text Corpus')",
                    "language": "Language code (e.g., 'zh-Hant', 'zh-Hans')",
                    "snapshot_id": "Version/date identifier (e.g., '2026-05-09')",
                    "rights_id": "Rights identifier (e.g., 'CC-BY-SA-4.0', 'public-domain')"
                },
                "example": r#"schema = "gd-cef-v1"
corpus_id = "my-corpus"
name = "My Corpus"
language = "zh-Hant"
script = "traditional"
snapshot_id = "2026-05-09"
description = "Description of corpus"
source_url = "https://example.com"
rights_id = "CC-BY-SA-4.0"
rights_notes = "Add rights information here"

[conversion]
converter_name = "my-converter"
converter_version = "1.0.0"
conversion_date = "2026-05-09"
notes = "Conversion notes"#
            }),
            "works_jsonl" => json!({
                "schema": "gd-cef-guide-v1",
                "topic": "works_jsonl",
                "description": "One JSONL row per work/book",
                "required_fields": {
                    "work_id": "Unique identifier for the work (e.g., 'work-0001')",
                    "title_zh": "Chinese title of the work"
                },
                "recommended_fields": {
                    "title_en": "English title",
                    "author": "Author name",
                    "period": "Historical period (e.g., 'Tang', 'Pre-Qin')",
                    "period_rank": "Numeric period rank for sorting (0 = earliest)",
                    "date_start": "Year range start (BCE as negative)",
                    "date_end": "Year range end",
                    "traditions": "Array of traditions (e.g., ['Confucian', 'Buddhist'])"
                },
                "example": r#"{"work_id":"work-0001","title_zh":"論語","title_en":"Analects","author":"孔子","period":"Pre-Qin","period_rank":0,"date_start":-475,"date_end":-221,"traditions":["Confucian"]}"#
            }),
            "passages_jsonl" => json!({
                "schema": "gd-cef-guide-v1",
                "topic": "passages_jsonl",
                "description": "One JSONL row per searchable passage",
                "required_fields": {
                    "passage_id": "Unique identifier (e.g., 'work-0001#p000001')",
                    "work_id": "Reference to work_id from works.jsonl",
                    "text": "The passage text content"
                },
                "recommended_fields": {
                    "section_id": "Section/chapter identifier",
                    "section_title": "Section/chapter title",
                    "text_type": "Type: 'prose', 'verse', 'dialogue'",
                    "line_start": "Starting line number",
                    "line_end": "Ending line number",
                    "heading_path": "Hierarchical heading path"
                },
                "passage_length_guidelines": {
                    "recommended": "50-800 CJK characters",
                    "minimum": "8 CJK characters",
                    "maximum": "3000 CJK characters",
                    "exceptions": "Verse lines may be shorter; long prose may exceed 3000"
                },
                "example": r#"{"passage_id":"work-0001#p000001","work_id":"work-0001","section_id":"001","section_title":"學而","text":"子曰學而時習之不亦說乎","text_type":"prose"}"#
            }),
            "best_practices" => json!({
                "schema": "gd-cef-guide-v1",
                "topic": "best_practices",
                "passage_segmentation": [
                    "Aim for 50-800 CJK characters per passage",
                    "Preserve context for meaningful search results",
                    "Use natural boundaries (paragraphs, verse lines)",
                    "Avoid breaking in the middle of sentences"
                ],
                "dating": [
                    "Use negative years for BCE (e.g., -475 for 475 BCE)",
                    "Include period_rank for first-attestation sorting",
                    "Set date_certainty: 'exact', 'approximate', 'dynasty', 'century', 'unknown'",
                    "If only dynasty is known, set period_rank and omit exact dates"
                ],
                "traditions": [
                    "Use consistent labels: 'Buddhist', 'Confucian', 'Daoist', 'Historical', etc.",
                    "Multiple traditions allowed as array",
                    "Empty array if no clear tradition"
                ],
                "quality_flags": [
                    "Set 'synthetic_passage_id' if auto-generated",
                    "Set 'synthetic_segmentation' if paragraph boundaries are approximate",
                    "Set 'metadata_inferred_from_filename' if metadata derived from paths",
                    "Set 'needs_review' for uncertain data"
                ],
                "validation": [
                    "Always run cef-validate before ingest",
                    "Check that all passage work_ids exist in works.jsonl",
                    "Ensure passage_id values are unique",
                    "Verify CJK character presence in text fields"
                ]
            }),
            _ => json!({
                "error": format!("Unknown topic: {}. Available topics: overview, corpus_toml, works_jsonl, passages_jsonl, best_practices", topic)
            })
        };
        
        Ok(Json(guidance))
    }
}

fn build_outline_tree(
    catalog: &crate::catalog_index::CorpusCatalogIndex,
    node_id: u32,
    max_depth: usize,
    current_depth: usize,
) -> serde_json::Value {
    let node = match catalog.get_node(node_id) {
        Some(n) => n,
        None => return serde_json::json!(null),
    };

    let children = if current_depth < max_depth {
        node.children.iter().map(|&child_id| build_outline_tree(catalog, child_id, max_depth, current_depth + 1)).collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    serde_json::json!({
        "node_id": node.node_id,
        "node_kind": format!("{:?}", node.node_kind),
        "label": node.label,
        "passage_count": node.passage_count,
        "children": children,
    })
}

fn classify_query_type(query: &str) -> String {
    let q = query.to_lowercase();
    if q.contains("first") || q.contains("earliest") || q.contains("when") {
        if q.contains("mention") || q.contains("appear") {
            "first_attestation".to_string()
        } else if q.contains("person") || q.contains("master") || q.contains("monk") {
            "person_history".to_string()
        } else {
            "first_attestation".to_string()
        }
    } else if q.contains("origin") || q.contains("genesis") || q.contains("come from") || q.contains("where did") {
        "phrase_genesis".to_string()
    } else if q.contains("sutra") || q.contains("canon") || q.contains("from which") {
        "canonical_source".to_string()
    } else if q.contains("compare") || q.contains("disagreement") || q.contains("difference") {
        "doctrinal_comparison".to_string()
    } else if q.contains("reuse") || q.contains("similar") || q.contains("related") {
        "text_reuse".to_string()
    } else if q.contains("timeline") || q.contains("history over time") || q.contains("spread") {
        "timeline_analysis".to_string()
    } else {
        "unknown".to_string()
    }
}

fn extract_entities(query: &str) -> Vec<serde_json::Value> {
    let mut entities = Vec::new();
    // Simple heuristic extraction - in production this would use NLP
    if query.contains("phrase") || query.contains("saying") || query.contains("word") {
        entities.push(serde_json::json!({
            "kind": "phrase",
            "surface": query,
            "normalized": query.to_lowercase()
        }));
    }
    if query.contains("master") || query.contains("monk") || query.contains("person") {
        entities.push(serde_json::json!({
            "kind": "person",
            "surface": query,
            "aliases": []
        }));
    }
    entities
}

fn get_recommended_tools(query_type: &str) -> Vec<String> {
    match query_type {
        "first_attestation" => vec!["search".to_string(), "first_attestation".to_string(), "phrase_history".to_string()],
        "phrase_genesis" => vec!["search".to_string(), "first_attestation".to_string(), "similar_passages".to_string()],
        "person_history" => vec!["search".to_string(), "phrase_history".to_string(), "frontier".to_string()],
        "canonical_source" => vec!["search".to_string(), "passage".to_string()],
        "doctrinal_comparison" => vec!["search".to_string(), "similar_passages".to_string(), "frontier".to_string()],
        "text_reuse" => vec!["similar_passages".to_string(), "frontier".to_string(), "search".to_string()],
        "timeline_analysis" => vec!["phrase_history".to_string(), "search".to_string()],
        _ => vec!["search".to_string()],
    }
}

fn get_required_checks(query_type: &str) -> Vec<String> {
    match query_type {
        "first_attestation" => vec![
            "Check if phrase exists in corpus".to_string(),
            "Verify period_rank ordering".to_string(),
            "Check for formulaic phrases".to_string(),
        ],
        "phrase_genesis" => vec![
            "Check if phrase is formulaic".to_string(),
            "Verify multiple source paths".to_string(),
            "Check period distribution".to_string(),
        ],
        _ => vec!["Check if query is within corpus scope".to_string()],
    }
}

fn get_failure_modes(query_type: &str) -> Vec<String> {
    match query_type {
        "first_attestation" => vec![
            "Phrase may be formulaic or common".to_string(),
            "Corpus may not contain earliest historical source".to_string(),
            "Variant spellings may be missed".to_string(),
        ],
        "phrase_genesis" => vec![
            "Phrase may have multiple independent origins".to_string(),
            "Translation artifacts may confuse analysis".to_string(),
            "Kanripo dating may be uncertain".to_string(),
        ],
        _ => vec!["Query may be out of corpus scope".to_string()],
    }
}

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for GraphDiscoveryServer {
    fn get_info(&self) -> ServerInfo {
        let mut instructions = get_mcp_instructions();
        if self.readonly {
            instructions.push_str("\n\n**NOTE**: This server is running in READ-ONLY mode.");
        }
        if self.allow_admin_tools {
            instructions.push_str("\n\n**NOTE**: Admin tools are enabled (currently no admin tools are implemented).");
        }
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(instructions)
    }
}

pub fn run(
    transport: String,
    parquet: PathBuf,
    tfidf_index: PathBuf,
    catalog_index: PathBuf,
    registry: Option<PathBuf>,
    readonly: bool,
    allow_admin_tools: bool,
) -> Result<()> {
    if transport != "stdio" {
        anyhow::bail!("Only stdio transport is currently supported");
    }
    let registry_path = registry.unwrap_or_else(|| {
        parquet
            .parent()
            .map(|p| p.join("registry.sqlite"))
            .unwrap_or_else(|| PathBuf::from("registry.sqlite"))
    });

    // Validate TF-IDF index fingerprint if it exists
    if tfidf_index.exists() {
        let index = TfidfIndex::load(&tfidf_index)?;
        let index_fingerprint = index.doc_table_fingerprint().to_string();
        if !index_fingerprint.is_empty() {
            let handle = tokio::runtime::Handle::current();
            let store = handle.block_on(DataFusionStore::open(&parquet))?;
            let current_fingerprint = store.source_fingerprint();
            let current_fingerprint_str = current_fingerprint
                .get("fingerprint")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            if index_fingerprint != current_fingerprint_str {
                eprintln!("WARNING: TF-IDF index fingerprint mismatch!");
                eprintln!("  Index fingerprint: {}", index_fingerprint);
                eprintln!("  Current fingerprint: {}", current_fingerprint_str);
                eprintln!("  The TF-IDF index may be stale. Rebuild it with: tfidf build");
                if !readonly {
                    eprintln!("  Server will continue but results may be inconsistent.");
                } else {
                    anyhow::bail!("TF-IDF index fingerprint mismatch in readonly mode. Refusing to start.");
                }
            }
        }
    }

    let server = GraphDiscoveryServer::new(
        parquet,
        tfidf_index,
        registry_path,
        catalog_index,
        readonly,
        allow_admin_tools,
    );

    // The outer `commands::run` entry point is already inside a tokio runtime
    // (via `#[tokio::main]` in main.rs), so we use `Handle::block_on` to drive
    // the MCP server to completion on the existing runtime.
    let handle = tokio::runtime::Handle::current();
    handle.block_on(async move {
        let service = server
            .serve(stdio())
            .await
            .map_err(|e| anyhow::anyhow!("mcp serve failed: {e}"))?;
        service
            .waiting()
            .await
            .map_err(|e| anyhow::anyhow!("mcp server exited with error: {e}"))?;
        Ok::<_, anyhow::Error>(())
    })
}

/// Returns comprehensive instructions for LLM agents using the SinoRAGD MCP server.
///
/// This function consolidates the GraphDiscovery documentation (toolplan.md,
/// FRONTIER_AGENT.md, GRAPH_RESEARCH_LENSES.md, WORKFLOW.md, etc.) into
/// instructions that guide agents on how to use the available tools for
/// Buddhist textual research.
fn get_mcp_instructions() -> String {
    r#"# SinoRAGD MCP Server Instructions

You are a research agent for Buddhist textual relationships using the SinoRAGD (formerly GraphDiscovery) toolset. The available corpus is the Chinese Buddhist Text Translation (CBETA) collection plus Kanripo classical Chinese texts.

## IMPORTANT: Scope Check

Before processing any query, check if it falls within available resources.

### In-scope queries (can be handled with current tools):
- Phrase search and first-attestation: "When was the first mention of X?"
- Person/master mentions: "When does Mazu first appear?"
- Phrase genealogy with variants: "Where did this saying come from?"
- Canonical source identification: "Is this from a sutra?"
- Timeline analysis: "Trace the spread of a phrase"
- Evidence adjudication: Validate claims with corpus evidence
- Registry queries: Check prior work, phrase status, work summary
- Passage retrieval: Fetch specific passages by ID
- TF-IDF similarity: Find lexically similar passages

### Out-of-scope queries (should pass on with clear explanation):
- Non-Buddhist corpora: "Find this in Confucian texts" (only CBETA loaded)
- Sanskrit/Pali sources: "Find the Pali original" (only Chinese CBETA loaded)
- Translation work: "Translate this passage" (not a translation tool)
- General knowledge: "Who is the Buddha?" (use general knowledge, not corpus search)
- Modern scholarship: "What do modern scholars say about..." (corpus is historical texts)
- Text generation: "Write a new gatha in the style of..." (not a generative tool)
- External data: "Search the internet for..." (corpus is local only)
- Linguistic analysis beyond search: "Analyze the grammar of..." (limited to search/matching)

Response for out-of-scope queries:
"This request is outside the scope of the current SinoRAGD corpus and tools. The available corpus is the Chinese Buddhist Text Translation (CBETA) collection. Available tools support phrase search, first-attestation, person mentions, canonical sources, and evidence validation within this corpus. For requests outside this scope, please use appropriate external resources or specify a different corpus adapter."

## General Query Handling

When the user asks a vague or general question, decompose it using this classifier:

### Query types:
1. **First-attestation**: "When was X first mentioned?", "Earliest use of X"
2. **Person history**: "When did Mazu appear?", "First mention of X"
3. **Phrase genesis**: "Where did X come from?", "Origin of saying X"
4. **Canonical source**: "Is this from a sutra?", "What sutra is X from?"
5. **Timeline**: "Trace the spread of X", "History of X over time"
6. **Evidence validation**: "Is this claim supported?", "Verify X"
7. **General knowledge**: "Who is X?", "What is X?" (pass to general knowledge)
8. **Out-of-scope**: Non-Buddhist, translation, generation, etc. (pass with explanation)

### Classification heuristics:
- Keywords "first", "earliest", "when" + phrase/person → first-attestation or person history
- Keywords "where did X come from", "origin", "genesis" → phrase genesis or canonical source
- Keywords "master", "monk", named person → person history
- Keywords "sutra", "canon", "from which sutra" → canonical source
- Keywords "timeline", "history over time", "spread" → timeline analysis
- Keywords "verify", "is this true", "evidence" → evidence validation
- Proper noun without context → check if it's a known person/phrase in corpus, else general knowledge
- Request for translation/generation/external search → out-of-scope

### If classification is uncertain:
1. Check if the query target exists in the corpus using `search --phrase <target> --limit 5`
2. If results found, proceed with corpus research (first-attestation or phrase-history)
3. If no results found, inform the user and suggest:
   - The phrase/person may not be in the loaded corpus
   - Try variant spellings or Chinese names
   - Check if the query is actually a general knowledge question
   - Verify the query is within scope (Buddhist Chinese corpus)

## Available Tools

### Catalog Index Tools

### catalog_overview
Get an overview of the corpus catalog index including total works, nodes, and metadata statistics.
- Use this first to understand the corpus structure and available works
- Provides a structural map of the corpus for faster navigation and filtering
- Parameters: index_path (optional, uses default if not provided)

### list_works
List works in the corpus catalog with optional filters for tradition, period, canon, and author.
- Use to find works by metadata (tradition, period, canon, author)
- Returns work metadata including title, author, period, and passage counts
- Parameters: index_path (optional), tradition (optional), period (optional), canon (optional), author (optional), limit (default 100)

### work_outline
Get the hierarchical outline tree structure for a work or node.
- Use to explore the structural organization (sections, headings) of a work
- Shows the tree structure up to a specified depth
- Parameters: index_path (optional), work (optional, work_id), node (optional, node_id), max_depth (default 3)

### section_scope
Get the scope and passage filters for a specific node in the catalog.
- Use to get filter criteria for querying passages within a section
- Returns node metadata and filter criteria for targeted searches
- Parameters: index_path (optional), node_id (required)

### Search and Retrieval Tools

### search
Full-text search over CBETA + Kanripo passages with optional phrase, tradition, period, origin, canon, author, and title filters.
- Use for finding passages containing specific phrases or matching metadata criteria
- Parameters: phrase (optional), tradition, period, origin, canon, author, title, limit (default 20)

### passage
Fetch a single passage by its passage_id (typically `<rel_path>#<xml_id>`).
- Use for retrieving full passage text and metadata when you have a passage ID
- Parameters: passage_id (required)

### first_attestation
Earliest passages containing a phrase, ordered by period_rank then locator.
- Use for finding the earliest mentions of a phrase in the corpus
- Parameters: phrase (required), limit (default 100)

### phrase_history
All passages containing a phrase with optional period-bucketed timeline aggregation.
- Use for comprehensive phrase analysis across time periods
- Parameters: phrase (required), include_variants (default false), timeline (default false)

### prior_work
Registry-tracked prior work for a seed passage id (uses the local registry SQLite).
- Use to avoid redoing completed work on a passage
- Parameters: seed_passage_id (required), limit (default 20)

### phrase_status
Registry-tracked usage status for a phrase (which works have already cited it).
- Use to check if a phrase has been researched before
- Parameters: phrase (required), limit (default 20)

### work_summary
Recent registry-tracked work summary entries (research runs, generated artifacts).
- Use to see recent research activity
- Parameters: limit (default 50)

### get_research_template
Get a structured markdown research output template with paragraph sections, citations, and optional diagrams for formatting answers.
- Use to optionally structure your research output in a consistent format
- Parameters: template_type (optional, default "basic"), include_diagrams (optional, default false), diagram_types (optional vector)
- Returns a markdown template with sections for Summary/Answer, Evidence Breakdown, Analysis, Conclusion, Citations, and optional Diagrams
- This tool is optional - you can format your output however you prefer

### similar_passages
Find passages textually similar to a seed passage using TF-IDF similarity.
- Use for finding related passages even when wording changes
- Parameters: seed (required), limit (default 20), shared_ngram_limit (default 12), shared_phrase_limit (default 8), min_shared_phrase_len (default 4)
- Returns similar passages with TF-IDF similarity scores and shared phrases

### frontier
Find frontier passages and phrase frontiers from a seed passage.
- Use for discovering related content through both similarity and phrase-based exploration
- Parameters: seed (required), limit (default 20), phrase_limit (default 10)
- Returns similar passages, phrase frontiers, and prior work

### classify_query
Classify a research query and return a research plan with recommended tools and approach.
- Use to determine query type and get recommended tools before starting research
- Parameters: query (required)
- Returns query classification, entities, recommended tools, required checks, and failure modes

### research_first_attestation
Workflow tool for first attestation research that internally calls primitives and returns structured research results.
- Use for comprehensive first attestation research with evidence classification and confidence assessment
- Parameters: query (required), limit (default 20)
- Returns structured research result with schema, evidence classification, tool trace, and answer draft

### research_phrase_genesis
Workflow tool for phrase genesis research (origin of sayings, where they come from).
- Use for comprehensive phrase genesis research with evidence classification and confidence assessment
- Parameters: query (required), limit (default 20)
- Returns structured research result with schema, evidence classification, tool trace, and answer draft

## Tool Plans

### Tool Plan 1: Exact phrase first-attestation
Use when the user asks: "When was the first mention of X?", "Where does X first appear?"

Steps:
1. Use `phrase_status` to check for prior work
2. Use `search` with the phrase (limit 200)
3. Use `first_attestation` for the phrase (limit 200)
4. Use `phrase_history` with `include_variants=true` for comprehensive analysis
5. Sort hits by period_rank ASC, then source_rel_path ASC, from_lb ASC
6. Select the earliest exact hit
7. Extract a short quote around the phrase
8. Count distribution by period/canon/tradition
9. Answer with caveat: "This should be treated as the earliest attestation in the loaded corpus, not proof that it is the absolute historical origin."

### Tool Plan 2: First mention of a master/person
Use when the user asks: "When was the first mention of Mazu?", "When does 趙州 first appear?"

Steps:
1. Resolve the name to Chinese canonical forms and aliases
2. Use `search` with primary name (limit 300)
3. Use `search` for aliases separately if available
4. Deduplicate by passage_id
5. Classify each hit: name mention, lineage relation, attributed saying, case appearance, biographical retelling, commentarial reference
6. Prefer earliest high-confidence hit over earliest ambiguous alias hit
7. Report earliest unambiguous mention
8. Report ambiguous earlier hits separately

### Tool Plan 3: Phrase genesis with variants
Use when the user asks: "Where did X come from?", "What is the genesis of X?"

Steps:
1. Use `phrase_history` with `include_variants=true` and `timeline=true`
2. Distinguish: exact phrase, partial phrase, variant phrase, same story/case, same motif but no wording overlap
3. Variant generation rules: remove punctuation, try shorter anchors (4-10 char distinctive substrings), do not invent doctrinal paraphrases

### Tool Plan 4: Canonical source of a Buddhist saying
Use when the user asks: "Where does X come from?", "Is X originally from the Nirvana Sutra?"

Steps:
1. Use `search` with `canon=T` filter to find canonical-side sources
2. Use `phrase_history` to see phrase distribution
3. Hard evidence rule: Every accepted canonical source claim needs two-sided evidence (phrase-side hit + canon-side source passage + exact Chinese quote from both)
4. Do not assert a sutra source from memory. If canon-side passage cannot be found through search, the edge must be rejected

## Hard Rules

- Treat all findings as "earliest in the loaded corpus," not absolute historical origin
- Use corpus evidence only. Do not assert origins from memory
- Preserve exact Chinese quotes for every accepted claim
- Distinguish exact phrase hits from variants, partials, aliases, and broad motif parallels
- Prefer earlier metadata periods, but disclose when dating is coarse or unknown
- If a phrase is short, generic, or doctrinally common, lower confidence and recommend variant/collocation follow-up
- Every accepted canonical source claim needs two-sided exact Chinese evidence
- Do not assert a sutra source from memory
- A bare citation marker (經云 alone) is not evidence
- Prior knowledge may suggest searches, but it cannot justify an edge

## Research Lenses

When conducting research, consider these lenses (from GRAPH_RESEARCH_LENSES.md):

- **document-context**: What helps a reader understand this seed passage's corpus neighborhood?
- **case-genealogy**: How does a case, episode, or encounter mutate across texts?
- **master-reception**: Who matters, where, when, and in what role?
- **canonical-dependence**: What comes from Indic scripture, sutra, sastra, or translation vocabulary?
- **cultural-uptake**: What non-Buddhist Chinese material is reused or repurposed?
- **sectarian-positioning**: How do traditions frame, absorb, oppose, or ignore each other?
- **vocabulary-history**: How do terms stabilize, shift, or move between contexts?
- **transmission**: How do texts, people, stories, and motifs move across canons, regions, or eras?
- **data-reliability**: Which claims need source-quality caution?

Before accepting any edge, ask: "Does this edge answer the active lens question?"

## Workflow

For any research task:
1. Check `work_summary` to see recent activity
2. Check `prior_work` for the seed passage to avoid redoing work
3. Check `phrase_status` for the target phrase
4. Use `search` for initial discovery
5. Use `first_attestation` for earliest mentions
6. Use `phrase_history` for comprehensive analysis
7. Use `passage` to retrieve full text of specific passages
8. Preserve exact Chinese quotes for all evidence
9. Report caveats and limitations clearly

## Concurrency Notes

This server supports parallel tool calls within a single instance. The registry uses SQLite with WAL mode enabled for better concurrent read access.

- **Batch independent queries** (e.g., multiple phrase searches) when possible for better performance
- **Avoid daisy-chaining more than 3-4 sequential tool calls** if you can batch them instead
- **If you encounter timeouts**, retry with exponential backoff - the registry has a 5-second busy timeout
- **Single server instance only**: Running multiple server instances against the same registry file is not supported

The DataFusion backend (for search/passage queries) handles concurrent queries natively. The registry (for prior_work/phrase_status/work_summary) uses SQLite with WAL mode to allow concurrent readers.
"#.to_string()
}
