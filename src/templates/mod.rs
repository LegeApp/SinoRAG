//! Output-shape templates: markdown report, graph drafts (evidence/timeline/
//! lineage), readzen collection, and the research packet. Templates are
//! deterministic functions over a `serde_json::Value` payload — no I/O of
//! their own. `commands::export` is the thin dispatcher.

pub mod evidence_graph;
pub mod lineage_graph;
pub mod markdown_report;
pub mod readzen_collection;
pub mod research_packet;
pub mod timeline_graph;
pub mod variants;

use anyhow::Result;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub enum GraphKind {
    Evidence,
    Timeline,
    Lineage,
}

// ---------------------------------------------------------------------------
// Shared payload accessors
// ---------------------------------------------------------------------------

pub fn evidence_items(payload: &Value) -> Vec<Value> {
    if let Some(items) = payload.get("evidence").and_then(Value::as_array) {
        return items.clone();
    }

    let mut out = Vec::new();

    if let Some(claims) = payload.get("accepted_claims").and_then(Value::as_array) {
        for claim in claims {
            let claim_id = claim.get("claim_id").and_then(|v| v.as_str()).unwrap_or("");

            let claim_type = claim
                .get("claim_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let ring = claim.get("ring").and_then(|v| v.as_str()).unwrap_or("");

            let matched_phrases = claim
                .get("matched_phrases")
                .cloned()
                .unwrap_or_else(|| json!([]));

            if let Some(evidence) = claim.get("evidence").and_then(Value::as_array) {
                for item in evidence {
                    let mut item = item.clone();

                    if let Some(obj) = item.as_object_mut() {
                        obj.insert("claim_id".to_string(), json!(claim_id));
                        obj.insert("claim_type".to_string(), json!(claim_type));
                        obj.insert("ring".to_string(), json!(ring));
                        obj.insert("matched_phrases".to_string(), matched_phrases.clone());

                        if !obj.contains_key("zh_quote") {
                            if let Some(q) = obj.get("quote_zh").cloned() {
                                obj.insert("zh_quote".to_string(), q);
                            }
                        }

                        if !obj.contains_key("evidence_role") {
                            if let Some(side) = obj.get("side").and_then(|v| v.as_str()) {
                                let role = match side {
                                    "zen" => "seed",
                                    "canon" => "candidate",
                                    other => other,
                                };
                                obj.insert("evidence_role".to_string(), json!(role));
                            }
                        }
                    }

                    out.push(item);
                }
            }
        }
    }

    out
}

pub fn query_raw(payload: &Value) -> String {
    payload
        .get("query")
        .and_then(|q| q.get("raw"))
        .and_then(Value::as_str)
        .or_else(|| payload.get("phrase").and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
}

pub fn default_title(payload: &Value) -> String {
    let raw = query_raw(payload);
    if raw.is_empty() {
        "GraphDiscovery Report".to_string()
    } else {
        format!("GraphDiscovery Report: {raw}")
    }
}

// ---------------------------------------------------------------------------
// Shared graph-node builders
// ---------------------------------------------------------------------------

pub fn node_from_evidence(item: &Value) -> Value {
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

pub fn with_layout(mut node: Value, shape: &str, x: i32, y: i32, orientation: &str) -> Value {
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

pub fn dedup_graph(mut graph: Value) -> Value {
    if let Some(nodes) = graph.get_mut("nodes").and_then(Value::as_array_mut) {
        let mut seen = BTreeSet::new();
        nodes.retain(|node| {
            let id = node.get("id").and_then(Value::as_str).unwrap_or("");
            !id.is_empty() && seen.insert(id.to_string())
        });
    }
    graph
}

// ---------------------------------------------------------------------------
// Shared misc helpers
// ---------------------------------------------------------------------------

pub fn stable_id(prefix: &str, value: &str) -> String {
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

pub fn format_notes(item: &Value) -> String {
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

/// Rule-based, intentionally conservative. Used by the lineage graph
/// template; not authority control.
pub fn extract_names(text: &str) -> Vec<String> {
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

// ---------------------------------------------------------------------------
// Artifact-list helpers (used by report_build)
// ---------------------------------------------------------------------------

pub fn merge_evidence(artifacts: &[Value]) -> Result<Vec<Value>> {
    let mut evidence = Vec::new();
    let mut seen = BTreeSet::new();
    for artifact in artifacts {
        let path = artifact.get("path").and_then(Value::as_str).unwrap_or("");
        let payload = read_json(Path::new(path))?;
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

pub fn merge_string_arrays(artifacts: &[Value], key: &str) -> Result<Vec<String>> {
    let mut values = BTreeSet::new();
    for artifact in artifacts {
        let path = artifact.get("path").and_then(Value::as_str).unwrap_or("");
        let payload = read_json(Path::new(path))?;
        if let Some(items) = payload.get(key).and_then(Value::as_array) {
            for item in items.iter().filter_map(Value::as_str) {
                values.insert(item.to_string());
            }
        }
    }
    Ok(values.into_iter().collect())
}

// ---------------------------------------------------------------------------
// File I/O helpers (templates are still pure — these are conveniences for
// the dispatcher in `commands::export`)
// ---------------------------------------------------------------------------

pub fn read_json(path: &Path) -> Result<Value> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

pub fn write_text(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)?;
    Ok(())
}

pub fn write_json(path: &Path, value: &Value) -> Result<()> {
    write_text(path, &serde_json::to_string_pretty(value)?)
}
