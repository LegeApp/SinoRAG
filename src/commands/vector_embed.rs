use crate::embedding::build::{run_vector_update, VectorUpdateConfig};
use crate::embedding::models::LocalEmbeddingProfile;
use crate::vector_index::HnswParams;
use anyhow::Result;
use std::path::{Path, PathBuf};

#[allow(clippy::too_many_arguments)]
pub async fn update(
    parquet: PathBuf,
    doc_table: PathBuf,
    profile: LocalEmbeddingProfile,
    cache: Option<PathBuf>,
    out: PathBuf,
    batch_size: Option<usize>,
    model_cache_dir: Option<PathBuf>,
    show_download_progress: bool,
    fail_if_feature_missing: bool,
    max_nb_connection: usize,
    ef_construction: usize,
    nb_layer: usize,
) -> Result<()> {
    let cache_path = cache.unwrap_or_else(|| default_cache_path(&parquet, profile));
    let batch_size = batch_size.unwrap_or_else(|| profile.default_batch_size());

    eprintln!("=== vector-update ===");
    eprintln!("Model:      {}", profile.model_id());
    eprintln!("Dim:        {}", profile.dim());
    eprintln!("Batch size: {}", batch_size);
    eprintln!("Cache:      {}", cache_path.display());
    eprintln!("Out:        {}", out.display());

    let config = VectorUpdateConfig {
        parquet_path: parquet,
        doc_table_path: doc_table,
        cache_path,
        vector_out: out.clone(),
        profile,
        batch_size,
        model_cache_dir,
        show_download_progress,
        fail_if_feature_missing,
        hnsw: HnswParams {
            max_nb_connection,
            ef_construction,
            nb_layer,
        },
    };

    run_vector_update(config).await?;

    // Only print index info if the output was actually created
    if out.exists() {
        let info = crate::vector_index::VectorIndex::header_info(&out)?;
        println!("{}", serde_json::to_string_pretty(&info)?);
    }
    Ok(())
}

fn default_cache_path(parquet: &Path, profile: LocalEmbeddingProfile) -> PathBuf {
    // parquet is typically "data/passages.parquet" (a directory); parent is "data/"
    let data_dir = parquet.parent().unwrap_or(Path::new("data"));
    data_dir.join("derived").join(profile.cache_filename())
}
