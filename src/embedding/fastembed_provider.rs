#[cfg(feature = "local-embeddings")]
use super::models::EmbeddingExecutionProvider;
#[cfg(feature = "local-embeddings")]
use super::models::LocalEmbeddingProfile;
#[cfg(feature = "local-embeddings")]
use super::provider::{EmbeddingInput, EmbeddingProvider, EmbeddingRow};
#[cfg(feature = "local-embeddings")]
use anyhow::Result;
#[cfg(feature = "local-embeddings-tensorrt")]
use std::sync::OnceLock;

#[cfg(feature = "local-embeddings-tensorrt")]
static TENSORRT_PLUGIN_REGISTRATION: OnceLock<std::result::Result<std::path::PathBuf, String>> =
    OnceLock::new();

#[cfg(feature = "local-embeddings")]
pub struct FastEmbedProvider {
    model_id: String,
    model_revision: Option<String>,
    model: fastembed::TextEmbedding,
    dim: usize,
    document_prefix: &'static str,
    query_prefix: &'static str,
    batch_size: usize,
}

#[cfg(feature = "local-embeddings")]
impl FastEmbedProvider {
    pub fn new(
        profile: LocalEmbeddingProfile,
        cache_dir: Option<std::path::PathBuf>,
        batch_size: usize,
        execution_provider: EmbeddingExecutionProvider,
        tensorrt_root: Option<std::path::PathBuf>,
        tensorrt_cache_dir: Option<std::path::PathBuf>,
        show_download_progress: bool,
    ) -> Result<Self> {
        use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

        let model_enum = match profile {
            LocalEmbeddingProfile::BgeSmallZhV15 => EmbeddingModel::BGESmallZHV15,
            LocalEmbeddingProfile::BgeM3 => EmbeddingModel::BGEM3,
        };

        let mut opts =
            TextInitOptions::new(model_enum).with_show_download_progress(show_download_progress);
        if let Some(dir) = cache_dir {
            opts = opts.with_cache_dir(dir);
        }
        let dylib_hint = std::env::var("ORT_DYLIB_PATH")
            .unwrap_or_else(|_| "onnxruntime.dll (default search)".to_string());
        eprintln!("       ONNX Runtime dylib: {}", dylib_hint);

        let providers = execution_providers(
            execution_provider,
            profile,
            batch_size,
            tensorrt_root,
            tensorrt_cache_dir,
        )?;
        if plugin_tensorrt_enabled(execution_provider) {
            eprintln!("       Execution provider: TensorRTEp plugin");
        } else {
            // providers is empty only if TensorRT plugin registration failed but
            // error_on_failure was false; the caller already printed a warning.
            opts = opts.with_execution_providers(providers);
        }

        let model = TextEmbedding::try_new(opts).map_err(|e| {
            let msg = e.to_string();
            let mut hint = String::new();
            if msg.contains("onnxruntime") || msg.contains("ONNX") || msg.contains("dylib") || msg.contains("load") {
                hint.push_str("\n\nHints:\n");
                hint.push_str("  - Set ORT_DYLIB_PATH to a working onnxruntime.dll, or place one next to the executable.\n");
                #[cfg(feature = "local-embeddings-tensorrt")]
                hint.push_str("  - Set SINORAG_TENSORRT_EP_DLL to ORTTensorRTEp.dll; make TensorRT/CUDA DLLs visible via --tensorrt-root, SINORAG_TENSORRT_ROOT, or PATH.\n");
                #[cfg(not(feature = "local-embeddings-tensorrt"))]
                hint.push_str("  - Rebuild with --features local-embeddings-tensorrt to enable TensorRT.\n");
            }
            anyhow::anyhow!("failed to initialize fastembed model {}: {}{}", profile.model_id(), msg, hint)
        })?;

        Ok(Self {
            model_id: profile.model_id().to_string(),
            model_revision: None,
            model,
            dim: profile.dim(),
            document_prefix: profile.document_prefix(),
            query_prefix: profile.query_prefix(),
            batch_size,
        })
    }
}

#[cfg(feature = "local-embeddings")]
fn plugin_tensorrt_enabled(_execution_provider: EmbeddingExecutionProvider) -> bool {
    std::env::var_os("SINORAG_USE_TENSORRT_PLUGIN_EP").is_some()
}

#[cfg(feature = "local-embeddings")]
fn execution_providers(
    _execution_provider: EmbeddingExecutionProvider,
    profile: LocalEmbeddingProfile,
    batch_size: usize,
    tensorrt_root: Option<std::path::PathBuf>,
    tensorrt_cache_dir: Option<std::path::PathBuf>,
) -> Result<Vec<fastembed::ExecutionProviderDispatch>> {
    let providers = Vec::new();
    #[cfg(feature = "local-embeddings-tensorrt")]
    {
        register_tensorrt_plugin(profile, batch_size, tensorrt_root, tensorrt_cache_dir, true)?;
    }
    #[cfg(not(feature = "local-embeddings-tensorrt"))]
    {
        let _ = (profile, batch_size, tensorrt_root, tensorrt_cache_dir);
        anyhow::bail!(
            "TensorRT is required for vector embedding but this binary was not built with \
             --features local-embeddings-tensorrt. Rebuild with that feature flag."
        );
    }
    Ok(providers)
}

#[cfg(feature = "local-embeddings-tensorrt")]
fn register_tensorrt_plugin(
    profile: LocalEmbeddingProfile,
    batch_size: usize,
    tensorrt_root: Option<std::path::PathBuf>,
    tensorrt_cache_dir: Option<std::path::PathBuf>,
    error_on_failure: bool,
) -> Result<()> {
    let root = resolve_tensorrt_root(tensorrt_root);
    if let Some(root) = &root {
        add_tensorrt_to_path(root);
        eprintln!("       TensorRT root: {}", root.display());
    } else {
        eprintln!("       TensorRT root: auto (PATH)");
    }

    let cache_dir = tensorrt_cache_dir.unwrap_or_else(|| {
        std::path::PathBuf::from("data/derived/tensorrt").join(profile.cache_slug())
    });
    std::fs::create_dir_all(&cache_dir)?;
    let timing_cache = cache_dir.join("timing.cache");
    eprintln!("       TensorRT engine cache: {}", cache_dir.display());

    let plugin = register_tensorrt_plugin_library()?;
    eprintln!("       TensorRT plugin EP: {}", plugin.display());

    std::env::set_var("SINORAG_USE_TENSORRT_PLUGIN_EP", "1");
    set_plugin_provider_option("trt_engine_cache_enable", "1")?;
    set_plugin_provider_option("trt_engine_cache_path", cache_dir.to_string_lossy())?;
    set_plugin_provider_option(
        "trt_engine_cache_prefix",
        format!("sinorag-{}", profile.cache_slug()),
    )?;
    set_plugin_provider_option("trt_timing_cache_enable", "1")?;
    set_plugin_provider_option("trt_timing_cache_path", timing_cache.to_string_lossy())?;
    set_plugin_provider_option("trt_fp16_enable", "1")?;
    set_plugin_provider_option("trt_builder_optimization_level", "3")?;
    set_plugin_provider_option("trt_force_sequential_engine_build", "1")?;
    set_plugin_provider_option("trt_detailed_build_log", "1")?;

    if let Some((min, opt, max)) = tensorrt_profile_shapes(profile, batch_size) {
        set_plugin_provider_option("trt_profile_min_shapes", min)?;
        set_plugin_provider_option("trt_profile_opt_shapes", opt)?;
        set_plugin_provider_option("trt_profile_max_shapes", max)?;
    }

    let env = ort::environment::Environment::current()?;
    let device_count = env
        .devices()
        .filter_map(|device| device.ep().ok().map(str::to_string))
        .filter(|ep| ep.eq_ignore_ascii_case("TensorRTEp"))
        .count();

    if device_count == 0 && error_on_failure {
        anyhow::bail!(
            "TensorRT plugin EP registered from {}, but ORT reported no TensorRTEp devices",
            plugin.display()
        );
    }
    eprintln!("       TensorRT plugin devices: {device_count}");

    Ok(())
}

#[cfg(feature = "local-embeddings-tensorrt")]
fn register_tensorrt_plugin_library() -> Result<std::path::PathBuf> {
    let result = TENSORRT_PLUGIN_REGISTRATION.get_or_init(|| {
        let plugin = resolve_tensorrt_plugin_dll()
            .ok_or_else(|| "could not find ORTTensorRTEp.dll".to_string())?;
        let env = ort::environment::Environment::current().map_err(|e| e.to_string())?;
        env.register_ep_library("TensorRTEp", &plugin)
            .map_err(|e| format!("failed to register {}: {e}", plugin.display()))?;
        Ok(plugin)
    });
    result
        .as_ref()
        .cloned()
        .map_err(|message| anyhow::anyhow!("{message}"))
}

#[cfg(feature = "local-embeddings-tensorrt")]
fn resolve_tensorrt_plugin_dll() -> Option<std::path::PathBuf> {
    let mut candidates = Vec::new();
    for var in ["SINORAG_TENSORRT_EP_DLL", "ORT_TENSORRT_EP_DLL"] {
        if let Some(path) = std::env::var_os(var).map(std::path::PathBuf::from) {
            candidates.push(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("ORTTensorRTEp.dll"));
            candidates.push(
                dir.join("onnxruntime-ep-tensorrt")
                    .join("out")
                    .join("install-relwithdebinfo")
                    .join("bin")
                    .join("ORTTensorRTEp.dll"),
            );
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("ORTTensorRTEp.dll"));
        candidates.push(
            cwd.join("onnxruntime-ep-tensorrt")
                .join("out")
                .join("install-relwithdebinfo")
                .join("bin")
                .join("ORTTensorRTEp.dll"),
        );
    }
    #[cfg(windows)]
    candidates.push(std::path::PathBuf::from(
        r"D:\Rust-projects\SinoRAG-runtime\onnxruntime-ep-tensorrt\out\install-relwithdebinfo\bin\ORTTensorRTEp.dll",
    ));

    candidates.into_iter().find(|path| path.is_file())
}

#[cfg(feature = "local-embeddings-tensorrt")]
fn set_plugin_provider_option(key: &str, value: impl std::fmt::Display) -> Result<()> {
    let env_key = format!("SINORAG_TENSORRT_EP_OPTION_{}", key.to_ascii_uppercase());
    std::env::set_var(env_key, value.to_string());
    Ok(())
}

#[cfg(feature = "local-embeddings-tensorrt")]
fn resolve_tensorrt_root(explicit: Option<std::path::PathBuf>) -> Option<std::path::PathBuf> {
    explicit
        .or_else(|| std::env::var_os("SINORAG_TENSORRT_ROOT").map(std::path::PathBuf::from))
        .or_else(|| std::env::var_os("TENSORRT_ROOT").map(std::path::PathBuf::from))
        .or_else(|| std::env::var_os("TRT_ROOT").map(std::path::PathBuf::from))
        .or_else(|| {
            #[cfg(windows)]
            {
                let default = std::path::PathBuf::from(r"D:\TensorRT");
                if default.exists() {
                    return Some(default);
                }
            }
            None
        })
        .filter(|path| path.exists())
}

#[cfg(feature = "local-embeddings-tensorrt")]
fn add_tensorrt_to_path(root: &std::path::Path) {
    let mut entries = Vec::new();
    for child in ["lib", "bin"] {
        let path = root.join(child);
        if path.exists() {
            entries.push(path);
        }
    }
    if entries.is_empty() {
        entries.push(root.to_path_buf());
    }

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = std::env::split_paths(&old_path).collect::<Vec<_>>();
    for entry in entries.into_iter().rev() {
        if !paths.iter().any(|path| path == &entry) {
            paths.insert(0, entry);
        }
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        std::env::set_var("PATH", joined);
    }
}

#[cfg(feature = "local-embeddings-tensorrt")]
fn tensorrt_profile_shapes(
    profile: LocalEmbeddingProfile,
    batch_size: usize,
) -> Option<(String, String, String)> {
    if std::env::var_os("SINORAG_TRT_DISABLE_PROFILE_SHAPES").is_some() {
        return None;
    }
    if let Ok(raw) = std::env::var("SINORAG_TRT_PROFILE_SHAPES") {
        let mut parts = raw.split('|').map(str::trim);
        let min = parts.next()?.to_string();
        let opt = parts.next().unwrap_or(&min).to_string();
        let max = parts.next().unwrap_or(&opt).to_string();
        return Some((min, opt, max));
    }

    let opt_batch = batch_size.max(1);
    let max_batch = opt_batch.max(1);
    let names: &[&str] = match profile {
        LocalEmbeddingProfile::BgeSmallZhV15 => &["input_ids", "attention_mask", "token_type_ids"],
        LocalEmbeddingProfile::BgeM3 => &["input_ids", "attention_mask"],
    };
    let min = shape_list(names, 1, 1);
    let opt = shape_list(names, opt_batch, 512);
    let max = shape_list(names, max_batch, 512);
    Some((min, opt, max))
}

#[cfg(feature = "local-embeddings-tensorrt")]
fn shape_list(names: &[&str], batch: usize, seq_len: usize) -> String {
    names
        .iter()
        .map(|name| format!("{name}:{batch}x{seq_len}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(feature = "local-embeddings")]
impl EmbeddingProvider for FastEmbedProvider {
    fn provider_id(&self) -> &'static str {
        "fastembed-rs"
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn model_revision(&self) -> Option<&str> {
        self.model_revision.as_deref()
    }

    fn embedding_dim(&self) -> usize {
        self.dim
    }

    fn document_prefix(&self) -> &'static str {
        self.document_prefix
    }

    fn query_prefix(&self) -> &'static str {
        self.query_prefix
    }

    fn embed_documents(&mut self, inputs: &[EmbeddingInput]) -> Result<Vec<EmbeddingRow>> {
        let texts: Vec<String> = inputs
            .iter()
            .map(|x| format!("{}{}", self.document_prefix, x.embedding_text))
            .collect();

        let vectors = self.model.embed(texts, Some(self.batch_size))?;

        if vectors.len() != inputs.len() {
            anyhow::bail!(
                "embedding count mismatch: inputs={}, vectors={}",
                inputs.len(),
                vectors.len()
            );
        }

        let mut rows = Vec::with_capacity(inputs.len());
        for (input, vector) in inputs.iter().zip(vectors.into_iter()) {
            if vector.len() != self.dim {
                anyhow::bail!(
                    "embedding dimension mismatch for doc_id {}: expected {}, got {}",
                    input.doc_id,
                    self.dim,
                    vector.len()
                );
            }
            rows.push(EmbeddingRow {
                doc_id: input.doc_id,
                passage_id: input.passage_id.clone(),
                model_id: self.model_id.clone(),
                model_revision: self.model_revision.clone(),
                input_hash: input.input_hash.clone(),
                dim: vector.len(),
                vector,
            });
        }
        Ok(rows)
    }

    fn embed_query(&mut self, query: &str) -> Result<Vec<f32>> {
        let text = format!("{}{}", self.query_prefix, query);
        let mut vectors = self.model.embed(vec![text], Some(1))?;
        vectors
            .pop()
            .ok_or_else(|| anyhow::anyhow!("fastembed returned no query vector"))
    }
}
