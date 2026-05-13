use crate::datafusion_store::DataFusionStore;
use crate::jsonout::write_or_print;
use crate::registry;
use crate::research::{
    base_payload, default_registry_for, evidence_from_row, exact_phrase_rows_with_index, SearchSpec,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::PathBuf;

pub async fn run(
    phrase: String,
    parquet_path: PathBuf,
    canon: Vec<String>,
    limit: usize,
    phrase_index: Option<PathBuf>,
    out: Option<PathBuf>,
) -> Result<()> {
    let store = DataFusionStore::open(&parquet_path).await?;
    let all_hits = exact_phrase_rows_with_index(
        &store,
        &SearchSpec::exact_phrase(phrase.clone(), limit),
        phrase_index.as_deref(),
    )
    .await?;
    let canon_filter = if canon.is_empty() {
        vec!["T".to_string()]
    } else {
        canon
    };
    let mut canon_spec = SearchSpec::exact_phrase(phrase.clone(), limit);
    canon_spec.canon = canon_filter.clone();
    let canon_hits =
        exact_phrase_rows_with_index(&store, &canon_spec, phrase_index.as_deref()).await?;

    let source_claim = if let Some(hit) = canon_hits.first() {
        json!({
            "status": "candidate",
            "claim": "Exact phrase occurs in the requested canonical scope.",
            "canon_side_passage": hit,
            "strength": "exact_corpus_hit"
        })
    } else {
        json!({
            "status": "not_supported",
            "claim": "No exact canon-side source claim can be made from this search.",
            "strength": "none"
        })
    };

    let evidence = canon_hits
        .iter()
        .take(8)
        .map(|row| evidence_from_row(row, &phrase, "canon_side_candidate"))
        .chain(
            all_hits
                .iter()
                .take(4)
                .map(|row| evidence_from_row(row, &phrase, "loaded_corpus_hit")),
        )
        .collect::<Vec<_>>();

    let payload = base_payload(
        "readzen-canonical-source-v1",
        json!({
            "raw": phrase,
            "normalized": canon_spec.normalized,
            "query_type": "canonical_source",
            "canon": canon_filter
        }),
        json!({
            "command": "canonical-source",
            "evidence_rule": "Do not assert a source unless an exact canon-side Chinese hit is present.",
            "search_backend": if phrase_index.is_some() { "phrase_index_verified_by_datafusion" } else { "datafusion_strpos" },
            "default_canon": "T when --canon is omitted",
            "limit": limit.max(1)
        }),
        json!({
            "source_claim": source_claim,
            "earliest_loaded_hit": all_hits.first().cloned().unwrap_or(Value::Null),
            "canon_side_candidates": canon_hits,
            "loaded_corpus_sample": all_hits.iter().take(10).cloned().collect::<Vec<_>>()
        }),
        json!(evidence),
        vec![
            "A canon-side exact hit is evidence for occurrence, not proof of absolute origin.",
            "Commentarial or dictionary texts can preserve citations but are not automatically the source.",
            "Cross-corpus literary allusion checks require additional classical literature corpora.",
        ],
        if canon_hits.is_empty() { "none" } else { "medium" },
        vec![
            "Inspect the canon-side passage and surrounding context.",
            "Run phrase-history to compare later reuse distribution.",
        ],
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
