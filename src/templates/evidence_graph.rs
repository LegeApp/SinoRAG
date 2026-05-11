//! Evidence-graph draft — `readzen-text-reuse-graph-draft-v1`.

use super::{evidence_items, node_from_evidence, query_raw, stable_id};
use serde_json::{json, Value};

pub fn render(payload: &Value, title: &str) -> Value {
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
