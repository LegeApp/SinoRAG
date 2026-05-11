use crate::datafusion_store::DataFusionStore;
use crate::jsonout::write_or_print;
use crate::phrase_index::PhraseIndex;
use crate::research::{exact_phrase_rows_with_index, SearchSpec};
use anyhow::Result;
use serde_json::json;
use std::path::PathBuf;

pub fn build(
    parquet: PathBuf,
    doc_table: PathBuf,
    out: PathBuf,
    gram_len: usize,
    buckets: usize,
    temp_dir: Option<PathBuf>,
) -> Result<()> {
    crate::phrase_index::build(parquet, doc_table, out, gram_len, buckets, temp_dir)
}

pub fn info(index_path: PathBuf) -> Result<()> {
    // Header-only read so `info` works on multi-GB indexes without loading them.
    let payload = PhraseIndex::header_info(&index_path)?;
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

pub async fn search(
    parquet_path: PathBuf,
    index_path: PathBuf,
    phrase: String,
    limit: usize,
    out: Option<PathBuf>,
) -> Result<()> {
    if !index_path.exists() {
        anyhow::bail!(
            "phrase index not found at {}. Run `sinoragd phrase-index-build` first.",
            index_path.display()
        );
    }
    let store = DataFusionStore::open(&parquet_path).await?;
    let rows = exact_phrase_rows_with_index(
        &store,
        &SearchSpec::exact_phrase(phrase.clone(), limit),
        Some(index_path.as_path()),
    )
    .await?;
    let payload = json!({
        "schema": "sinorag-phrase-index-search",
        "phrase": phrase,
        "index": index_path.display().to_string(),
        "returned_count": rows.len(),
        "limit": limit.max(1),
        "results": rows,
    });
    write_or_print(&payload, out)
}
