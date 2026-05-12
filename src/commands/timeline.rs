use crate::datafusion_store::DataFusionStore;
use crate::jsonout::write_or_print;
use crate::registry;
use crate::research::{
    base_payload, default_registry_for, evidence_from_row, exact_phrase_rows_with_index, SearchSpec,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub async fn run(
    phrase: String,
    parquet_path: PathBuf,
    include_variants: bool,
    limit: usize,
    phrase_index: Option<PathBuf>,
    out: Option<PathBuf>,
) -> Result<()> {
    let store = DataFusionStore::open(&parquet_path).await?;
    let spec = SearchSpec::exact_phrase(phrase.clone(), limit);
    let rows = exact_phrase_rows_with_index(&store, &spec, phrase_index.as_deref()).await?;
    let buckets = timeline_buckets(&rows, &phrase);
    let evidence = rows
        .iter()
        .take(12)
        .map(|row| evidence_from_row(row, &phrase, "timeline_representative"))
        .collect::<Vec<_>>();

    let payload = base_payload(
        "readzen-timeline-v1",
        json!({
            "raw": phrase,
            "normalized": spec.normalized,
            "query_type": "timeline",
            "include_variants": include_variants
        }),
        json!({
            "command": "timeline",
            "match_type": "normalized exact phrase containment",
            "search_backend": if phrase_index.is_some() { "phrase_index_verified_by_datafusion" } else { "datafusion_strpos" },
            "variant_policy": "include-variants is recorded but v1 does not accept generated variants as evidence",
            "ordering": ["period_rank", "source_rel_path", "from_lb", "xml_id"],
            "limit": limit.max(1)
        }),
        json!({
            "bucket_count": buckets.len(),
            "buckets": buckets,
            "returned_count": rows.len()
        }),
        json!(evidence),
        vec![
            "Timeline buckets follow corpus metadata period labels.",
            "Variant wording may predate the exact phrase.",
        ],
        if rows.is_empty() { "none" } else { "medium" },
        vec!["Run first-attestation for the earliest exact loaded-corpus hit."],
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

fn timeline_buckets(rows: &[Value], phrase: &str) -> Vec<Value> {
    let mut buckets: BTreeMap<(i64, String), (usize, Option<Value>)> = BTreeMap::new();
    for row in rows {
        let rank = row
            .get("period_rank")
            .and_then(|v| v.as_i64())
            .unwrap_or(99);
        let period = row
            .get("period")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("Unknown")
            .to_string();
        let entry = buckets.entry((rank, period)).or_insert((0, None));
        entry.0 += 1;
        if entry.1.is_none() {
            entry.1 = Some(evidence_from_row(row, phrase, "bucket_representative"));
        }
    }
    buckets
        .into_iter()
        .map(
            |((period_rank, period), (count, representative_evidence))| {
                json!({
                    "period": period,
                    "period_rank": period_rank,
                    "count": count,
                    "representative_evidence": representative_evidence
                })
            },
        )
        .collect()
}
