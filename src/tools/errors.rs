use serde::Serialize;
use std::path::PathBuf;

/// Classified tool error with structured information for agents
#[derive(Debug, Serialize)]
pub struct ToolErrorBody {
    pub code: String,
    pub message: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_command: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// Typed tool errors
#[derive(thiserror::Error, Debug)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("missing phrase index at {path}")]
    MissingPhraseIndex { path: PathBuf },

    #[error("missing tfidf index at {path}")]
    MissingTfidfIndex { path: PathBuf },

    #[error("missing catalog index at {path}")]
    MissingCatalogIndex { path: PathBuf },

    #[error("missing doc table at {path}")]
    MissingDocTable { path: PathBuf },

    #[error("missing passages parquet at {path}")]
    MissingPassages { path: PathBuf },

    #[error("readonly mode blocks tool: {tool}")]
    ReadonlyViolation { tool: String },

    #[error("admin tool disabled: {tool}")]
    AdminToolDisabled { tool: String },

    #[error("output path {path} is outside output root {root}")]
    OutputPathViolation { path: PathBuf, root: PathBuf },

    #[error("invalid JSON args: {0}")]
    InvalidJson(String),

    #[error("invalid args: {0}")]
    InvalidArgs(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl ToolError {
    pub fn unknown_tool(name: &str) -> Self {
        ToolError::UnknownTool(name.to_string())
    }

    pub fn into_anyhow(self) -> anyhow::Error {
        anyhow::Error::from(self)
    }
}

/// Classify an anyhow error into a structured ToolErrorBody
pub fn classify_tool_error(err: &anyhow::Error) -> ToolErrorBody {
    let msg = err.to_string();

    // Try to downcast to ToolError first
    if let Some(tool_err) = err.downcast_ref::<ToolError>() {
        return match tool_err {
            ToolError::UnknownTool(name) => ToolErrorBody {
                code: "unknown_tool".to_string(),
                message: format!("Unknown tool: {}", name),
                suggested_command: Some(
                    "Run 'sinorag tools-manifest' to see available tools".to_string(),
                ),
                details: None,
            },
            ToolError::MissingPhraseIndex { path } => ToolErrorBody {
                code: "missing_phrase_index".to_string(),
                message: format!("Phrase index not found at {}", path.display()),
                suggested_command: Some(format!(
                    "sinorag index phrase --parquet data/passages.parquet --out {}",
                    path.display()
                )),
                details: Some(serde_json::json!({ "path": path.display().to_string() })),
            },
            ToolError::MissingTfidfIndex { path } => ToolErrorBody {
                code: "missing_tfidf_index".to_string(),
                message: format!("TF-IDF index not found at {}", path.display()),
                suggested_command: Some(format!(
                    "sinorag index tfidf --parquet data/passages.parquet --out {}",
                    path.display()
                )),
                details: Some(serde_json::json!({ "path": path.display().to_string() })),
            },
            ToolError::MissingCatalogIndex { path } => ToolErrorBody {
                code: "missing_catalog_index".to_string(),
                message: format!("Catalog index not found at {}", path.display()),
                suggested_command: Some(
                    "Run 'sinorag catalog-index-build' to build the catalog index".to_string(),
                ),
                details: Some(serde_json::json!({ "path": path.display().to_string() })),
            },
            ToolError::MissingDocTable { path } => ToolErrorBody {
                code: "missing_doc_table".to_string(),
                message: format!("Document table not found at {}", path.display()),
                suggested_command: Some(
                    "Run 'sinorag doc-table-build' to build the document table".to_string(),
                ),
                details: Some(serde_json::json!({ "path": path.display().to_string() })),
            },
            ToolError::MissingPassages { path } => ToolErrorBody {
                code: "missing_passages".to_string(),
                message: format!("Passages parquet not found at {}", path.display()),
                suggested_command: Some("Run 'sinorag ingest' to build the corpus".to_string()),
                details: Some(serde_json::json!({ "path": path.display().to_string() })),
            },
            ToolError::ReadonlyViolation { tool } => ToolErrorBody {
                code: "readonly_violation".to_string(),
                message: format!(
                    "Tool '{}' writes output but engine is in readonly mode",
                    tool
                ),
                suggested_command: Some("Remove --readonly flag to allow writes".to_string()),
                details: Some(serde_json::json!({ "tool": tool })),
            },
            ToolError::AdminToolDisabled { tool } => ToolErrorBody {
                code: "admin_tool_disabled".to_string(),
                message: format!("Tool '{}' requires --allow-admin-tools flag", tool),
                suggested_command: Some(
                    "Add --allow-admin-tools flag to enable admin tools".to_string(),
                ),
                details: Some(serde_json::json!({ "tool": tool })),
            },
            ToolError::OutputPathViolation { path, root } => ToolErrorBody {
                code: "output_path_denied".to_string(),
                message: format!(
                    "Output path {} is outside output root {}",
                    path.display(),
                    root.display()
                ),
                suggested_command: Some(
                    "Specify --output-root or use a path inside the allowed directory".to_string(),
                ),
                details: Some(
                    serde_json::json!({ "path": path.display().to_string(), "root": root.display().to_string() }),
                ),
            },
            ToolError::InvalidJson(s) => ToolErrorBody {
                code: "invalid_json".to_string(),
                message: format!("Invalid JSON: {}", s),
                suggested_command: None,
                details: None,
            },
            ToolError::InvalidArgs(s) => ToolErrorBody {
                code: "invalid_args".to_string(),
                message: format!("Invalid arguments: {}", s),
                suggested_command: Some(
                    "Run 'sinorag explain-tool <tool>' for usage information".to_string(),
                ),
                details: None,
            },
            ToolError::Internal(s) => ToolErrorBody {
                code: "internal_error".to_string(),
                message: format!("Internal error: {}", s),
                suggested_command: None,
                details: None,
            },
        };
    }

    // Fallback: string matching for common patterns
    if msg.contains("phrase") && (msg.contains("not found") || msg.contains("No such file")) {
        return ToolErrorBody {
            code: "missing_phrase_index".to_string(),
            message: msg,
            suggested_command: Some(
                "sinorag index phrase --parquet data/passages.parquet --out data/derived/phrase.index".to_string()
            ),
            details: None,
        };
    }

    if msg.contains("tfidf") && (msg.contains("not found") || msg.contains("No such file")) {
        return ToolErrorBody {
            code: "missing_tfidf_index".to_string(),
            message: msg,
            suggested_command: Some(
                "sinorag index tfidf --parquet data/passages.parquet --out data/derived/tfidf.index".to_string()
            ),
            details: None,
        };
    }

    if msg.contains("catalog") && (msg.contains("not found") || msg.contains("No such file")) {
        return ToolErrorBody {
            code: "missing_catalog_index".to_string(),
            message: msg,
            suggested_command: Some(
                "Run 'sinorag catalog-index-build' to build the catalog index".to_string(),
            ),
            details: None,
        };
    }

    if msg.contains("passages") && (msg.contains("not found") || msg.contains("No such file")) {
        return ToolErrorBody {
            code: "missing_passages".to_string(),
            message: msg,
            suggested_command: Some("Run 'sinorag ingest' to build the corpus".to_string()),
            details: None,
        };
    }

    ToolErrorBody {
        code: "internal_error".to_string(),
        message: msg,
        suggested_command: None,
        details: None,
    }
}
