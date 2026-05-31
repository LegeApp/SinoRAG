use crate::embedding::models::LocalEmbeddingProfile;
use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Serialize)]
struct TensorRtEngineBuildMetadata {
    schema: &'static str,
    model_id: &'static str,
    model: &'static str,
    onnx_path: String,
    onnx_sha256: String,
    engine_path: String,
    seq_len: usize,
    min_batch: usize,
    opt_batch: usize,
    max_batch: usize,
    precision: &'static str,
    input_dtype: &'static str,
    output_dtype: &'static str,
    output_kind: &'static str,
    dim: usize,
}

/// Ensure the TensorRT engine for `profile` is built in `engine_dir`.
/// Downloads the ONNX from HuggingFace if not already cached. No-ops if the engine exists.
#[cfg(feature = "local-embeddings")]
pub fn ensure_engine_ready(
    engine_dir: &Path,
    profile: LocalEmbeddingProfile,
    batch_size: Option<usize>,
    trtexec: &Path,
    model_cache_dir: Option<&Path>,
    show_download_progress: bool,
) -> Result<()> {
    let engine_path = engine_dir.join("engine.plan");
    let metadata_path = engine_dir.join("build.json");
    if engine_path.is_file() && metadata_path.is_file() {
        return Ok(());
    }
    eprintln!(
        "       TensorRT engine not found in {}; fetching ONNX model...",
        engine_dir.display()
    );
    let onnx = download_onnx(profile, model_cache_dir, show_download_progress)?;
    build(
        Some(onnx),
        engine_dir.to_path_buf(),
        profile,
        batch_size,
        trtexec.to_path_buf(),
        false,
        model_cache_dir.map(|p| p.to_path_buf()),
        show_download_progress,
    )
}

/// Download the ONNX model for `profile` from HuggingFace and return its local path.
#[cfg(feature = "local-embeddings")]
fn download_onnx(
    profile: LocalEmbeddingProfile,
    model_cache_dir: Option<&Path>,
    show_download_progress: bool,
) -> Result<PathBuf> {
    use hf_hub::api::sync::ApiBuilder;

    let cache_dir = model_cache_dir
        .map(|p| p.to_path_buf())
        .or_else(|| std::env::var("FASTEMBED_CACHE_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(".fastembed_cache"));

    let endpoint =
        std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string());

    let api = ApiBuilder::new()
        .with_cache_dir(cache_dir)
        .with_endpoint(endpoint)
        .with_progress(show_download_progress)
        .build()
        .context("building HuggingFace Hub API client")?;

    let repo = api.model(profile.onnx_hf_repo().to_string());
    let path = repo.get(profile.onnx_hf_file()).with_context(|| {
        format!(
            "downloading {} from {}",
            profile.onnx_hf_file(),
            profile.onnx_hf_repo()
        )
    })?;
    eprintln!("       ONNX cached at {}", path.display());
    Ok(path)
}

pub fn build(
    onnx: Option<PathBuf>,
    engine_dir: PathBuf,
    profile: LocalEmbeddingProfile,
    batch_size: Option<usize>,
    trtexec: PathBuf,
    force: bool,
    model_cache_dir: Option<PathBuf>,
    show_download_progress: bool,
) -> Result<()> {
    if profile != LocalEmbeddingProfile::BgeSmallZhV15 {
        anyhow::bail!("direct TensorRT engine build currently supports bge-small-zh-v1.5 only");
    }

    let onnx = match onnx {
        Some(p) => p,
        None => {
            #[cfg(feature = "local-embeddings")]
            {
                download_onnx(profile, model_cache_dir.as_deref(), show_download_progress)?
            }
            #[cfg(not(feature = "local-embeddings"))]
            {
                let _ = (model_cache_dir, show_download_progress);
                anyhow::bail!(
                    "no --onnx path provided and auto-download requires the local-embeddings \
                     feature; provide --onnx <model.onnx> or rebuild with --features local-embeddings"
                )
            }
        }
    };

    if !onnx.is_file() {
        anyhow::bail!("ONNX model not found: {}", onnx.display());
    }
    std::fs::create_dir_all(&engine_dir)?;
    let engine_path = engine_dir.join("engine.plan");
    let metadata_path = engine_dir.join("build.json");
    if engine_path.exists() && metadata_path.exists() && !force {
        anyhow::bail!(
            "{} already exists; pass --force to rebuild",
            engine_path.display()
        );
    }

    let batch = batch_size
        .unwrap_or_else(|| profile.default_batch_size())
        .max(1);
    let seq_len = 512usize;
    let shapes = |batch: usize| {
        format!(
            "input_ids:{batch}x{seq_len},attention_mask:{batch}x{seq_len},token_type_ids:{batch}x{seq_len}"
        )
    };

    eprintln!(
        "       Running trtexec: {} -> {}",
        onnx.display(),
        engine_path.display()
    );
    let status = Command::new(&trtexec)
        .arg(format!("--onnx={}", onnx.display()))
        .arg(format!("--saveEngine={}", engine_path.display()))
        .arg("--builderOptimizationLevel=3")
        .arg(format!("--minShapes={}", shapes(1)))
        .arg(format!("--optShapes={}", shapes(batch)))
        .arg(format!("--maxShapes={}", shapes(batch)))
        .status()
        .with_context(|| format!("running {}", trtexec.display()))?;
    if !status.success() {
        anyhow::bail!("trtexec failed with status {status}");
    }

    let onnx_sha256 = sha256_file(&onnx)?;
    let metadata = TensorRtEngineBuildMetadata {
        schema: "sinorag-tensorrt-engine-build-v1",
        model_id: profile.model_id(),
        model: profile.cache_slug(),
        onnx_path: onnx.display().to_string(),
        onnx_sha256,
        engine_path: engine_path.display().to_string(),
        seq_len,
        min_batch: 1,
        opt_batch: batch,
        max_batch: batch,
        precision: "default",
        input_dtype: "engine-native-i32-or-i64",
        output_dtype: "f32",
        output_kind: "last_hidden_state",
        dim: profile.dim(),
    };
    std::fs::write(&metadata_path, serde_json::to_vec_pretty(&metadata)?)?;
    println!("{}", serde_json::to_string_pretty(&metadata)?);
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let digest = Sha256::digest(bytes);
    Ok(hex::encode(digest))
}
