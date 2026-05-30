use std::{env, path::PathBuf};

use anyhow::{Context, Result};
use fastembed::{EmbeddingBackendConfig, EmbeddingModel, TextEmbedding, TextInitOptions};

fn main() -> Result<()> {
    let engine_dir = env_path("FASTEMBED_TRT_ENGINE_DIR")
        .or_else(|| env_arg("--engine-dir"))
        .context("set FASTEMBED_TRT_ENGINE_DIR or pass --engine-dir <path>")?;
    let cache_dir = env_path("FASTEMBED_CACHE_DIR")
        .or_else(|| env_arg("--cache-dir"))
        .context("set FASTEMBED_CACHE_DIR or pass --cache-dir <path>")?;
    let batch_size = env::var("FASTEMBED_TRT_BATCH")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2);

    let mut model = TextEmbedding::try_new(
        TextInitOptions::new(EmbeddingModel::BGESmallZHV15)
            .with_cache_dir(cache_dir)
            .with_show_download_progress(false)
            .with_backend(EmbeddingBackendConfig::TensorRt {
                engine_dir: Some(engine_dir),
                engine_path: None,
            }),
    )?;

    let texts = [
        "佛教文本向量检索 smoke test",
        "The TensorRT embedding backend should return normalized vectors.",
    ];
    let embeddings = model.embed(texts, Some(batch_size))?;
    for (idx, embedding) in embeddings.iter().enumerate() {
        let norm = embedding.iter().map(|value| value * value).sum::<f32>().sqrt();
        let preview = embedding.iter().take(4).copied().collect::<Vec<_>>();
        println!(
            "embedding[{idx}] dim={} norm={norm:.6} first4={preview:?}",
            embedding.len()
        );
    }
    Ok(())
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name).map(PathBuf::from)
}

fn env_arg(name: &str) -> Option<PathBuf> {
    let mut args = env::args_os();
    while let Some(arg) = args.next() {
        if arg == name {
            return args.next().map(PathBuf::from);
        }
    }
    None
}
