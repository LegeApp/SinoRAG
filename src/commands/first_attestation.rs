use crate::datafusion_store::DataFusionStore;
use crate::jsonout::write_or_print;
use crate::registry;
use crate::research::{
    base_payload, default_registry_for, distribution, evidence_from_row,
    exact_phrase_rows_with_index, tradition_distribution, SearchSpec,
};
use anyhow::Result;
use serde_json::json;
use std::path::PathBuf;

pub async fn first_attestation(
    phrase: String,
    store: &DataFusionStore,
    limit: usize,
    phrase_index: Option<PathBuf>,
) -> Result<serde_json::Value> {
    let spec = SearchSpec::exact_phrase(phrase.clone(), limit);
    let rows = exact_phrase_rows_with_index(store, &spec, phrase_index.as_deref()).await?;
    let earliest_exact = rows.first().cloned();
    let top_exact_hits = rows.iter().take(10).cloned().collect::<Vec<_>>();
    let evidence = earliest_exact
        .as_ref()
        .map(|row| vec![evidence_from_row(row, &phrase, "earliest_exact")])
        .unwrap_or_default();

    let hits_by_period = distribution(&rows, "period");
    let hits_by_canon = distribution(&rows, "canon");
    let hits_by_tradition = tradition_distribution(&rows);

    let source_fingerprint = store.source_fingerprint();
    let mut payload = base_payload(
        "readzen-first-attestation-v2",
        json!({
            "raw": phrase,
            "normalized": spec.normalized,
            "query_type": "exact_phrase"
        }),
        json!({
            "command": "first-attestation",
            "match_type": "normalized exact phrase containment",
            "search_backend": if phrase_index.is_some() { "phrase_index_verified_by_datafusion" } else { "datafusion_strpos" },
            "ordering": ["period_rank", "source_rel_path", "from_lb", "xml_id"],
            "limit": limit.max(1)
        }),
        json!({
            "earliest_exact": earliest_exact,
            "top_exact_hits": top_exact_hits,
            "returned_count": rows.len(),
            "hits_by_period": hits_by_period,
            "hits_by_canon": hits_by_canon,
            "hits_by_tradition": hits_by_tradition
        }),
        json!(evidence),
        vec![
            "Date metadata is corpus metadata, not proof of composition date.",
            "Variant wording may predate the exact phrase.",
            "Chinese classical literary allusion checks are incomplete until non-Buddhist corpora are loaded.",
        ],
        if rows.is_empty() { "none" } else { "medium" },
        vec![
            "Run phrase-history to inspect distribution and later reuse.",
            "Run similar-phrase to look for conservative partial or lexical neighbors.",
        ],
        source_fingerprint,
    );

    if let Some(obj) = payload.as_object_mut() {
        let legacy = obj
            .get("results")
            .and_then(|v| v.as_object())
            .map(|results| {
                [
                    (
                        "earliest_exact",
                        results.get("earliest_exact").cloned().unwrap_or_default(),
                    ),
                    (
                        "top_exact_hits",
                        results.get("top_exact_hits").cloned().unwrap_or_default(),
                    ),
                    (
                        "hits_by_period",
                        results.get("hits_by_period").cloned().unwrap_or_default(),
                    ),
                    (
                        "hits_by_canon",
                        results.get("hits_by_canon").cloned().unwrap_or_default(),
                    ),
                    (
                        "hits_by_tradition",
                        results
                            .get("hits_by_tradition")
                            .cloned()
                            .unwrap_or_default(),
                    ),
                ]
            });
        if let Some(legacy) = legacy {
            for (key, value) in legacy {
                obj.insert(key.to_string(), value);
            }
        }
    }
    
    Ok(payload)
}

pub async fn run(
    phrase: String,
    parquet_path: PathBuf,
    limit: usize,
    phrase_index: Option<PathBuf>,
    out: Option<PathBuf>,
) -> Result<()> {
    let store = DataFusionStore::open(&parquet_path).await?;
    let payload = first_attestation(phrase, &store, limit, phrase_index).await?;

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
