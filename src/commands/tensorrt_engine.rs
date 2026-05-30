use crate::embedding::models::LocalEmbeddingProfile;
use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
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

pub fn build(
    onnx: PathBuf,
    engine_dir: PathBuf,
    profile: LocalEmbeddingProfile,
    batch_size: Option<usize>,
    trtexec: PathBuf,
    force: bool,
) -> Result<()> {
    if profile != LocalEmbeddingProfile::BgeSmallZhV15 {
        anyhow::bail!("direct TensorRT engine build currently supports bge-small-zh-v1.5 only");
    }
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

fn sha256_file(path: &std::path::Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let digest = Sha256::digest(bytes);
    Ok(hex::encode(digest))
}
