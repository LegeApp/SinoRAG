use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::datafusion_store::{sql_literal, DataFusionStore};
use crate::research::format_citation;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpandedContext {
    pub schema: String,
    pub request: ContextRequest,
    pub center: ExpandedCenter,
    pub context: Vec<ContextPassage>,
    pub section: ContextSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRequest {
    pub passage_id: String,
    pub hit_id: Option<String>,
    pub before: usize,
    pub after: usize,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpandedCenter {
    pub passage_id: String,
    pub hit_id: Option<String>,
    pub center_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPassage {
    pub relative_index: isize,
    pub passage_id: String,
    pub citation: String,
    pub source_rel_path: String,
    pub source_work_id: String,
    pub xml_id: String,
    pub heading: String,
    pub heading_path: String,
    pub div_path: String,
    pub from_lb: Option<String>,
    pub to_lb: Option<String>,
    pub zh_text_raw: String,
    pub zh_text_normalized: String,
    pub is_center: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSection {
    pub source_work_id: String,
    pub source_rel_path: String,
    pub heading_path: String,
    pub div_path: String,
    pub first_passage_id: Option<String>,
    pub last_passage_id: Option<String>,
    pub passage_count: usize,
}

pub async fn expand_passage_context(
    store: &DataFusionStore,
    passage_id: &str,
    hit_id: Option<String>,
    before: usize,
    after: usize,
) -> Result<ExpandedContext> {
    let center = store.get_passage(passage_id).await?;

    let source_rel_path = str_field(&center, "source_rel_path")?;
    let source_work_id = str_field(&center, "source_work_id").unwrap_or_default();
    let center_heading_path = str_field(&center, "heading_path").unwrap_or_default();
    let center_div_path = str_field(&center, "div_path").unwrap_or_default();

    // V1 strategy (updated):
    // Fetch same source file, order by passage_ord_in_file for correct ordering.
    let sql = format!(
        r#"
        SELECT passage_id, source_rel_path, xml_id, div_path, heading, heading_path,
               from_lb, to_lb, zh_text_raw, zh_text_normalized,
               source_work_id, source_section_id, source_locator,
               canon, canon_name, traditions, period, origin, author, main_title,
               period_rank, source_corpus, source_url, edition_siglum, edition_label,
               rights_id, rights_notes, retrieval_method, snapshot_id, quality_flags_json,
               passage_ord_in_file
        FROM passages
        WHERE source_rel_path = {}
        ORDER BY passage_ord_in_file ASC
        "#,
        sql_literal(source_rel_path)
    );

    let rows = store.query_json(&sql).await?;

    let center_index = rows
        .iter()
        .position(|row| {
            row.get("passage_id")
                .and_then(|v| v.as_str())
                == Some(passage_id)
        })
        .ok_or_else(|| anyhow!("center passage not found in source_rel_path query: {passage_id}"))?;

    let start = center_index.saturating_sub(before);
    let end_exclusive = (center_index + after + 1).min(rows.len());

    let mut context = Vec::new();

    for (idx, row) in rows[start..end_exclusive].iter().enumerate() {
        let absolute_idx = start + idx;
        let relative_index = absolute_idx as isize - center_index as isize;
        context.push(row_to_context_passage(row, relative_index, absolute_idx == center_index)?);
    }

    let first_passage_id = context.first().map(|p| p.passage_id.clone());
    let last_passage_id = context.last().map(|p| p.passage_id.clone());

    Ok(ExpandedContext {
        schema: "readzen-expanded-context-v1".to_string(),
        request: ContextRequest {
            passage_id: passage_id.to_string(),
            hit_id: hit_id.clone(),
            before,
            after,
            mode: "passage_window".to_string(),
        },
        center: ExpandedCenter {
            passage_id: passage_id.to_string(),
            hit_id,
            center_index,
        },
        section: ContextSection {
            source_work_id: source_work_id.to_string(),
            source_rel_path: source_rel_path.to_string(),
            heading_path: center_heading_path.to_string(),
            div_path: center_div_path.to_string(),
            first_passage_id,
            last_passage_id,
            passage_count: context.len(),
        },
        context,
    })
}

fn row_to_context_passage(row: &Value, relative_index: isize, is_center: bool) -> Result<ContextPassage> {
    let from_lb = row.get("from_lb").and_then(|v| v.as_str()).map(str::to_string);
    let to_lb = row.get("to_lb").and_then(|v| v.as_str()).map(str::to_string);

    let citation = format_citation(row, from_lb.as_deref().unwrap_or(""), to_lb.as_deref().unwrap_or(""));

    Ok(ContextPassage {
        relative_index,
        passage_id: str_field(row, "passage_id")?.to_string(),
        citation,
        source_rel_path: str_field(row, "source_rel_path").unwrap_or_default().to_string(),
        source_work_id: str_field(row, "source_work_id").unwrap_or_default().to_string(),
        xml_id: str_field(row, "xml_id").unwrap_or_default().to_string(),
        heading: str_field(row, "heading").unwrap_or_default().to_string(),
        heading_path: str_field(row, "heading_path").unwrap_or_default().to_string(),
        div_path: str_field(row, "div_path").unwrap_or_default().to_string(),
        from_lb,
        to_lb,
        zh_text_raw: str_field(row, "zh_text_raw").unwrap_or_default().to_string(),
        zh_text_normalized: str_field(row, "zh_text_normalized").unwrap_or_default().to_string(),
        is_center,
    })
}

fn str_field<'a>(row: &'a Value, key: &str) -> Result<&'a str> {
    row.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing string field `{key}`"))
}
