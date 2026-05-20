//! Basic PDF report template for the Lopdf-backed bilingual renderer.
//!
//! This template converts structured report/evidence JSON into paired
//! Chinese/English sections consumed by `cbeta-pdf-creator`.

use super::{default_title, evidence_items, query_raw};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfSections {
    pub chinese: Vec<String>,
    pub english: Vec<String>,
}

pub fn render(
    payload: &Value,
    title_override: Option<&str>,
    essay_max_pages: usize,
) -> PdfSections {
    let title = title_override
        .map(ToString::to_string)
        .or_else(|| {
            payload
                .get("title")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| default_title(payload));

    let mut chinese = Vec::new();
    let mut english = Vec::new();

    english.push(format!(
        "{title}\n\nEvidence scaffold report. Maximum essay length hint: up to {} pages.\n\nQuery: {}\nSchema: {}{}",
        essay_max_pages.max(1),
        query_raw(payload),
        payload.get("schema").and_then(Value::as_str).unwrap_or(""),
        payload
            .get("confidence")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|value| format!("\nConfidence: {value}"))
            .unwrap_or_default()
    ));
    chinese.push(String::new());

    if let Some(results) = payload.get("results") {
        english.push(format!(
            "Structured Results\n\n{}",
            serde_json::to_string_pretty(results).unwrap_or_default()
        ));
        chinese.push(String::new());
    }

    let evidence = evidence_items(payload);
    if evidence.is_empty() {
        english.push(
            "Evidence\n\nNo evidence records were present in the input artifact.".to_string(),
        );
        chinese.push(String::new());
    } else {
        for (idx, item) in evidence.iter().enumerate() {
            let mut meta = Vec::new();
            push_meta(&mut meta, "Passage", item, "passage_id");
            push_meta(&mut meta, "Source", item, "source_rel_path");
            push_meta(&mut meta, "Location", item, "lb_range");
            push_meta(&mut meta, "Title", item, "main_title");
            push_meta(&mut meta, "Author", item, "author");
            push_meta(&mut meta, "Period", item, "period");
            push_meta(&mut meta, "Canon", item, "canon");
            push_meta(&mut meta, "Corpus", item, "source_corpus");
            push_meta(&mut meta, "Rights", item, "rights_id");

            english.push(format!("Evidence {}\n\n{}", idx + 1, meta.join("\n")));
            chinese.push(
                item.get("zh_quote")
                    .or_else(|| item.get("quote_zh"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            );
        }
    }

    push_string_array_section(payload, "Caveats", "caveats", &mut chinese, &mut english);
    push_string_array_section(
        payload,
        "Next Steps",
        "next_steps",
        &mut chinese,
        &mut english,
    );

    PdfSections { chinese, english }
}

fn push_meta(out: &mut Vec<String>, label: &str, item: &Value, key: &str) {
    if let Some(value) = item
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        out.push(format!("{label}: {value}"));
    }
}

fn push_string_array_section(
    payload: &Value,
    label: &str,
    key: &str,
    chinese: &mut Vec<String>,
    english: &mut Vec<String>,
) {
    if let Some(values) = payload.get(key).and_then(Value::as_array) {
        let values = values
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .collect::<Vec<_>>();
        if !values.is_empty() {
            english.push(format!("{label}\n\n{}", values.join("\n")));
            chinese.push(String::new());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::render;
    use serde_json::json;

    #[test]
    fn renders_paired_sections_from_evidence_payload() {
        let payload = json!({
            "schema": "test",
            "title": "Sample",
            "query": {"raw": "佛性"},
            "evidence": [{
                "passage_id": "T01#1",
                "main_title": "Test Sutra",
                "zh_quote": "佛性常住"
            }],
            "caveats": ["Corpus-limited."]
        });

        let sections = render(&payload, None, 2);
        assert_eq!(sections.chinese.len(), sections.english.len());
        assert!(sections.english[0].contains("Sample"));
        assert!(sections
            .english
            .iter()
            .any(|section| section.contains("Evidence 1")));
        assert!(sections
            .chinese
            .iter()
            .any(|section| section.contains("佛性常住")));
    }
}
