use anyhow::Result;
use serde_json::Value;

use crate::tools::engine::ToolEngine;
use crate::tools::errors::{classify_tool_error, ToolError};
use crate::tools::requests::*;
use crate::tools::responses::*;
use crate::tools::spec::{schema_for, ToolAudience, ToolDef, ToolExample, ToolSafety, ToolSpec};

/// Standard response envelope for tool calls
#[derive(Debug, serde::Serialize)]
pub struct ToolCallEnvelope {
    pub id: Option<String>,
    pub ok: bool,
    pub tool: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<crate::tools::errors::ToolErrorBody>,

    pub meta: ToolCallMeta,
}

#[derive(Debug, serde::Serialize)]
pub struct ToolCallMeta {
    pub elapsed_ms: u128,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_utc: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_utc: Option<String>,
}

/// Enforce safety rules for a tool
fn enforce_safety(engine: &ToolEngine, spec: &ToolSpec) -> Result<()> {
    match spec.safety {
        ToolSafety::ReadOnly => Ok(()),

        ToolSafety::WritesOutput => {
            if engine.config.readonly {
                return Err(ToolError::ReadonlyViolation {
                    tool: spec.name.to_string(),
                }
                .into_anyhow());
            }
            Ok(())
        }

        ToolSafety::MutatesRegistry | ToolSafety::Admin => {
            if !engine.config.allow_admin_tools {
                return Err(ToolError::AdminToolDisabled {
                    tool: spec.name.to_string(),
                }
                .into_anyhow());
            }
            Ok(())
        }
    }
}

/// Call a tool by name with JSON arguments
pub async fn call_tool(engine: &ToolEngine, name: &str, args: Value) -> Result<Value> {
    let defs = tool_defs();

    let def = defs
        .iter()
        .find(|d| d.spec.name == name)
        .ok_or_else(|| ToolError::unknown_tool(name).into_anyhow())?;

    enforce_safety(engine, &def.spec)?;

    let mut result = (def.call)(engine, args).await?;
    crate::dict::annotate_response(&mut result).await;
    Ok(result)
}

/// Call a tool with envelope response
pub async fn call_tool_enveloped(
    engine: &ToolEngine,
    id: Option<String>,
    tool: String,
    args: Value,
) -> ToolCallEnvelope {
    let started = std::time::Instant::now();

    match call_tool(engine, &tool, args).await {
        Ok(result) => ToolCallEnvelope {
            id,
            ok: true,
            tool,
            result: Some(result),
            error: None,
            meta: ToolCallMeta {
                elapsed_ms: started.elapsed().as_millis(),
                started_utc: None,
                finished_utc: None,
            },
        },

        Err(err) => ToolCallEnvelope {
            id,
            ok: false,
            tool,
            result: None,
            error: Some(classify_tool_error(&err)),
            meta: ToolCallMeta {
                elapsed_ms: started.elapsed().as_millis(),
                started_utc: None,
                finished_utc: None,
            },
        },
    }
}

pub fn audience_for_tool(name: &str) -> ToolAudience {
    match name {
        "status"
        | "plan-tools"
        | "evidence-search"
        | "source-investigate"
        | "hybrid-discover"
        | "source-read"
        | "scope-profile"
        | "pair-appearance"
        | "pair-profile"
        | "citation-verify"
        | "person-resolve"
        | "place-resolve"
        | "person-history"
        | "report-from-evidence"
        | "pdf-build" => ToolAudience::DefaultAgent,
        "phrase-index-search" | "catalog-index-info" | "vector-info" => ToolAudience::InternalDebug,
        _ => ToolAudience::Specialist,
    }
}

/// Get all tool definitions
pub fn tool_defs() -> Vec<ToolDef> {
    let mut defs = vec![
        // Status tool
        ToolDef {
            spec: ToolSpec {
                name: "status",
                description: "Show what's been ingested and which indexes are built under the data root.",
                input_schema: schema_for::<StatusRequest>(),
                output_schema: schema_for::<StatusResponse>(),
                requires: vec![],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Check system status",
                        args: serde_json::json!({}),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let _req: StatusRequest = serde_json::from_value(args)?;
                let res = engine.status_impl().await?;
                Ok(serde_json::to_value(res)?)
            }),
        },
        // Tool docs tool
        ToolDef {
            spec: ToolSpec {
                name: "tool-docs",
                description: "Return compiled-in documentation for all tools or one named tool.",
                input_schema: schema_for::<ToolDocsRequest>(),
                output_schema: schema_for::<ToolDocsResponse>(),
                requires: vec![],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Show all tool docs",
                        args: serde_json::json!({}),
                    },
                    ToolExample {
                        title: "Show search docs",
                        args: serde_json::json!({ "tool": "search" }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: ToolDocsRequest = serde_json::from_value(args)?;
                let res = engine.tool_docs_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Passage tool
        ToolDef {
            spec: ToolSpec {
                name: "passage",
                description: "Retrieve one passage by passage_id.",
                input_schema: schema_for::<PassageRequest>(),
                output_schema: schema_for::<PassageResponse>(),
                requires: vec!["passages.parquet"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Fetch seed passage",
                        args: serde_json::json!({
                            "id": "B/B13/B13n0079.xml#pB13p0047a0417"
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: PassageRequest = serde_json::from_value(args)?;
                let res = engine.passage_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Source-read tool
        ToolDef {
            spec: ToolSpec {
                name: "source-read",
                description: "Read an ordered source stream in cursor-based, citation-aware chunks.",
                input_schema: schema_for::<SourceReadRequest>(),
                output_schema: schema_for::<SourceReadResponse>(),
                requires: vec!["passages.parquet"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Start reading a work",
                        args: serde_json::json!({
                            "source_work_id": "T08n0235",
                            "direction": "start",
                            "max_chars": 4000
                        }),
                    },
                    ToolExample {
                        title: "Read around a passage",
                        args: serde_json::json!({
                            "passage_id": "T/T08/T08n0235.xml#pT08p0750c0201",
                            "direction": "around",
                            "before_chars": 1200,
                            "after_chars": 1800
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: SourceReadRequest = serde_json::from_value(args)?;
                let res = engine.source_read_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Search tool
        ToolDef {
            spec: ToolSpec {
                name: "search",
                description: "Exact phrase search across loaded passage text. Uses the phrase index when available, verifies candidates against parquet text, and falls back to a parquet scan if no index/doc table is available. Optional modes add work/division clusters or term-usage traces.",
                input_schema: schema_for::<SearchRequest>(),
                output_schema: schema_for::<SearchResponse>(),
                requires: vec!["passages.parquet"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Search for exact phrase hits",
                        args: serde_json::json!({
                            "phrase": "金剛經云",
                            "limit": 5
                        }),
                    },
                    ToolExample {
                        title: "Search and cluster hits by work",
                        args: serde_json::json!({
                            "phrase": "雪峯辭洞山",
                            "mode": "clusters",
                            "group_by": "work",
                            "limit": 50,
                            "limit_per_group": 10
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: SearchRequest = serde_json::from_value(args)?;
                let res = engine.search_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },
        // Heading-search tool
        ToolDef {
            spec: ToolSpec {
                name: "heading-search",
                description: "Search heading and section metadata by title/path, with passage-text fallback.",
                input_schema: schema_for::<HeadingSearchRequest>(),
                output_schema: schema_for::<HeadingSearchResponse>(),
                requires: vec!["passages.parquet"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Find sections headed by a case title",
                        args: serde_json::json!({
                            "query": "雪峰過嶺",
                            "limit": 10,
                            "brief": true
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: HeadingSearchRequest = serde_json::from_value(args)?;
                let res = engine.heading_search_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Canonical-source tool
        ToolDef {
            spec: ToolSpec {
                name: "canonical-source",
                description: "Find canon-side source passages for a phrase.",
                input_schema: schema_for::<CanonicalSourceRequest>(),
                output_schema: schema_for::<CanonicalSourceResponse>(),
                requires: vec!["passages.parquet"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Find canonical sources for Diamond Sutra phrase",
                        args: serde_json::json!({
                            "phrase": "一切有為法如夢幻泡影",
                            "limit": 50
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: CanonicalSourceRequest = serde_json::from_value(args)?;
                let res = engine.canonical_source_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Validate-adjudication tool
        ToolDef {
            spec: ToolSpec {
                name: "validate-adjudication",
                description: "Validate an adjudication JSON file for structural correctness.",
                input_schema: schema_for::<ValidateAdjudicationRequest>(),
                output_schema: schema_for::<ValidateAdjudicationResponse>(),
                requires: vec![],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Validate adjudication file",
                        args: serde_json::json!({
                            "path": "GraphDiscovery/Runs/text-reuse-discovery/adjudications/test3.json"
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: ValidateAdjudicationRequest = serde_json::from_value(args)?;
                let res = engine.validate_adjudication_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Graph-build tool
        ToolDef {
            spec: ToolSpec {
                name: "graph-build",
                description: "Build evidence graph from adjudication JSON.",
                input_schema: schema_for::<GraphBuildRequest>(),
                output_schema: schema_for::<GraphBuildResponse>(),
                requires: vec![],
                safety: ToolSafety::WritesOutput,
                examples: vec![
                    ToolExample {
                        title: "Build evidence graph",
                        args: serde_json::json!({
                            "input": "GraphDiscovery/Runs/text-reuse-discovery/adjudications/test3.json",
                            "kind": "evidence",
                            "name": "test3",
                            "out": "GraphDiscovery/Runs/text-reuse-discovery/drafts/test3.graph-draft.json"
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: GraphBuildRequest = serde_json::from_value(args)?;
                let res = engine.graph_build_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Report-build tool
        ToolDef {
            spec: ToolSpec {
                name: "report-build",
                description: "Build markdown report from adjudication and graph files.",
                input_schema: schema_for::<ReportBuildRequest>(),
                output_schema: schema_for::<ReportBuildResponse>(),
                requires: vec![],
                safety: ToolSafety::WritesOutput,
                examples: vec![
                    ToolExample {
                        title: "Build markdown report",
                        args: serde_json::json!({
                            "inputs": [
                                "GraphDiscovery/Runs/text-reuse-discovery/adjudications/test3.json",
                                "GraphDiscovery/Runs/text-reuse-discovery/drafts/test3.graph-draft.json"
                            ],
                            "out": "GraphDiscovery/Runs/text-reuse-discovery/dossiers/test3.report.md",
                            "title": "Canonical Dependence Test 3"
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: ReportBuildRequest = serde_json::from_value(args)?;
                let res = engine.report_build_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },
        // PDF-build tool
        ToolDef {
            spec: ToolSpec {
                name: "pdf-build",
                description: "Build a PDF with the built-in Lopdf renderer from either Markdown or structured report/evidence JSON. Use input_json for the basic report template; no external PDF tools are required.",
                input_schema: schema_for::<PdfBuildRequest>(),
                output_schema: schema_for::<PdfBuildResponse>(),
                requires: vec![],
                safety: ToolSafety::WritesOutput,
                examples: vec![
                    ToolExample {
                        title: "Build PDF from structured report JSON",
                        args: serde_json::json!({
                            "input_json": "GraphDiscovery/Runs/text-reuse-discovery/dossiers/test3.report.json",
                            "out": "GraphDiscovery/Runs/text-reuse-discovery/dossiers/test3.report.pdf",
                            "title": "Canonical Dependence Test 3"
                        }),
                    },
                    ToolExample {
                        title: "Build PDF from model-authored Markdown",
                        args: serde_json::json!({
                            "input_markdown": "GraphDiscovery/Runs/text-reuse-discovery/dossiers/test3.report.md",
                            "out": "GraphDiscovery/Runs/text-reuse-discovery/dossiers/test3.report.pdf",
                            "side_by_side": true
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: PdfBuildRequest = serde_json::from_value(args)?;
                let res = engine.pdf_build_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Works tool
        ToolDef {
            spec: ToolSpec {
                name: "works",
                description: "List works in the catalog, optionally filtered by tradition/period/canon/author.",
                input_schema: schema_for::<WorksRequest>(),
                output_schema: schema_for::<WorksResponse>(),
                requires: vec!["catalog.index"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "List all works",
                        args: serde_json::json!({
                            "limit": 50
                        }),
                    },
                    ToolExample {
                        title: "Filter by tradition",
                        args: serde_json::json!({
                            "tradition": "canon",
                            "limit": 50
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: WorksRequest = serde_json::from_value(args)?;
                let res = engine.works_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Catalog-index-info tool
        ToolDef {
            spec: ToolSpec {
                name: "catalog-index-info",
                description: "Show catalog index metadata.",
                input_schema: schema_for::<CatalogIndexInfoRequest>(),
                output_schema: schema_for::<CatalogIndexInfoResponse>(),
                requires: vec!["catalog.index"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Get catalog index info",
                        args: serde_json::json!({}),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let _req: CatalogIndexInfoRequest = serde_json::from_value(args)?;
                let res = engine.catalog_index_info_impl(_req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Vector-info tool
        ToolDef {
            spec: ToolSpec {
                name: "vector-info",
                description: "Show vector index metadata and doc-table compatibility.",
                input_schema: schema_for::<VectorInfoRequest>(),
                output_schema: schema_for::<VectorInfoResponse>(),
                requires: vec!["vector.index", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Get vector index info",
                        args: serde_json::json!({}),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: VectorInfoRequest = serde_json::from_value(args)?;
                let res = engine.vector_info_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Vector-neighbors tool
        ToolDef {
            spec: ToolSpec {
                name: "vector-neighbors",
                description: "Find semantic neighbor candidates from a seed passage or external query embedding. Results are discovery candidates, not exact evidence.",
                input_schema: schema_for::<VectorNeighborsRequest>(),
                output_schema: schema_for::<VectorNeighborsResponse>(),
                requires: vec!["vector.index", "doc_table.bin", "passages.parquet"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Find seed passage vector neighbors",
                        args: serde_json::json!({
                            "seed_passage_id": "B/B13/B13n0079.xml#pB13p0047a0417",
                            "k": 25
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: VectorNeighborsRequest = serde_json::from_value(args)?;
                let res = engine.vector_neighbors_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Similar tool
        ToolDef {
            spec: ToolSpec {
                name: "similar",
                description: "Find TF-IDF similar passages to a seed passage.",
                input_schema: schema_for::<SimilarRequest>(),
                output_schema: schema_for::<SimilarResponse>(),
                requires: vec!["passages.parquet", "tfidf.index", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Find similar passages",
                        args: serde_json::json!({
                            "seed": "B/B13/B13n0079.xml#pB13p0047a0417",
                            "limit": 25
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: SimilarRequest = serde_json::from_value(args)?;
                let res = engine.similar_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Frontier tool
        ToolDef {
            spec: ToolSpec {
                name: "frontier",
                description: "Generate a discovery frontier packet for an agent session.",
                input_schema: schema_for::<FrontierRequest>(),
                output_schema: schema_for::<FrontierResponse>(),
                requires: vec!["passages.parquet", "tfidf.index", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Generate frontier packet",
                        args: serde_json::json!({
                            "seed": "B/B13/B13n0079.xml#pB13p0047a0417",
                            "limit": 25,
                            "phrase_limit": 20
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: FrontierRequest = serde_json::from_value(args)?;
                let res = engine.frontier_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // First-attestation tool
        ToolDef {
            spec: ToolSpec {
                name: "first-attestation",
                description: "Find the earliest attestation of a phrase, ordered by period_rank.",
                input_schema: schema_for::<FirstAttestationRequest>(),
                output_schema: schema_for::<FirstAttestationResponse>(),
                requires: vec!["passages.parquet", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Find earliest attestation",
                        args: serde_json::json!({
                            "phrase": "一切有為法如夢幻泡影",
                            "limit": 25
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: FirstAttestationRequest = serde_json::from_value(args)?;
                let res = engine.first_attestation_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Phrase-history tool
        ToolDef {
            spec: ToolSpec {
                name: "phrase-history",
                description: "Analyze the historical distribution of a phrase across periods, canons, and traditions.",
                input_schema: schema_for::<PhraseHistoryRequest>(),
                output_schema: schema_for::<PhraseHistoryResponse>(),
                requires: vec!["passages.parquet"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Analyze phrase history",
                        args: serde_json::json!({
                            "phrase": "一切有為法",
                            "timeline": true
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: PhraseHistoryRequest = serde_json::from_value(args)?;
                let res = engine.phrase_history_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Phrase-index-search tool
        ToolDef {
            spec: ToolSpec {
                name: "phrase-index-search",
                description: "Search for a phrase using the phrase index for fast lookup.",
                input_schema: schema_for::<PhraseIndexSearchRequest>(),
                output_schema: schema_for::<PhraseIndexSearchResponse>(),
                requires: vec!["passages.parquet", "phrase.index"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Search phrase index",
                        args: serde_json::json!({
                            "phrase": "一切有為法",
                            "limit": 25
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: PhraseIndexSearchRequest = serde_json::from_value(args)?;
                let res = engine.phrase_index_search_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Seed-pick tool
        ToolDef {
            spec: ToolSpec {
                name: "seed-pick",
                description: "Pick unworked seed passages for research, filtered by tradition and period.",
                input_schema: schema_for::<SeedPickRequest>(),
                output_schema: schema_for::<SeedPickResponse>(),
                requires: vec!["passages.parquet"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Pick seed passages",
                        args: serde_json::json!({
                            "tradition": ["canon"],
                            "period": ["Tang"],
                            "limit": 50
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: SeedPickRequest = serde_json::from_value(args)?;
                let res = engine.seed_pick_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Expand-context-adaptive tool
        ToolDef {
            spec: ToolSpec {
                name: "expand-context-adaptive",
                description: "Expand context around a passage by climbing the catalog tree to fit a character budget.",
                input_schema: schema_for::<ExpandContextAdaptiveRequest>(),
                output_schema: schema_for::<ExpandContextAdaptiveResponse>(),
                requires: vec!["passages.parquet", "catalog.index", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Expand context",
                        args: serde_json::json!({
                            "passage_id": "B/B13/B13n0079.xml#pB13p0047a0417",
                            "max_chars": 5000
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: ExpandContextAdaptiveRequest = serde_json::from_value(args)?;
                let res = engine.expand_context_adaptive_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Trace-term-usage tool
        ToolDef {
            spec: ToolSpec {
                name: "trace-term-usage",
                description: "Trace term usage across periods, canons, authors, or works with hit counts and representative passages.",
                input_schema: schema_for::<TraceTermUsageRequest>(),
                output_schema: schema_for::<TraceTermUsageResponse>(),
                requires: vec!["passages.parquet", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Trace term usage by period",
                        args: serde_json::json!({
                            "phrase": "一切有為法",
                            "group_by": "period",
                            "limit_total": 200,
                            "limit_per_group": 5
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: TraceTermUsageRequest = serde_json::from_value(args)?;
                let res = engine.trace_term_usage_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Query-expand-terms tool
        ToolDef {
            spec: ToolSpec {
                name: "query-expand-terms",
                description: "Produce variants/orthographic flips/aliases for a seed phrase using bundled lookup tables.",
                input_schema: schema_for::<QueryExpandTermsRequest>(),
                output_schema: schema_for::<QueryExpandTermsResponse>(),
                requires: vec![],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Expand query terms",
                        args: serde_json::json!({
                            "phrase": "一切有為法",
                            "mode": "all",
                            "max": 20
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: QueryExpandTermsRequest = serde_json::from_value(args)?;
                let res = engine.query_expand_terms_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Compare-usage tool
        ToolDef {
            spec: ToolSpec {
                name: "compare-usage",
                description: "Compare two sub-corpora and return distinctive terms using log-odds ratio scoring.",
                input_schema: schema_for::<CompareUsageRequest>(),
                output_schema: schema_for::<CompareUsageResponse>(),
                requires: vec!["passages.parquet", "catalog.index", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Compare usage between canons",
                        args: serde_json::json!({
                            "scope_a_canon": "canon",
                            "scope_b_canon": "zhonghua",
                            "gram_len": 1,
                            "limit_passages": 1000,
                            "limit_terms": 50
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: CompareUsageRequest = serde_json::from_value(args)?;
                let res = engine.compare_usage_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Collocation-search tool
        ToolDef {
            spec: ToolSpec {
                name: "collocation-search",
                description: "Find terms that co-occur near a seed phrase more often than expected by chance.",
                input_schema: schema_for::<CollocationSearchRequest>(),
                output_schema: schema_for::<CollocationSearchResponse>(),
                requires: vec!["passages.parquet", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Search for collocates",
                        args: serde_json::json!({
                            "phrase": "一切有為法",
                            "window_chars": 20,
                            "gram_len": 1,
                            "limit_total": 200,
                            "limit_collocates": 30
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: CollocationSearchRequest = serde_json::from_value(args)?;
                let res = engine.collocation_search_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Pair-appearance tool
        ToolDef {
            spec: ToolSpec {
                name: "pair-appearance",
                description: "Find passages where two specified terms both appear, optionally constrained to a character window or sentence.",
                input_schema: schema_for::<PairAppearanceRequest>(),
                output_schema: schema_for::<PairAppearanceResponse>(),
                requires: vec!["passages.parquet", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Find co-mentions in the same passage",
                        args: serde_json::json!({
                            "term1": "念佛",
                            "term2": "禪",
                            "unit": "passage",
                            "limit": 10
                        }),
                    },
                    ToolExample {
                        title: "Find local co-occurrence within a window",
                        args: serde_json::json!({
                            "term1": "念佛",
                            "term2": "禪",
                            "unit": "window",
                            "window_chars": 80,
                            "limit": 10
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: PairAppearanceRequest = serde_json::from_value(args)?;
                let res = engine.pair_appearance_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Outline-search tool
        ToolDef {
            spec: ToolSpec {
                name: "outline-search",
                description: "Search for a phrase within a catalog outline node and return hits grouped by child outline nodes.",
                input_schema: schema_for::<OutlineSearchRequest>(),
                output_schema: schema_for::<OutlineSearchResponse>(),
                requires: vec!["passages.parquet", "catalog.index", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Search within a work",
                        args: serde_json::json!({
                            "phrase": "一切有為法",
                            "work_id": "B/B13/B13n0079.xml",
                            "group_by": "division",
                            "limit_total": 200,
                            "limit_per_group": 20
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: OutlineSearchRequest = serde_json::from_value(args)?;
                let res = engine.outline_search_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Cluster-hits tool
        ToolDef {
            spec: ToolSpec {
                name: "cluster-hits",
                description: "Cluster phrase search hits by catalog outline (work/division), returning hit counts per cluster with representative passages.",
                input_schema: schema_for::<ClusterHitsRequest>(),
                output_schema: schema_for::<ClusterHitsResponse>(),
                requires: vec!["passages.parquet", "catalog.index", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Cluster hits by work",
                        args: serde_json::json!({
                            "phrase": "一切有為法",
                            "cluster_by": "work",
                            "limit_total": 200,
                            "limit_per_cluster": 20
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: ClusterHitsRequest = serde_json::from_value(args)?;
                let res = engine.cluster_hits_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Absence-check tool
        ToolDef {
            spec: ToolSpec {
                name: "absence-check",
                description: "Check whether a phrase is absent from a specific catalog scope (work, canon, period).",
                input_schema: schema_for::<AbsenceCheckRequest>(),
                output_schema: schema_for::<AbsenceCheckResponse>(),
                requires: vec!["passages.parquet", "catalog.index", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Check absence in a work",
                        args: serde_json::json!({
                            "phrase": "一切有為法",
                            "scope_work_id": "B/B13/B13n0079.xml",
                            "limit": 100
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: AbsenceCheckRequest = serde_json::from_value(args)?;
                let res = engine.absence_check_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Evidence-search wrapper
        ToolDef {
            spec: ToolSpec {
                name: "plan-tools",
                description: "Recommend an agent workflow and concrete next tool calls for a research task.",
                input_schema: schema_for::<PlanToolsRequest>(),
                output_schema: schema_for::<PlanToolsResponse>(),
                requires: vec![],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Plan exact evidence then discovery",
                        args: serde_json::json!({
                            "task": "find earliest usage of 一切有為法 and compare related passages",
                            "known_phrase": "一切有為法"
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: PlanToolsRequest = serde_json::from_value(args)?;
                let res = engine.plan_tools_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Evidence-search wrapper
        ToolDef {
            spec: ToolSpec {
                name: "evidence-search",
                description: "Run exact phrase evidence search plus optional attestation/history/usage/cluster summaries.",
                input_schema: schema_for::<EvidenceSearchRequest>(),
                output_schema: schema_for::<EvidenceSearchResponse>(),
                requires: vec!["passages.parquet"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Search phrase evidence",
                        args: serde_json::json!({
                            "phrase": "一切有為法",
                            "limit": 25,
                            "include_attestation": true,
                            "include_history": true
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: EvidenceSearchRequest = serde_json::from_value(args)?;
                let res = engine.evidence_search_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Hybrid-discover wrapper
        ToolDef {
            spec: ToolSpec {
                name: "hybrid-discover",
                description: "Combine vector and TF-IDF discovery candidates when both indexes are available, or degrade to explicit lexical-only/semantic-only discovery mode.",
                input_schema: schema_for::<HybridDiscoverRequest>(),
                output_schema: schema_for::<HybridDiscoverResponse>(),
                requires: vec!["passages.parquet", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Hybrid discovery from a seed passage",
                        args: serde_json::json!({
                            "seed_passage_id": "B/B13/B13n0079.xml#pB13p0047a0417",
                            "limit": 25
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: HybridDiscoverRequest = serde_json::from_value(args)?;
                let res = engine.hybrid_discover_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Source-investigate wrapper
        ToolDef {
            spec: ToolSpec {
                name: "source-investigate",
                description: "Gather seed passage context, frontier, similarity, vector neighbors, and phrase histories for source investigation.",
                input_schema: schema_for::<SourceInvestigateRequest>(),
                output_schema: schema_for::<SourceInvestigateResponse>(),
                requires: vec!["passages.parquet", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Investigate a seed passage",
                        args: serde_json::json!({
                            "seed_passage_id": "B/B13/B13n0079.xml#pB13p0047a0417",
                            "phrases": ["一切有為法"],
                            "limit": 10
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: SourceInvestigateRequest = serde_json::from_value(args)?;
                let res = engine.source_investigate_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Scope-profile wrapper
        ToolDef {
            spec: ToolSpec {
                name: "scope-profile",
                description: "Compare two corpus scopes and optionally trace a phrase inside the same profile.",
                input_schema: schema_for::<ScopeProfileRequest>(),
                output_schema: schema_for::<ScopeProfileResponse>(),
                requires: vec!["passages.parquet", "catalog.index", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Compare usage between two canons",
                        args: serde_json::json!({
                            "scope_a_canon": "T",
                            "scope_b_canon": "X",
                            "gram_len": 1,
                            "limit_passages": 1000
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: ScopeProfileRequest = serde_json::from_value(args)?;
                let res = engine.scope_profile_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Batch-evidence-search wrapper
        ToolDef {
            spec: ToolSpec {
                name: "batch-evidence-search",
                description: "Search for multiple phrases and return compact per-phrase summaries.",
                input_schema: schema_for::<BatchEvidenceSearchRequest>(),
                output_schema: schema_for::<BatchEvidenceSearchResponse>(),
                requires: vec!["passages.parquet"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Search multiple phrases",
                        args: serde_json::json!({
                            "phrases": ["金剛經", "般若波羅蜜多"],
                            "limit": 10
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: BatchEvidenceSearchRequest = serde_json::from_value(args)?;
                let res = engine.batch_evidence_search_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // pair-profile tool
        ToolDef {
            spec: ToolSpec {
                name: "pair-profile",
                description: "Summarise how often two terms appear together versus separately, grouped by period, canon, work, or author. Use for analytical questions like 'does term A appear with term B more in Song than Tang sources?'",
                input_schema: schema_for::<PairProfileRequest>(),
                output_schema: schema_for::<PairProfileResponse>(),
                requires: vec!["passages.parquet", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Compare 念佛+禪 co-occurrence rates by period",
                        args: serde_json::json!({
                            "term1": "念佛",
                            "term2": "禪",
                            "group_by": "period",
                            "unit": "passage",
                            "limit_groups": 10
                        }),
                    },
                    ToolExample {
                        title: "Compare by canon, scoped to Taishō",
                        args: serde_json::json!({
                            "term1": "如來",
                            "term2": "法身",
                            "group_by": "work",
                            "unit": "passage",
                            "scope_canon": "T",
                            "limit_groups": 15,
                            "sample_hits_per_group": 2
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: PairProfileRequest = serde_json::from_value(args)?;
                let res = engine.pair_profile_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // person-resolve tool
        ToolDef {
            spec: ToolSpec {
                name: "person-resolve",
                description: "Resolve a person's name to candidate forms and show corpus presence. Use before person-history to confirm spelling and aliases are present in the corpus.",
                input_schema: schema_for::<PersonResolveRequest>(),
                output_schema: schema_for::<PersonResolveResponse>(),
                requires: vec!["passages.parquet"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Resolve with primary name and alias",
                        args: serde_json::json!({
                            "name": "雪峰義存",
                            "aliases": ["雪峰", "義存"]
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: PersonResolveRequest = serde_json::from_value(args)?;
                let res = engine.person_resolve_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // place-resolve tool
        ToolDef {
            spec: ToolSpec {
                name: "place-resolve",
                description: "Resolve a place name to its DDBC authority record (coordinates, category, alternate names) and show corpus presence. Use to disambiguate historical place names and find geographic context.",
                input_schema: schema_for::<PlaceResolveRequest>(),
                output_schema: schema_for::<PlaceResolveResponse>(),
                requires: vec!["passages.parquet"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Resolve a place name",
                        args: serde_json::json!({
                            "name": "那爛陀",
                            "aliases": ["那爛陀寺"]
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: PlaceResolveRequest = serde_json::from_value(args)?;
                let res = engine.place_resolve_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // person-history tool
        ToolDef {
            spec: ToolSpec {
                name: "person-history",
                description: "Retrieve passages mentioning a person, ordered by period. Returns mention-class labels (lineage_relation, attributed_saying, case_appearance, commentarial_reference, name_mention). Run person-resolve first to confirm name forms.",
                input_schema: schema_for::<PersonHistoryRequest>(),
                output_schema: schema_for::<PersonHistoryResponse>(),
                requires: vec!["passages.parquet"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Person history for 雪峰 with aliases",
                        args: serde_json::json!({
                            "name": "雪峰義存",
                            "aliases": ["雪峰", "義存"],
                            "limit": 200
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: PersonHistoryRequest = serde_json::from_value(args)?;
                let res = engine.person_history_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // citation-verify tool
        ToolDef {
            spec: ToolSpec {
                name: "citation-verify",
                description: "Verify whether a claimed quotation appears in the corpus, optionally scoped to a specific work, canon, or node. Returns exact hits and near-matches when the exact quote is not found. Use for provenance questions like 'is this saying really from the Diamond Sutra?'",
                input_schema: schema_for::<CitationVerifyRequest>(),
                output_schema: schema_for::<CitationVerifyResponse>(),
                requires: vec!["passages.parquet", "doc_table.bin"],
                safety: ToolSafety::ReadOnly,
                examples: vec![
                    ToolExample {
                        title: "Verify a quote in the Taishō canon",
                        args: serde_json::json!({
                            "quote": "一切有為法如夢幻泡影",
                            "scope_canon": "T",
                            "claimed_attribution": "金剛般若波羅蜜經",
                            "limit": 5,
                            "include_near_matches": true
                        }),
                    },
                    ToolExample {
                        title: "Verify scoped to a single work",
                        args: serde_json::json!({
                            "quote": "直指人心見性成佛",
                            "scope_source_work_id": "B/B13/B13n0079.xml",
                            "include_near_matches": true,
                            "near_match_limit": 5
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: CitationVerifyRequest = serde_json::from_value(args)?;
                let res = engine.citation_verify_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },

        // Report-from-evidence wrapper
        ToolDef {
            spec: ToolSpec {
                name: "report-from-evidence",
                description: "Validate adjudication, build evidence graph, and build the markdown report in one workflow.",
                input_schema: schema_for::<ReportFromEvidenceRequest>(),
                output_schema: schema_for::<ReportFromEvidenceResponse>(),
                requires: vec![],
                safety: ToolSafety::WritesOutput,
                examples: vec![
                    ToolExample {
                        title: "Build graph and report",
                        args: serde_json::json!({
                            "adjudication": "GraphDiscovery/Runs/text-reuse-discovery/adjudications/test3.json",
                            "graph_out": "GraphDiscovery/Runs/text-reuse-discovery/drafts/test3.graph-draft.json",
                            "report_out": "GraphDiscovery/Runs/text-reuse-discovery/dossiers/test3.report.md",
                            "title": "Canonical Dependence"
                        }),
                    }
                ],
            },
            call: |engine, args| Box::pin(async move {
                let req: ReportFromEvidenceRequest = serde_json::from_value(args)?;
                let res = engine.report_from_evidence_impl(req).await?;
                Ok(serde_json::to_value(res)?)
            }),
        },
    ];

    // annotate_response injects _term_context and _entity_context into every tool
    // response. Explicitly allow additional properties in all output schemas so
    // strict JSON-Schema validators do not reject annotated responses.
    for def in &mut defs {
        if let Some(obj) = def.spec.output_schema.as_object_mut() {
            obj.insert("additionalProperties".to_string(), serde_json::Value::Bool(true));
        }
    }

    defs
}

// Implementations will be added to engine.rs
