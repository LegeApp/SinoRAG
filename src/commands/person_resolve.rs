use crate::datafusion_store::DataFusionStore;
use crate::jsonout::write_or_print;
use crate::registry;
use crate::research::{
    base_payload, default_registry_for, evidence_from_row, exact_phrase_rows, SearchSpec,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::PathBuf;

pub async fn run(
    name: String,
    aliases: Vec<String>,
    parquet_path: PathBuf,
    out: Option<PathBuf>,
) -> Result<()> {
    let store = DataFusionStore::open(&parquet_path).await?;
    let mut forms = vec![name.clone()];
    for alias in aliases {
        if !forms.iter().any(|v| v == &alias) {
            forms.push(alias);
        }
    }

    let mut candidates = Vec::new();
    let mut evidence = Vec::new();
    for form in &forms {
        let spec = SearchSpec::exact_phrase(form.clone(), 50);
        let rows = exact_phrase_rows(&store, &spec).await?;
        if let Some(first) = rows.first() {
            evidence.push(evidence_from_row(first, form, "name_form_sample"));
        }
        candidates.push(json!({
            "form": form,
            "normalized": spec.normalized,
            "hit_count_sample": rows.len(),
            "first_hit": rows.first().cloned().unwrap_or(Value::Null),
            "ambiguity": if form.chars().count() <= 1 { "high" } else { "unknown" }
        }));
    }

    let payload = base_payload(
        "readzen-person-resolve-v1",
        json!({
            "raw": name,
            "aliases": forms.iter().skip(1).cloned().collect::<Vec<_>>(),
            "query_type": "person_resolve"
        }),
        json!({
            "command": "person-resolve",
            "policy": "Resolve by supplied names and aliases only; no external authority table is consulted."
        }),
        json!({
            "canonical_candidate": forms.first().cloned().unwrap_or_default(),
            "name_forms": candidates,
            "ambiguity_notes": [
                "Short aliases and honorific titles may refer to more than one person.",
                "Use person-history to inspect earliest and contextualized mentions."
            ]
        }),
        json!(evidence),
        vec![
            "This is a corpus-local resolver, not a historical authority file.",
            "Aliases must be supplied explicitly until a persons/aliases table is added.",
        ],
        "low",
        vec!["Run person-history with the same aliases to classify mention contexts."],
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
