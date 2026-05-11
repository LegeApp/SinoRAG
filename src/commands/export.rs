//! Thin dispatcher for export commands. All template rendering lives in
//! `crate::templates::*`; this file only routes CLI calls and owns the PDF
//! pipeline (a separate concern from template rendering).

use crate::jsonout::write_or_print;
use crate::templates::{
    self, evidence_graph, lineage_graph, markdown_report, readzen_collection, timeline_graph,
};
use anyhow::Result;
use chrono::Utc;
use serde_json::{json, Value};
use std::path::PathBuf;

// Re-export so callers (e.g. `commands::mod`) keep their existing import surface.
pub use crate::templates::GraphKind;

pub fn markdown(input: PathBuf, out: PathBuf, title: Option<String>) -> Result<()> {
    let payload = templates::read_json(&input)?;
    let md = markdown_report::render(&payload, title.as_deref());
    templates::write_text(&out, &md)?;
    println!("wrote {}", out.display());
    Ok(())
}

pub fn readzen(input: PathBuf, out: Option<PathBuf>, name: Option<String>) -> Result<()> {
    let payload = templates::read_json(&input)?;
    let collection = readzen_collection::render(&payload, name.as_deref());
    write_or_print(&json!([collection]), out)
}

pub fn graph(
    input: PathBuf,
    out: Option<PathBuf>,
    kind: GraphKind,
    name: Option<String>,
) -> Result<()> {
    let payload = templates::read_json(&input)?;
    let title = name
        .clone()
        .unwrap_or_else(|| templates::default_title(&payload));
    let graph = match kind {
        GraphKind::Evidence => evidence_graph::render(&payload, &title),
        GraphKind::Timeline => timeline_graph::render(&payload, &title),
        GraphKind::Lineage => lineage_graph::render(&payload, &title),
    };
    write_or_print(&graph, out)
}

pub fn pdf(input_markdown: PathBuf, out: PathBuf, side_by_side: bool) -> Result<()> {
    let markdown = std::fs::read_to_string(&input_markdown)?;
    pdf_from_markdown(&markdown, out, side_by_side)
}

pub fn report_build(
    inputs: Vec<PathBuf>,
    out: PathBuf,
    title: Option<String>,
    essay_max_pages: usize,
) -> Result<()> {
    let mut artifacts = Vec::new();
    for input in inputs {
        let payload = templates::read_json(&input)?;
        artifacts.push(json!({
            "path": input.display().to_string(),
            "schema": payload.get("schema").and_then(Value::as_str).unwrap_or(""),
            "query": templates::query_raw(&payload),
            "confidence": payload.get("confidence").and_then(Value::as_str).unwrap_or(""),
            "results": payload.get("results").cloned().unwrap_or(Value::Null),
            "method": payload.get("method").cloned().unwrap_or(Value::Null),
        }));
    }

    let evidence = templates::merge_evidence(&artifacts)?;
    let caveats = templates::merge_string_arrays(&artifacts, "caveats")?;
    let next_steps = templates::merge_string_arrays(&artifacts, "next_steps")?;
    let inferred_title = title.unwrap_or_else(|| {
        artifacts
            .iter()
            .find_map(|a| a.get("query").and_then(Value::as_str))
            .filter(|q| !q.is_empty())
            .map(|q| format!("GraphDiscovery Report: {q}"))
            .unwrap_or_else(|| "GraphDiscovery Research Report".to_string())
    });
    let report = json!({
        "schema": "readzen-research-report-v1",
        "title": inferred_title,
        "created_by": "graphdiscovery-rust",
        "created_utc": Utc::now().to_rfc3339(),
        "essay_max_pages": essay_max_pages,
        "query": {
            "raw": artifacts
                .iter()
                .filter_map(|a| a.get("query").and_then(Value::as_str))
                .find(|q| !q.is_empty())
                .unwrap_or("")
        },
        "method": {
            "contract": "combine semantic research artifacts into a dossier before prose synthesis",
            "source_artifact_count": artifacts.len(),
            "evidence_policy": "prose claims should cite evidence records or preserve caveats"
        },
        "output_contracts": {
            "short_answer": "answer from evidence with caveats",
            "essay": "up to configured page limit, organized by evidence chronology and source confidence",
            "markdown": "export-markdown",
            "pdf": "export-pdf --features pdf-export",
            "readzen_collection": "export-readzen",
            "graphs": ["graph-build --kind evidence", "graph-build --kind timeline", "graph-build --kind lineage"]
        },
        "results": {
            "source_artifacts": artifacts,
            "evidence_count": evidence.len()
        },
        "evidence": evidence,
        "caveats": caveats,
        "next_steps": next_steps,
        "confidence": "evidence_bundle"
    });
    templates::write_json(&out, &report)?;
    println!("wrote {}", out.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// PDF pipeline (separate concern from templates; kept here as a CLI sink)
// ---------------------------------------------------------------------------

#[cfg(feature = "pdf-export")]
fn pdf_from_markdown(markdown: &str, out: PathBuf, side_by_side: bool) -> Result<()> {
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let (zh, en) = markdown_to_pdf_sections(markdown);
    let mut context = cbeta_pdf_creator::FontContext::initialize_fonts()?;
    context.set_options(595.0, 842.0, 72.0, 12.5, 11.5, 1.35, 8.0, 6.0, 0.45);
    let output = out.to_string_lossy().to_string();
    if side_by_side {
        cbeta_pdf_creator::create_bilingual_pdf_side_by_side_with_context(&zh, &en, &output, &context)?;
    } else {
        cbeta_pdf_creator::create_bilingual_pdf_with_context(&zh, &en, &output, &context)?;
    }
    println!("wrote {}", out.display());
    Ok(())
}

#[cfg(not(feature = "pdf-export"))]
fn pdf_from_markdown(_markdown: &str, _out: PathBuf, _side_by_side: bool) -> Result<()> {
    anyhow::bail!("PDF export is integrated but not enabled in this build. Rebuild with `--features pdf-export`.")
}

#[cfg(feature = "pdf-export")]
fn markdown_to_pdf_sections(markdown: &str) -> (Vec<String>, Vec<String>) {
    let mut zh = Vec::new();
    let mut en = Vec::new();
    let mut in_code = false;
    let mut current_zh = Vec::new();
    let mut current_en = Vec::new();
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            if in_code {
                if !current_zh.is_empty() {
                    zh.push(current_zh.join("\n"));
                    en.push(current_en.join("\n"));
                }
                current_zh.clear();
                current_en.clear();
            }
            in_code = !in_code;
            continue;
        }
        if in_code {
            if crate::normalize::contains_cjk(line) {
                current_zh.push(line.to_string());
            }
        } else if !line.trim().is_empty() && !line.starts_with("---") {
            current_en.push(strip_markdown(line));
        }
    }
    if zh.is_empty() {
        zh.push(String::new());
        en.push(markdown.lines().map(strip_markdown).collect::<Vec<_>>().join("\n"));
    }
    (zh, en)
}

#[cfg(feature = "pdf-export")]
fn strip_markdown(line: &str) -> String {
    line.trim_start_matches('#')
        .trim_start_matches("- ")
        .trim_start_matches("> ")
        .trim()
        .to_string()
}
