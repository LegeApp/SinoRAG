//! `collocation-search`: find terms that co-occur near a seed phrase
//! more often than expected by chance. Uses n-gram hashes from the
//! text_analyzer to score collocates within a window.

use crate::datafusion_store::DataFusionStore;
use crate::document_table::DocumentTable;
use crate::jsonout::write_or_print;
use crate::normalize::normalize_zh;
use crate::research_tools::phrase::phrase_rows_with_explicit_doc_table;
use crate::research_tools::stats::score_collocates;
use crate::text_analyzer::{analyze, AnalyzeOptions, AnalyzeScratch, FilterMode};
use anyhow::Result;
use rustc_hash::FxHashMap;
use serde_json::json;
use std::path::PathBuf;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    parquet: PathBuf,
    phrase_index: Option<PathBuf>,
    doc_table_path: PathBuf,
    phrase: String,
    window_chars: usize,
    gram_len: usize,
    limit_total: usize,
    limit_collocates: usize,
    out: Option<PathBuf>,
) -> Result<()> {
    let doc_table = DocumentTable::load(&doc_table_path)?;
    let store = DataFusionStore::open(&parquet).await?;

    let (hits, phrase_strategy) = phrase_rows_with_explicit_doc_table(
        &store,
        &doc_table,
        phrase_index.as_deref(),
        &phrase,
        limit_total,
        None,
        None,
        None,
    )
    .await?;

    let normalized_phrase = normalize_zh(&phrase);

    let analyze_opts = AnalyzeOptions {
        min_n: gram_len,
        max_n: gram_len,
        filter: FilterMode::WhitespaceOnly,
        apply_low_value_filter: false,
        dedup: true,
        count_tf: false,
    };
    let mut scratch = AnalyzeScratch::new();

    // Collect n-gram hashes from the window around each phrase occurrence.
    let mut near_counts: FxHashMap<u64, u32> = FxHashMap::default();
    let mut background_counts: FxHashMap<u64, u32> = FxHashMap::default();
    let mut near_total = 0u32;
    let mut bg_total = 0u32;

    for row in &hits {
        let norm = row
            .get("zh_text_normalized")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Background: all n-grams in the passage.
        analyze(norm, &analyze_opts, &mut scratch);
        for &h in &scratch.unique {
            *background_counts.entry(h).or_insert(0) += 1;
            bg_total += 1;
        }

        // Near: n-grams within the window around each occurrence.
        if let Some(byte_pos) = norm.find(&normalized_phrase) {
            let char_pos = norm[..byte_pos].chars().count();
            let phrase_chars = normalized_phrase.chars().count();
            let window_start = char_pos.saturating_sub(window_chars);
            let window_end = (char_pos + phrase_chars + window_chars).min(norm.chars().count());

            let window_text: String = norm
                .chars()
                .skip(window_start)
                .take(window_end - window_start)
                .collect();
            analyze(&window_text, &analyze_opts, &mut scratch);
            for &h in &scratch.unique {
                *near_counts.entry(h).or_insert(0) += 1;
                near_total += 1;
            }
        }
    }

    let collocates = score_collocates(&near_counts, &background_counts, limit_collocates);

    let payload = json!({
        "schema": "sinoragd-collocation-search-v1",
        "phrase": phrase,
        "window_chars": window_chars,
        "gram_len": gram_len,
        "total_passages": hits.len(),
        "near_ngram_count": near_total,
        "background_ngram_count": bg_total,
        "collocates": collocates.iter().map(|c| json!({
            "term_hash": c.term_hash,
            "score": c.score,
            "near_count": c.near_count,
            "background_count": c.background_count,
        })).collect::<Vec<_>>(),
        "search_strategy": {
            "phrase": phrase_strategy,
            "limit_total": limit_total,
            "limit_collocates": limit_collocates,
        }
    });
    write_or_print(&payload, out)
}
