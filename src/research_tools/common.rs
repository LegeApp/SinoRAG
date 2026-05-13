use serde::{Deserialize, Serialize};

/// Scope filter shared by most F2 tools.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolScope {
    pub corpus: Option<Vec<String>>,
    pub canon: Option<Vec<String>>,
    pub period: Option<Vec<String>>,
    pub author: Option<Vec<String>>,
    pub source_work_id: Option<Vec<String>>,
    pub catalog_node_id: Option<u32>,
}

/// Consistent evidence passage shape returned by all F2 tools.
#[derive(Debug, Clone, Serialize)]
pub struct EvidencePassage {
    pub passage_id: String,
    pub doc_id: u32,
    pub source_work_id: String,
    pub main_title: Option<String>,
    pub author: Option<String>,
    pub period: Option<String>,
    pub period_rank: Option<i32>,
    pub canon: Option<String>,
    pub from_lb: Option<String>,
    pub to_lb: Option<String>,
    pub zh_quote: String,
    pub score: Option<f32>,
}

/// Build an EvidencePassage from a serde_json row Value (as returned by
/// DataFusionStore queries).
pub fn evidence_from_row(
    row: &serde_json::Value,
    doc_id: u32,
    zh_quote: String,
    score: Option<f32>,
) -> EvidencePassage {
    EvidencePassage {
        passage_id: crate::research::field_str(row, "passage_id"),
        doc_id,
        source_work_id: crate::research::field_str(row, "source_work_id"),
        main_title: opt_str(row, "main_title"),
        author: opt_str(row, "author"),
        period: opt_str(row, "period"),
        period_rank: row
            .get("period_rank")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32),
        canon: opt_str(row, "canon"),
        from_lb: opt_str(row, "from_lb"),
        to_lb: opt_str(row, "to_lb"),
        zh_quote,
        score,
    }
}

fn opt_str(row: &serde_json::Value, field: &str) -> Option<String> {
    let s = row.get(field).and_then(|v| v.as_str()).unwrap_or("");
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}
