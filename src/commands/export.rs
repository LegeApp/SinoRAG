use crate::jsonout::write_or_print;
use anyhow::Result;
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
pub enum GraphKind {
    Evidence,
    Timeline,
    Lineage,
}

pub fn markdown(input: PathBuf, out: PathBuf, title: Option<String>) -> Result<()> {
    let payload = read_json(&input)?;
    let markdown = render_markdown(&payload, title.as_deref());
    write_text(&out, &markdown)?;
    println!("wrote {}", out.display());
    Ok(())
}

pub fn readzen(input: PathBuf, out: Option<PathBuf>, name: Option<String>) -> Result<()> {
    let payload = read_json(&input)?;
    let collection = readzen_collection(&payload, name.as_deref());
    write_or_print(&json!([collection]), out)
}

pub fn graph(
    input: PathBuf,
    out: Option<PathBuf>,
    kind: GraphKind,
    name: Option<String>,
) -> Result<()> {
    let payload = read_json(&input)?;
    let graph = graph_payload(&payload, kind, name.as_deref());
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
        let payload = read_json(&input)?;
        artifacts.push(json!({
            "path": input.display().to_string(),
            "schema": payload.get("schema").and_then(Value::as_str).unwrap_or(""),
            "query": query_raw(&payload),
            "confidence": payload.get("confidence").and_then(Value::as_str).unwrap_or(""),
            "results": payload.get("results").cloned().unwrap_or(Value::Null),
            "method": payload.get("method").cloned().unwrap_or(Value::Null),
        }));
    }

    let evidence = merge_evidence(&artifacts)?;
    let caveats = merge_string_arrays(&artifacts, "caveats")?;
    let next_steps = merge_string_arrays(&artifacts, "next_steps")?;
    let inferred_title = title.unwrap_or_else(|| {
        artifacts
            .iter()
            .find_map(|artifact| artifact.get("query").and_then(Value::as_str))
            .filter(|query| !query.is_empty())
            .map(|query| format!("GraphDiscovery Report: {query}"))
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
                .filter_map(|artifact| artifact.get("query").and_then(Value::as_str))
                .find(|query| !query.is_empty())
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
    write_json(&out, &report)?;
    println!("wrote {}", out.display());
    Ok(())
}

fn render_markdown(payload: &Value, title_override: Option<&str>) -> String {
    let title = title_override
        .map(ToString::to_string)
        .or_else(|| {
            payload
                .get("title")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| default_title(payload));
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("format: graphdiscovery-report-md-v1\n");
    out.push_str(&format!("created_utc: {}\n", Utc::now().to_rfc3339()));
    out.push_str("max_length_hint: up to 3 pages\n");
    out.push_str("---\n\n");
    out.push_str(&format!("# {}\n\n", title));
    out.push_str("> This report is an evidence scaffold for an LLM or researcher. Claims should stay tied to the cited Chinese evidence below.\n\n");

    out.push_str("## Query\n\n");
    out.push_str(&format!("- Raw: {}\n", query_raw(payload)));
    out.push_str(&format!(
        "- Schema: {}\n",
        payload.get("schema").and_then(Value::as_str).unwrap_or("")
    ));
    if let Some(confidence) = payload.get("confidence").and_then(Value::as_str) {
        out.push_str(&format!("- Confidence: {confidence}\n"));
    }
    out.push('\n');

    out.push_str("## Answer Scaffold\n\n");
    out.push_str("- Start with the shortest defensible answer.\n");
    out.push_str("- Distinguish loaded-corpus evidence from historical origin.\n");
    out.push_str("- Use exact Chinese quotes from the Evidence section when making claims.\n");
    out.push_str(
        "- For a longer essay, organize by earliest evidence, later distribution, and caveats.\n\n",
    );

    if let Some(results) = payload.get("results") {
        out.push_str("## Structured Results\n\n");
        out.push_str("```json\n");
        out.push_str(&serde_json::to_string_pretty(results).unwrap_or_default());
        out.push_str("\n```\n\n");
    }

    let evidence = evidence_items(payload);
    out.push_str("## Evidence\n\n");
    if evidence.is_empty() {
        out.push_str("No evidence records were present in the input artifact.\n\n");
    } else {
        for (idx, item) in evidence.iter().enumerate() {
            out.push_str(&format!("### Evidence {}\n\n", idx + 1));
            out.push_str(&format!(
                "- Passage: `{}`\n",
                item.get("passage_id").and_then(Value::as_str).unwrap_or("")
            ));
            out.push_str(&format!(
                "- Source: `{}`",
                item.get("source_rel_path")
                    .and_then(Value::as_str)
                    .unwrap_or("")
            ));
            if let Some(lb) = item
                .get("lb_range")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
            {
                out.push_str(&format!(" ({lb})"));
            }
            out.push('\n');
            for key in [
                "main_title",
                "author",
                "period",
                "canon",
                "source_corpus",
                "rights_id",
            ] {
                if let Some(value) = item
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                {
                    out.push_str(&format!("- {}: {}\n", key.replace('_', " "), value));
                }
            }
            out.push_str("\n```text\n");
            out.push_str(item.get("zh_quote").and_then(Value::as_str).unwrap_or(""));
            out.push_str("\n```\n\n");
        }
    }

    out.push_str("## Caveats\n\n");
    if let Some(caveats) = payload.get("caveats").and_then(Value::as_array) {
        for caveat in caveats.iter().filter_map(Value::as_str) {
            out.push_str(&format!("- {caveat}\n"));
        }
    }
    out.push('\n');

    out.push_str("## Next Steps\n\n");
    if let Some(next_steps) = payload.get("next_steps").and_then(Value::as_array) {
        for step in next_steps.iter().filter_map(Value::as_str) {
            out.push_str(&format!("- {step}\n"));
        }
    } else {
        out.push_str(
            "- Run additional targeted searches before expanding this into a polished essay.\n",
        );
    }
    out
}

fn readzen_collection(payload: &Value, name_override: Option<&str>) -> Value {
    let name = name_override
        .map(ToString::to_string)
        .unwrap_or_else(|| default_title(payload));
    let evidence = evidence_items(payload);
    let mut seen = BTreeSet::new();
    let mut passages = Vec::new();
    for item in evidence {
        let id = item.get("passage_id").and_then(Value::as_str).unwrap_or("");
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        passages.push(json!({
            "Id": id,
            "SourceRelPath": item.get("source_rel_path").and_then(Value::as_str).unwrap_or(""),
            "ZhText": item.get("zh_quote").and_then(Value::as_str).unwrap_or(""),
            "EnText": "",
            "Notes": format_notes(&item),
            "FromLb": item.get("lb_range").and_then(Value::as_str).unwrap_or(""),
            "ToLb": item.get("lb_range").and_then(Value::as_str).unwrap_or(""),
            "Summary": item.get("main_title").and_then(Value::as_str).unwrap_or(id),
            "Tags": ["GraphDiscovery"],
            "CreatedBy": "graphdiscovery-rust",
            "AddedUtc": Utc::now().to_rfc3339()
        }));
    }

    json!({
        "Id": stable_id("gd-collection", &name),
        "Name": name,
        "Description": "Generated from GraphDiscovery evidence artifact.",
        "Tags": ["GraphDiscovery"],
        "CreatedUtc": Utc::now().to_rfc3339(),
        "CreatedBy": "graphdiscovery-rust",
        "SchemaVersion": 2,
        "Passages": passages,
        "Edges": [],
        "Concepts": [],
        "Links": [],
        "StudyNotes": "Evidence imported from GraphDiscovery. Review claims before treating graph edges as accepted.",
        "GraphLayout": {
            "NodePositions": {},
            "OffsetX": 0,
            "OffsetY": 0,
            "Zoom": 1.0
        }
    })
}

fn graph_payload(payload: &Value, kind: GraphKind, name_override: Option<&str>) -> Value {
    let title = name_override
        .map(ToString::to_string)
        .unwrap_or_else(|| default_title(payload));
    match kind {
        GraphKind::Evidence => evidence_graph(payload, &title),
        GraphKind::Timeline => timeline_graph(payload, &title),
        GraphKind::Lineage => lineage_graph(payload, &title),
    }
}

fn evidence_graph(payload: &Value, title: &str) -> Value {
    let evidence = evidence_items(payload);
    let nodes = evidence.iter().map(node_from_evidence).collect::<Vec<_>>();
    json!({
        "schema": "readzen-text-reuse-graph-draft-v1",
        "graph_id": stable_id("gd-graph", title),
        "name": title,
        "description": "Evidence graph draft generated from GraphDiscovery artifact.",
        "source_task_id": stable_id("gd-task", &query_raw(payload)),
        "created_by": "graphdiscovery-rust",
        "layout_policy": {
            "orientation": "free",
            "allowed_shapes": ["standard", "person", "concept", "source", "text"],
            "edge_direction": "all-directions-allowed"
        },
        "nodes": nodes,
        "edges": Vec::<Value>::new()
    })
}

fn timeline_graph(payload: &Value, title: &str) -> Value {
    let evidence = evidence_items(payload);
    let mut buckets: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for item in evidence {
        let period = item
            .get("period")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .unwrap_or("Unknown")
            .to_string();
        buckets.entry(period).or_default().push(item);
    }
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut previous_period_id: Option<String> = None;
    for (idx, (period, items)) in buckets.into_iter().enumerate() {
        let period_id = format!("period-{idx}-{}", stable_id("", &period));
        nodes.push(json!({
            "id": period_id,
            "node_type": "concept",
            "label": period,
            "shape": "standard",
            "layout": {
                "orientation": "horizontal",
                "x": (idx as i32) * 420,
                "y": 0,
                "min_width": 280,
                "notes_space": "expanded"
            },
            "metadata": {"count": items.len(), "timeline_order": idx}
        }));
        if let Some(prev) = previous_period_id {
            edges.push(json!({
                "id": format!("timeline-edge-{idx}"),
                "from_node_id": prev,
                "to_node_id": period_id,
                "relation_label": "precedes",
                "readzen_relation_type": "precedes",
                "confidence": 1.0
            }));
        }
        previous_period_id = Some(period_id.clone());
        for (item_idx, item) in items.into_iter().enumerate() {
            let node = node_from_evidence(&item);
            let passage_id = node
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            nodes.push(with_layout(
                node,
                "standard",
                (idx as i32) * 420,
                140 + (item_idx as i32) * 170,
                "horizontal",
            ));
            edges.push(json!({
                "id": stable_id("timeline-member", &format!("{period_id}-{passage_id}")),
                "from_node_id": period_id,
                "to_node_id": passage_id,
                "relation_label": "contains evidence",
                "readzen_relation_type": "contains-evidence",
                "confidence": 1.0,
                "evidence": [item]
            }));
        }
    }
    json!({
        "schema": "readzen-timeline-graph-draft-v1",
        "graph_id": stable_id("gd-timeline", title),
        "name": title,
        "description": "Timeline graph draft generated from evidence periods.",
        "created_by": "graphdiscovery-rust",
        "layout_policy": {
            "orientation": "horizontal",
            "primary_shape": "standard",
            "rank_direction": "left-to-right",
            "notes_space": "expanded",
            "edge_direction": "all-directions-allowed"
        },
        "nodes": nodes,
        "edges": edges
    })
}

fn lineage_graph(payload: &Value, title: &str) -> Value {
    let evidence = evidence_items(payload);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for item in evidence {
        let passage_node = with_layout(
            node_from_evidence(&item),
            "standard",
            360,
            (nodes.len() as i32) * 120,
            "vertical",
        );
        let passage_id = passage_node
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        nodes.push(passage_node);
        for name in extract_names(item.get("zh_quote").and_then(Value::as_str).unwrap_or("")) {
            let person_id = stable_id("person", &name);
            nodes.push(json!({
                "id": person_id,
                "node_type": "person",
                "label": name,
                "shape": "person",
                "layout": {
                    "orientation": "vertical",
                    "x": 0,
                    "y": (nodes.len() as i32) * 120,
                    "rank_direction": "top-to-bottom"
                },
                "metadata": {"source": "rule_based_quote_scan"}
            }));
            edges.push(json!({
                "id": stable_id("lineage-edge", &format!("{person_id}-{passage_id}")),
                "from_node_id": person_id,
                "to_node_id": passage_id,
                "relation_label": "appears in evidence",
                "readzen_relation_type": "appears-in-evidence",
                "confidence": 0.25,
                "evidence": [item.clone()]
            }));
        }
    }
    dedup_graph(json!({
        "schema": "readzen-lineage-graph-draft-v1",
        "graph_id": stable_id("gd-lineage", title),
        "name": title,
        "description": "Lineage-oriented graph draft. Person extraction is conservative and rule-based.",
        "created_by": "graphdiscovery-rust",
        "layout_policy": {
            "orientation": "vertical",
            "primary_shape": "person",
            "rank_direction": "top-to-bottom",
            "edge_direction": "all-directions-allowed"
        },
        "nodes": nodes,
        "edges": edges,
        "caveats": ["Rule-based person extraction is not authority control.", "Review all lineage edges before accepting them."]
    }))
}

fn node_from_evidence(item: &Value) -> Value {
    let id = item.get("passage_id").and_then(Value::as_str).unwrap_or("");
    json!({
        "id": id,
        "node_type": "passage",
        "label": item.get("main_title").and_then(Value::as_str).filter(|v| !v.is_empty()).unwrap_or(id),
        "source_rel_path": item.get("source_rel_path").and_then(Value::as_str).unwrap_or(""),
        "xml_id": item.get("xml_id").and_then(Value::as_str).unwrap_or(""),
        "heading": item.get("main_title").and_then(Value::as_str).unwrap_or(""),
        "from_lb": item.get("lb_range").and_then(Value::as_str).unwrap_or(""),
        "to_lb": item.get("lb_range").and_then(Value::as_str).unwrap_or(""),
        "zh": item.get("zh_quote").and_then(Value::as_str).unwrap_or(""),
        "en_label_hint": "",
        "metadata": {
            "canon": item.get("canon").and_then(Value::as_str).unwrap_or(""),
            "period": item.get("period").and_then(Value::as_str).unwrap_or(""),
            "author": item.get("author").and_then(Value::as_str).unwrap_or(""),
            "main_title": item.get("main_title").and_then(Value::as_str).unwrap_or(""),
            "source_corpus": item.get("source_corpus").and_then(Value::as_str).unwrap_or(""),
            "rights_id": item.get("rights_id").and_then(Value::as_str).unwrap_or("")
        }
    })
}

fn with_layout(mut node: Value, shape: &str, x: i32, y: i32, orientation: &str) -> Value {
    if let Some(obj) = node.as_object_mut() {
        obj.insert("shape".to_string(), json!(shape));
        obj.insert(
            "layout".to_string(),
            json!({
                "orientation": orientation,
                "x": x,
                "y": y,
                "edge_direction": "all-directions-allowed"
            }),
        );
    }
    node
}

fn evidence_items(payload: &Value) -> Vec<Value> {
    payload
        .get("evidence")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn merge_evidence(artifacts: &[Value]) -> Result<Vec<Value>> {
    let mut evidence = Vec::new();
    let mut seen = BTreeSet::new();
    for artifact in artifacts {
        let path = artifact.get("path").and_then(Value::as_str).unwrap_or("");
        let payload = read_json(&PathBuf::from(path))?;
        for mut item in evidence_items(&payload) {
            let id = item
                .get("passage_id")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| stable_id("evidence", &item.to_string()));
            if seen.insert(id) {
                if let Some(obj) = item.as_object_mut() {
                    obj.insert(
                        "source_artifact_schema".to_string(),
                        json!(payload.get("schema").and_then(Value::as_str).unwrap_or("")),
                    );
                }
                evidence.push(item);
            }
        }
    }
    Ok(evidence)
}

fn merge_string_arrays(artifacts: &[Value], key: &str) -> Result<Vec<String>> {
    let mut values = BTreeSet::new();
    for artifact in artifacts {
        let path = artifact.get("path").and_then(Value::as_str).unwrap_or("");
        let payload = read_json(&PathBuf::from(path))?;
        if let Some(items) = payload.get(key).and_then(Value::as_array) {
            for item in items.iter().filter_map(Value::as_str) {
                values.insert(item.to_string());
            }
        }
    }
    Ok(values.into_iter().collect())
}

fn query_raw(payload: &Value) -> String {
    payload
        .get("query")
        .and_then(|q| q.get("raw"))
        .and_then(Value::as_str)
        .or_else(|| payload.get("phrase").and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
}

fn default_title(payload: &Value) -> String {
    let raw = query_raw(payload);
    if raw.is_empty() {
        "GraphDiscovery Report".to_string()
    } else {
        format!("GraphDiscovery Report: {raw}")
    }
}

fn format_notes(item: &Value) -> String {
    [
        "source_corpus",
        "canon",
        "period",
        "author",
        "rights_id",
        "snapshot_id",
    ]
    .iter()
    .filter_map(|key| {
        item.get(*key)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(|v| format!("{key}={v}"))
    })
    .collect::<Vec<_>>()
    .join("; ")
}

fn stable_id(prefix: &str, value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hex::encode(hasher.finalize());
    if prefix.is_empty() {
        digest[..12].to_string()
    } else {
        format!("{prefix}-{}", &digest[..12])
    }
}

fn extract_names(text: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    for marker in ["祖", "師", "禪師", "和尚"] {
        if let Some(idx) = text.find(marker) {
            let before = text[..idx].chars().rev().take(3).collect::<Vec<_>>();
            let name = before.into_iter().rev().collect::<String>();
            if !name.is_empty() {
                names.insert(format!("{name}{marker}"));
            }
        }
    }
    names.into_iter().collect()
}

fn dedup_graph(mut graph: Value) -> Value {
    if let Some(nodes) = graph.get_mut("nodes").and_then(Value::as_array_mut) {
        let mut seen = BTreeSet::new();
        nodes.retain(|node| {
            let id = node.get("id").and_then(Value::as_str).unwrap_or("");
            !id.is_empty() && seen.insert(id.to_string())
        });
    }
    graph
}

fn read_json(path: &PathBuf) -> Result<Value> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

fn write_text(path: &PathBuf, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)?;
    Ok(())
}

fn write_json(path: &PathBuf, value: &Value) -> Result<()> {
    write_text(path, &serde_json::to_string_pretty(value)?)
}

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
        cbeta_pdf_creator::create_bilingual_pdf_side_by_side_with_context(
            &zh, &en, &output, &context,
        )?;
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
        en.push(
            markdown
                .lines()
                .map(strip_markdown)
                .collect::<Vec<_>>()
                .join("\n"),
        );
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
