//! Lineage-graph draft — `readzen-lineage-graph-draft-v1`. Person extraction
//! is conservative and rule-based; not authority control.

use super::{dedup_graph, evidence_items, extract_names, node_from_evidence, stable_id, with_layout};
use serde_json::{json, Value};

pub fn render(payload: &Value, title: &str) -> Value {
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
        let passage_id = passage_node.get("id").and_then(Value::as_str).unwrap_or("").to_string();
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
