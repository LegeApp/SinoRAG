//! Markdown report template — produces a `graphdiscovery-report-md-v1`
//! frontmatter + sectioned scaffold from an evidence payload.

use super::{default_title, evidence_items, query_raw};
use chrono::Utc;
use serde_json::Value;

pub fn render(payload: &Value, title_override: Option<&str>) -> String {
    let title = title_override
        .map(ToString::to_string)
        .or_else(|| payload.get("title").and_then(Value::as_str).map(ToString::to_string))
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
                item.get("source_rel_path").and_then(Value::as_str).unwrap_or("")
            ));
            if let Some(lb) = item.get("lb_range").and_then(Value::as_str).filter(|v| !v.is_empty()) {
                out.push_str(&format!(" ({lb})"));
            }
            out.push('\n');
            for key in ["main_title", "author", "period", "canon", "source_corpus", "rights_id"] {
                if let Some(value) = item.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()) {
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
