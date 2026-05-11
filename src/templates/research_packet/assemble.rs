//! Assemble phase: scan gathered tool outputs, extract cited passages,
//! write contexts/full-work texts/pre-diagrams + manifest + README, zip.

use super::brief::Brief;
use super::gather::ToolInvocation;
use super::recipe::Recipe;
use crate::datafusion_store::{sql_literal, DataFusionStore};
use crate::pack::Pack;
use crate::templates::{evidence_graph, lineage_graph, timeline_graph};
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use zip::write::FileOptions;

pub const PACKET_SCHEMA: &str = "sinoragd-research-packet-v1";

#[derive(Debug, Clone)]
pub struct CitedPassage {
    pub passage_id: String,
    pub source_work_id: String,
    pub source_rel_path: String,
    /// Raw evidence item as it appeared inside a tool output — used for
    /// pre-diagram rendering, which expects this shape.
    pub item: Value,
}

pub struct PacketStats {
    pub passages_written: usize,
    pub works_seen: BTreeMap<String, usize>,
    pub cited: Vec<CitedPassage>,
}

// ---------------------------------------------------------------------------
// Pass 1 — extract cited passages from gather outputs and write passages/.
// ---------------------------------------------------------------------------

pub fn collect_and_write_passages(
    packet_root: &Path,
    passages_dir: &Path,
    invocations: &[ToolInvocation],
) -> Result<PacketStats> {
    std::fs::create_dir_all(passages_dir)?;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut works: BTreeMap<String, usize> = BTreeMap::new();
    let mut cited: Vec<CitedPassage> = Vec::new();
    let mut written = 0usize;

    for inv in invocations {
        if inv.error.is_some() { continue; }
        let path = packet_root.join(&inv.output_relpath);
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else { continue };
        scan_value(&value, &mut seen, &mut works, &mut cited, passages_dir, &mut written)?;
    }

    Ok(PacketStats { passages_written: written, works_seen: works, cited })
}

fn scan_value(
    value: &Value,
    seen: &mut BTreeSet<String>,
    works: &mut BTreeMap<String, usize>,
    cited: &mut Vec<CitedPassage>,
    passages_dir: &Path,
    written: &mut usize,
) -> Result<()> {
    for key in ["results", "evidence", "rows", "hits", "items"] {
        if let Some(arr) = value.get(key).and_then(Value::as_array) {
            for item in arr {
                try_emit_passage(item, seen, works, cited, passages_dir, written)?;
            }
        }
    }
    if let Some(buckets) = value.get("buckets").and_then(Value::as_array) {
        for bucket in buckets {
            if let Some(arr) = bucket.get("rows").and_then(Value::as_array) {
                for item in arr {
                    try_emit_passage(item, seen, works, cited, passages_dir, written)?;
                }
            }
        }
    }
    Ok(())
}

fn try_emit_passage(
    item: &Value,
    seen: &mut BTreeSet<String>,
    works: &mut BTreeMap<String, usize>,
    cited: &mut Vec<CitedPassage>,
    passages_dir: &Path,
    written: &mut usize,
) -> Result<()> {
    let pid = item.get("passage_id").and_then(Value::as_str).unwrap_or("");
    if pid.is_empty() || !seen.insert(pid.to_string()) { return Ok(()); }

    let wid = item.get("source_work_id").and_then(Value::as_str).unwrap_or("").to_string();
    if !wid.is_empty() { *works.entry(wid.clone()).or_insert(0) += 1; }

    cited.push(CitedPassage {
        passage_id: pid.to_string(),
        source_work_id: wid,
        source_rel_path: item.get("source_rel_path").and_then(Value::as_str).unwrap_or("").to_string(),
        item: item.clone(),
    });

    let safe = safe_filename(pid);
    std::fs::write(passages_dir.join(format!("{safe}.md")), render_passage_md(item))?;
    *written += 1;
    Ok(())
}

fn render_passage_md(item: &Value) -> String {
    let mut s = String::new();
    s.push_str("---\n");
    s.push_str("schema: sinoragd-passage-md-v1\n");
    for k in [
        "passage_id", "source_work_id", "main_title", "author",
        "period", "canon", "source_rel_path", "rights_id",
    ] {
        if let Some(v) = item.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()) {
            s.push_str(&format!("{k}: {v}\n"));
        }
    }
    s.push_str("---\n\n");
    let body = item.get("zh_quote").and_then(Value::as_str)
        .or_else(|| item.get("zh_text_normalized").and_then(Value::as_str))
        .or_else(|| item.get("zh_text").and_then(Value::as_str))
        .unwrap_or("");
    s.push_str(body);
    s.push('\n');
    s
}

fn safe_filename(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

// ---------------------------------------------------------------------------
// Pass 2 — contexts/. expand-context for each cited passage.
// ---------------------------------------------------------------------------

pub async fn write_contexts(
    contexts_dir: &Path,
    pack: &Pack,
    cited: &[CitedPassage],
    before: usize,
    after: usize,
) -> Result<usize> {
    std::fs::create_dir_all(contexts_dir)?;
    let parquet = pack.passages_path();
    let mut written = 0usize;
    for c in cited {
        let safe = safe_filename(&c.passage_id);
        let out = contexts_dir.join(format!("{safe}.context.json"));
        let res = crate::commands::expand_context::run(
            parquet.clone(),
            Some(c.passage_id.clone()),
            None, None,
            before, after,
            Some(out.clone()),
        ).await;
        match res {
            Ok(()) => written += 1,
            Err(e) => eprintln!("  ! expand-context for {}: {}", c.passage_id, e),
        }
    }
    Ok(written)
}

// ---------------------------------------------------------------------------
// Pass 3 — documents/. Full text of works with >= threshold cited passages.
// ---------------------------------------------------------------------------

pub async fn write_documents(
    documents_dir: &Path,
    pack: &Pack,
    works_seen: &BTreeMap<String, usize>,
    threshold: usize,
) -> Result<Vec<String>> {
    std::fs::create_dir_all(documents_dir)?;
    let heavy: Vec<&String> = works_seen.iter()
        .filter(|(_, n)| **n >= threshold)
        .map(|(w, _)| w)
        .collect();
    if heavy.is_empty() { return Ok(Vec::new()); }

    let store = DataFusionStore::open(&pack.passages_path()).await?;
    let mut written: Vec<String> = Vec::new();
    for work_id in heavy {
        let sql = format!(
            r#"SELECT passage_id, zh_text_normalized, heading, from_lb, to_lb,
                       main_title, author, period, canon
                FROM passages
                WHERE source_work_id = {}
                  AND zh_text_normalized IS NOT NULL
                  AND length(zh_text_normalized) > 0
                ORDER BY passage_id"#,
            sql_literal(work_id)
        );
        let rows = store.query_json(&sql).await
            .with_context(|| format!("query work {work_id}"))?;
        if rows.is_empty() { continue; }
        let md = render_work_md(work_id, &rows);
        let safe = safe_filename(work_id);
        std::fs::write(documents_dir.join(format!("{safe}.full.md")), md)?;
        written.push(work_id.clone());
    }
    Ok(written)
}

fn render_work_md(work_id: &str, rows: &[Value]) -> String {
    let first = rows.first();
    let f = |k: &str| -> &str {
        first.and_then(|r| r.get(k)).and_then(Value::as_str).unwrap_or("")
    };
    let mut s = String::new();
    s.push_str("---\n");
    s.push_str("schema: sinoragd-work-md-v1\n");
    s.push_str(&format!("work_id: {work_id}\n"));
    for k in ["main_title", "author", "period", "canon"] {
        let v = f(k);
        if !v.is_empty() { s.push_str(&format!("{k}: {v}\n")); }
    }
    s.push_str(&format!("passage_count: {}\n", rows.len()));
    s.push_str("---\n\n");
    s.push_str(&format!("# {} ({})\n\n", f("main_title"), work_id));
    for row in rows {
        let pid = row.get("passage_id").and_then(Value::as_str).unwrap_or("");
        let heading = row.get("heading").and_then(Value::as_str).unwrap_or("");
        let lb_from = row.get("from_lb").and_then(Value::as_str).unwrap_or("");
        let lb_to = row.get("to_lb").and_then(Value::as_str).unwrap_or("");
        let text = row.get("zh_text_normalized").and_then(Value::as_str).unwrap_or("");
        if !heading.is_empty() {
            s.push_str(&format!("\n## {heading}\n\n"));
        }
        let lb_marker = match (lb_from, lb_to) {
            ("", "") => String::new(),
            (a, b) if a == b => format!(" ({a})"),
            (a, b)           => format!(" ({a}–{b})"),
        };
        s.push_str(&format!("**`{pid}`**{lb_marker}\n\n"));
        s.push_str("```text\n");
        s.push_str(text);
        s.push_str("\n```\n\n");
    }
    s
}

// ---------------------------------------------------------------------------
// Pass 4 — pre_diagrams/. Feed the cited list through the graph templates.
// ---------------------------------------------------------------------------

pub fn write_pre_diagrams(
    diagrams_dir: &Path,
    brief: &Brief,
    cited: &[CitedPassage],
) -> Result<()> {
    std::fs::create_dir_all(diagrams_dir)?;
    // Synthesize the evidence-payload shape the templates consume.
    let evidence_items: Vec<Value> = cited.iter().map(|c| {
        let mut item = c.item.clone();
        if let Some(obj) = item.as_object_mut() {
            // Normalize for templates: ensure `zh_quote` is populated.
            if !obj.contains_key("zh_quote") {
                let q = obj.get("zh_text_normalized").cloned()
                    .or_else(|| obj.get("zh_text").cloned())
                    .unwrap_or(Value::String(String::new()));
                obj.insert("zh_quote".to_string(), q);
            }
            if !obj.contains_key("lb_range") {
                let from = obj.get("from_lb").and_then(Value::as_str).unwrap_or("").to_string();
                let to   = obj.get("to_lb").and_then(Value::as_str).unwrap_or("").to_string();
                let lb = match (from.as_str(), to.as_str()) {
                    ("", "") => String::new(),
                    (a, b) if a == b => a.to_string(),
                    (a, b) => format!("{a}–{b}"),
                };
                obj.insert("lb_range".to_string(), Value::String(lb));
            }
        }
        item
    }).collect();

    let payload = json!({
        "schema": "sinoragd-packet-evidence-payload-v1",
        "query": { "raw": brief.topic.clone() },
        "evidence": evidence_items,
    });

    let title = format!("Research packet: {}", brief.topic);
    let evidence = evidence_graph::render(&payload, &title);
    let timeline = timeline_graph::render(&payload, &title);
    let lineage  = lineage_graph::render(&payload, &title);

    std::fs::write(diagrams_dir.join("evidence.json"), serde_json::to_vec_pretty(&evidence)?)?;
    std::fs::write(diagrams_dir.join("timeline.json"), serde_json::to_vec_pretty(&timeline)?)?;
    std::fs::write(diagrams_dir.join("lineage.json"),  serde_json::to_vec_pretty(&lineage)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Manifest / README / index.jsonl
// ---------------------------------------------------------------------------

pub fn write_manifest(
    packet_root: &Path,
    brief: &Brief,
    recipe: &Recipe,
    pack: &Pack,
    invocations: &[ToolInvocation],
    stats: &PacketStats,
    contexts_written: usize,
    documents_written: &[String],
) -> Result<()> {
    let manifest = json!({
        "schema": PACKET_SCHEMA,
        "packet_id": brief.topic,
        "created_utc": Utc::now().to_rfc3339(),
        "brief_schema": super::brief::BRIEF_SCHEMA,
        "topic": brief.topic,
        "seed_count": brief.seeds.len(),
        "recipe": {
            "name": recipe.name,
            "description": recipe.description,
            "full_work_threshold": recipe.full_work_threshold,
            "context_before": recipe.context_before,
            "context_after": recipe.context_after,
            "step_count": recipe.steps.len(),
        },
        "pack": {
            "pack_id": pack.manifest.pack_id,
            "doc_table_fingerprint": pack.manifest.fingerprints.doc_table,
        },
        "tool_invocations": invocations,
        "stats": {
            "passages_written": stats.passages_written,
            "contexts_written": contexts_written,
            "documents_written": documents_written,
            "distinct_works": stats.works_seen.len(),
            "works_seen": stats.works_seen,
            "errors": invocations.iter().filter(|i| i.error.is_some()).count(),
        }
    });
    std::fs::write(packet_root.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
    Ok(())
}

pub fn write_readme(packet_root: &Path, brief: &Brief, recipe: &Recipe) -> Result<()> {
    let mut s = String::new();
    s.push_str("# SinoRAG Research Packet\n\n");
    s.push_str("This packet was assembled by SinoRAG as raw research material for a downstream agent or researcher. **SinoRAG did not write any prose conclusions.** Every claim in a final report should be tied to the primary sources in this packet.\n\n");
    s.push_str(&format!("- **Topic:** {}\n", brief.topic));
    s.push_str(&format!("- **Recipe:** `{}` — {}\n\n", recipe.name, recipe.description));

    s.push_str("## Contents\n\n");
    s.push_str("| Path | Purpose |\n|---|---|\n");
    s.push_str("| `manifest.json` | Provenance: pack id, fingerprint, brief, recipe, every tool call. |\n");
    s.push_str("| `brief/brief.json` | The original brief, machine-readable. |\n");
    s.push_str("| `brief/brief.md` | The brief, human-readable. |\n");
    s.push_str("| `tools/` | Raw JSON output from each SinoRAG tool invocation. |\n");
    s.push_str("| `passages/` | Each cited passage as Markdown (frontmatter + Chinese text). |\n");
    s.push_str("| `contexts/` | `±N`-passage windows around each cited passage. |\n");
    s.push_str("| `documents/` | Full text of works that contributed `>= recipe.full_work_threshold` passages. |\n");
    s.push_str("| `pre_diagrams/` | Evidence / timeline / lineage graph drafts (JSON). |\n");
    s.push_str("| `index.jsonl` | One row per artifact for fast scanning without unzipping. |\n\n");

    s.push_str("## How to use\n\n");
    s.push_str("1. Read `brief/brief.md` for the question.\n");
    s.push_str("2. Read `tools/*.json` for structured findings (each file is one tool's raw output).\n");
    s.push_str("3. Open `passages/*.md` for source text whenever you cite something.\n");
    s.push_str("4. Use `contexts/*.context.json` when a cited passage needs surrounding text to make sense.\n");
    s.push_str("5. Use `documents/*.full.md` when a single work supplies most of the evidence.\n");
    s.push_str("6. Use `pre_diagrams/*.json` as starting layouts for visualization; refine before publishing.\n");
    s.push_str("7. Verify provenance in `manifest.json`.\n");
    Ok(std::fs::write(packet_root.join("README.md"), s)?)
}

pub fn write_brief_files(packet_root: &Path, brief: &Brief) -> Result<()> {
    let dir = packet_root.join("brief");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("brief.json"), serde_json::to_vec_pretty(brief)?)?;
    std::fs::write(dir.join("brief.md"),   brief.render_markdown())?;
    Ok(())
}

pub fn write_index_jsonl(packet_root: &Path) -> Result<()> {
    let path = packet_root.join("index.jsonl");
    let mut w = BufWriter::new(File::create(path)?);
    for entry in walkdir::WalkDir::new(packet_root).into_iter().filter_map(Result::ok) {
        let p = entry.path();
        if !p.is_file() { continue; }
        let rel = p.strip_prefix(packet_root).unwrap_or(p);
        if rel == Path::new("index.jsonl") { continue; }
        let bytes = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        writeln!(w, "{}", json!({ "path": rel.to_string_lossy(), "bytes": bytes }))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Zip
// ---------------------------------------------------------------------------

pub fn seal_zip(packet_root: &Path, zip_path: &Path) -> Result<()> {
    if let Some(parent) = zip_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let zf = File::create(zip_path).with_context(|| format!("create {}", zip_path.display()))?;
    let mut zw = zip::ZipWriter::new(BufWriter::new(zf));
    let options = FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    for entry in walkdir::WalkDir::new(packet_root).into_iter().filter_map(Result::ok) {
        let p = entry.path();
        if !p.is_file() { continue; }
        let rel = p.strip_prefix(packet_root).unwrap_or(p);
        zw.start_file(rel.to_string_lossy(), options)?;
        let mut f = File::open(p)?;
        let mut buf = Vec::with_capacity(64 * 1024);
        f.read_to_end(&mut buf)?;
        zw.write_all(&buf)?;
    }
    zw.finish()?;
    Ok(())
}

#[allow(dead_code)]
pub fn rel(packet_root: &Path, p: &Path) -> PathBuf {
    p.strip_prefix(packet_root).unwrap_or(p).to_path_buf()
}
