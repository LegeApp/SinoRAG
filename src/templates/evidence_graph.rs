//! Evidence-graph draft — `readzen-text-reuse-graph-draft-v1`.

use super::{dedup_graph, evidence_items, node_from_evidence, query_raw, stable_id};
use serde_json::{json, Value};

fn edges_from_accepted_claims(payload: &Value) -> Vec<Value> {
    let mut edges = Vec::new();

    let Some(claims) = payload.get("accepted_claims").and_then(Value::as_array) else {
        return edges;
    };

    for claim in claims {
        let from = claim
            .get("from_passage_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let to = claim
            .get("to_passage_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if from.is_empty() || to.is_empty() {
            continue;
        }

        let label = claim
            .get("graph_hint")
            .and_then(|h| h.get("edge_label"))
            .and_then(|v| v.as_str())
            .or_else(|| claim.get("relation_label").and_then(|v| v.as_str()))
            .or_else(|| claim.get("claim_type").and_then(|v| v.as_str()))
            .unwrap_or("related");

        let render = claim
            .get("graph_hint")
            .and_then(|h| h.get("render"))
            .and_then(|v| v.as_str())
            .or_else(|| claim.get("graph_hint").and_then(|h| h.get("render_policy")).and_then(|v| v.as_str()))
            .unwrap_or("render_default");

        edges.push(json!({
            "id": format!("edge:{}:{}:{}", from, label, to),
            "from": from,
            "to": to,
            "label": label,
            "claim_id": claim.get("claim_id").cloned().unwrap_or_default(),
            "claim_type": claim.get("claim_type").cloned().unwrap_or_default(),
            "ring": claim.get("ring").cloned().unwrap_or_default(),
            "render": render,
            "matched_phrases": claim.get("matched_phrases").cloned().unwrap_or_else(|| json!([]))
        }));
    }

    edges
}

pub fn render(payload: &Value, title: &str) -> Value {
    let evidence = evidence_items(payload);
    let nodes = evidence.iter().map(node_from_evidence).collect::<Vec<_>>();
    let edges = edges_from_accepted_claims(payload);

    let mut graph = json!({
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
        "edges": edges
    });

    dedup_graph(graph)
}
