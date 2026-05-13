//! Timeline-graph draft — `readzen-timeline-graph-draft-v1`.

use super::{evidence_items, node_from_evidence, stable_id, with_layout};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub fn render(payload: &Value, title: &str) -> Value {
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
