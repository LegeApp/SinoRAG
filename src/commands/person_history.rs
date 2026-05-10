use crate::datafusion_store::DataFusionStore;
use crate::jsonout::write_or_print;
use crate::registry;
use crate::research::{
    base_payload, default_registry_for, evidence_from_row, exact_phrase_rows, field_str, SearchSpec,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub async fn run(
    name: String,
    aliases: Vec<String>,
    parquet_path: PathBuf,
    limit: usize,
    out: Option<PathBuf>,
) -> Result<()> {
    let store = DataFusionStore::open(&parquet_path).await?;
    let mut forms = vec![name.clone()];
    for alias in aliases {
        if !forms.iter().any(|v| v == &alias) {
            forms.push(alias);
        }
    }

    let mut by_passage: BTreeMap<String, Value> = BTreeMap::new();
    for (idx, form) in forms.iter().enumerate() {
        let rows = exact_phrase_rows(&store, &SearchSpec::exact_phrase(form.clone(), limit)).await?;
        for mut row in rows {
            let passage_id = field_str(&row, "passage_id");
            let mention_class = classify_mention(&row, form);
            if let Some(obj) = row.as_object_mut() {
                obj.insert("matched_name_form".to_string(), json!(form));
                obj.insert("matched_name_forms".to_string(), json!([form]));
                obj.insert("is_primary_name".to_string(), json!(idx == 0));
                obj.insert("mention_class".to_string(), json!(mention_class));
                obj.insert(
                    "ambiguity".to_string(),
                    json!(if idx == 0 {
                        "unambiguous_candidate"
                    } else {
                        "alias_candidate"
                    }),
                );
            }
            match by_passage.entry(passage_id) {
                Entry::Vacant(entry) => {
                    entry.insert(row);
                }
                Entry::Occupied(mut entry) => {
                    merge_name_form(entry.get_mut(), form, idx == 0);
                }
            }
        }
    }

    let mut mentions = by_passage.into_values().collect::<Vec<_>>();
    mentions.sort_by_key(sort_key);
    mentions.truncate(limit.max(1));

    let earliest_unambiguous = mentions
        .iter()
        .find(|row| row.get("is_primary_name").and_then(|v| v.as_bool()) == Some(true))
        .cloned()
        .unwrap_or(Value::Null);
    let ambiguous_earlier_hits = mentions
        .iter()
        .filter(|row| row.get("is_primary_name").and_then(|v| v.as_bool()) != Some(true))
        .take(5)
        .cloned()
        .collect::<Vec<_>>();
    let evidence = mentions
        .iter()
        .take(12)
        .map(|row| {
            let form = row
                .get("matched_name_form")
                .and_then(|v| v.as_str())
                .unwrap_or(&name);
            evidence_from_row(row, form, "person_mention")
        })
        .collect::<Vec<_>>();

    let payload = base_payload(
        "readzen-person-history-v1",
        json!({
            "raw": name,
            "aliases": forms.iter().skip(1).cloned().collect::<Vec<_>>(),
            "query_type": "person_history"
        }),
        json!({
            "command": "person-history",
            "classification": "rule-based v1 labels from local passage context",
            "ordering": ["period_rank", "source_rel_path", "from_lb", "xml_id"],
            "limit": limit.max(1)
        }),
        json!({
            "canonical_candidate": forms.first().cloned().unwrap_or_default(),
            "mentions": mentions,
            "earliest_unambiguous": earliest_unambiguous,
            "ambiguous_earlier_hits": ambiguous_earlier_hits
        }),
        json!(evidence),
        vec![
            "Rule-based mention classes are triage labels, not accepted historical claims.",
            "Alias hits may refer to more than one person.",
        ],
        "low",
        vec!["Review earliest hits manually before asserting a first mention."],
        store.source_fingerprint(),
    );

    let registry_path = default_registry_for(&parquet_path);
    let _ = registry::record_payload(
        &registry_path,
        "semantic_research",
        &payload,
        out.as_deref(),
        "",
        payload
            .get("query")
            .and_then(|q| q.get("raw"))
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    write_or_print(&payload, out)
}

fn merge_name_form(row: &mut Value, form: &str, is_primary: bool) {
    let Some(obj) = row.as_object_mut() else {
        return;
    };
    if is_primary {
        obj.insert("is_primary_name".to_string(), json!(true));
        obj.insert("matched_name_form".to_string(), json!(form));
        obj.insert("ambiguity".to_string(), json!("unambiguous_candidate"));
    }
    let entry = obj
        .entry("matched_name_forms".to_string())
        .or_insert_with(|| json!([]));
    if let Some(forms) = entry.as_array_mut() {
        if !forms.iter().any(|v| v.as_str() == Some(form)) {
            forms.push(json!(form));
        }
    }
}

fn classify_mention(row: &Value, form: &str) -> &'static str {
    let text = row
        .get("zh_text_raw")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if text.contains("嗣")
        || text.contains("法嗣")
        || text.contains("傳法")
        || text.contains("弟子")
    {
        "lineage_relation"
    } else if text.contains(form)
        && (text.contains("云") || text.contains("曰") || text.contains("示"))
    {
        "attributed_saying"
    } else if row.get("text_type").and_then(|v| v.as_str()) == Some("dialogue") {
        "case_appearance"
    } else if text.contains("頌") || text.contains("評") || text.contains("拈") {
        "commentarial_reference"
    } else {
        "name_mention"
    }
}

fn sort_key(row: &Value) -> (i64, String, String, String) {
    (
        row.get("period_rank")
            .and_then(|v| v.as_i64())
            .unwrap_or(99),
        field_str(row, "source_rel_path"),
        field_str(row, "from_lb"),
        field_str(row, "xml_id"),
    )
}
