//! ReadZen collection export.

use super::{default_title, evidence_items, format_notes, stable_id};
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub fn render(payload: &Value, name_override: Option<&str>) -> Value {
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
