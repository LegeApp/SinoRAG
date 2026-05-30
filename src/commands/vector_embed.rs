use crate::embedding::build::{run_vector_update, VectorUpdateConfig};
use crate::embedding::models::{EmbeddingExecutionProvider, LocalEmbeddingProfile};
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
    tensorrt_root: Option<PathBuf>,
    tensorrt_cache_dir: Option<PathBuf>,
    cpu: bool,
    show_download_progress: bool,
    fail_if_feature_missing: bool,
    allow_partial_vector_index: bool,
    max_nb_connection: usize,
    ef_construction: usize,
    nb_layer: usize,
) -> Result<()> {
    let cache_path = cache.unwrap_or_else(|| default_cache_path(&parquet, profile));
    let batch_size = batch_size.unwrap_or_else(|| profile.default_batch_size());
    let tensorrt_cache_dir =
        tensorrt_cache_dir.or_else(|| default_tensorrt_cache_dir(&cache_path, profile));
    let execution_provider = if cpu {
        EmbeddingExecutionProvider::Cpu
    } else {
        EmbeddingExecutionProvider::Tensorrt
    };

    eprintln!("=== vector-update ===");
    eprintln!("Model:      {}", profile.model_id());
    eprintln!("Dim:        {}", profile.dim());
    eprintln!("Batch size: {}", batch_size);
    eprintln!("Provider:   {:?}", execution_provider);
    eprintln!("Cache:      {}", cache_path.display());
    if let Some(root) = &tensorrt_root {
        eprintln!("TensorRT:   {}", root.display());
    } else {
        eprintln!("TensorRT:   auto (SINORAG_TENSORRT_ROOT / PATH)");
    }
    if let Some(cache_dir) = &tensorrt_cache_dir {
        eprintln!("TRT cache:  {}", cache_dir.display());
    }
    eprintln!("Out:        {}", out.display());

    let config = VectorUpdateConfig {
        parquet_path: parquet,
        doc_table_path: doc_table,
        cache_path,
        vector_out: out.clone(),
        profile,
        batch_size,
        model_cache_dir,
        tensorrt_root,
        tensorrt_cache_dir,
        execution_provider,
        show_download_progress,
        fail_if_feature_missing,
        allow_partial_vector_index,
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

fn default_tensorrt_cache_dir(
    cache_path: &Path,
    profile: LocalEmbeddingProfile,
) -> Option<PathBuf> {
    let base = cache_path.parent()?;
    Some(base.join("tensorrt").join(profile.cache_slug()))
}
