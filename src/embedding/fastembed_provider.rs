#[cfg(feature = "local-embeddings")]
use super::models::EmbeddingExecutionProvider;
#[cfg(feature = "local-embeddings")]
use super::models::LocalEmbeddingProfile;
#[cfg(feature = "local-embeddings")]
use super::provider::{EmbeddingInput, EmbeddingProvider, EmbeddingRow};
#[cfg(feature = "local-embeddings")]
use anyhow::Result;
#[cfg(feature = "tensorrt")]
use std::sync::OnceLock;

#[cfg(feature = "tensorrt")]
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

/// On Linux, auto-detect ORT and TensorRT EP library paths relative to the executable
/// when the caller has not set them explicitly.  This allows `sinorag index vector-update`
/// to work without sourcing a shell script, as long as the standard directory layout is
/// present next to the binary.
///
/// The expected layout (matches what setup_tensorrt_linux.sh produces):
///   <exe-dir>/
///     onnxruntime-linux-x64-gpu_cuda13-1.26.0/
///       onnxruntime-linux-x64-gpu-1.26.0/lib/
///         libonnxruntime.so.1.26.0          ← ORT_DYLIB_PATH
///         libonnxruntime_providers_tensorrt.so  ← SINORAG_TENSORRT_EP_DLL
///     cuda-compat/
///       libcudart.so.13   (shim → CUDA 12)
///       libcublas.so.13   (shim → cuBLAS 12)
///
/// The ORT and cuda-compat libraries carry RPATH entries that point back to their own
/// dependencies (/opt/tensorrt/lib, /usr/local/cuda/lib64, etc.), so no LD_LIBRARY_PATH
/// manipulation is needed here.
#[cfg(all(feature = "local-embeddings", target_os = "linux"))]
fn setup_linux_ort_paths() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(exe_dir) = exe.parent() else {
        return;
    };

    if std::env::var_os("ORT_DYLIB_PATH").is_none() {
        for rel in [
            "onnxruntime-linux-x64-gpu_cuda13-1.26.0/onnxruntime-linux-x64-gpu-1.26.0/lib/libonnxruntime.so.1.26.0",
            "libonnxruntime.so.1.26.0",
            "libonnxruntime.so",
        ] {
            let p = exe_dir.join(rel);
            if p.is_file() {
                std::env::set_var("ORT_DYLIB_PATH", p);
                break;
            }
        }
    }

    #[cfg(feature = "tensorrt")]
    if std::env::var_os("SINORAG_TENSORRT_EP_DLL").is_none()
        && std::env::var_os("ORT_TENSORRT_EP_DLL").is_none()
    {
        for rel in [
            "onnxruntime-linux-x64-gpu_cuda13-1.26.0/onnxruntime-linux-x64-gpu-1.26.0/lib/libonnxruntime_providers_tensorrt.so",
            "libonnxruntime_providers_tensorrt.so",
        ] {
            let p = exe_dir.join(rel);
            if p.is_file() {
                std::env::set_var("SINORAG_TENSORRT_EP_DLL", p);
                break;
            }
        }
    }
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
        #[cfg(target_os = "linux")]
        setup_linux_ort_paths();

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
        let dylib_hint = std::env::var("ORT_DYLIB_PATH").unwrap_or_else(|_| {
            if cfg!(windows) {
                "onnxruntime.dll (default search)".to_string()
            } else {
                "libonnxruntime.so (default search)".to_string()
            }
        });
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
                hint.push_str("  - Set ORT_DYLIB_PATH to a working ONNX Runtime shared library, or place one next to the executable.\n");
                #[cfg(feature = "tensorrt")]
                hint.push_str("  - Set SINORAG_TENSORRT_EP_DLL to ORTTensorRTEp.dll; make TensorRT/CUDA DLLs visible via --tensorrt-root, SINORAG_TENSORRT_ROOT, or PATH.\n");
                #[cfg(not(feature = "tensorrt"))]
                hint.push_str("  - Rebuild with --features tensorrt to enable TensorRT.\n");
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
    #[cfg(feature = "tensorrt")]
    {
        register_tensorrt_plugin(profile, batch_size, tensorrt_root, tensorrt_cache_dir, true)?;
        Ok(Vec::new())
    }
    #[cfg(not(feature = "tensorrt"))]
    {
        let _ = (profile, batch_size, tensorrt_root, tensorrt_cache_dir);
        anyhow::bail!(
            "TensorRT is required for vector embedding, but this executable was compiled without \
             the `tensorrt` feature.\n\n\
             This specific error is a build/copy mismatch, not a missing TensorRT .so/.dll. \
             Rebuild with:\n\
               cargo build --release --features tensorrt\n\
             Then copy the resulting `target/release/sinorag` into this runtime directory and \
             re-run the command."
        )
    }
}

#[cfg(feature = "tensorrt")]
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
    set_default_plugin_provider_option("trt_engine_cache_enable", "1");
    set_default_plugin_provider_option("trt_engine_cache_path", cache_dir.to_string_lossy());
    set_default_plugin_provider_option(
        "trt_engine_cache_prefix",
        format!("sinorag-{}", profile.cache_slug()),
    );
    set_default_plugin_provider_option("trt_timing_cache_enable", "1");
    set_default_plugin_provider_option("trt_timing_cache_path", timing_cache.to_string_lossy());
    set_default_plugin_provider_option("trt_fp16_enable", "1");
    set_default_plugin_provider_option("trt_builder_optimization_level", "3");
    set_default_plugin_provider_option("trt_force_sequential_engine_build", "1");
    set_default_plugin_provider_option("trt_detailed_build_log", "1");

    if let Some((min, opt, max)) = tensorrt_profile_shapes(profile, batch_size) {
        set_default_plugin_provider_option("trt_profile_min_shapes", min);
        set_default_plugin_provider_option("trt_profile_opt_shapes", opt);
        set_default_plugin_provider_option("trt_profile_max_shapes", max);
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

#[cfg(feature = "tensorrt")]
fn register_tensorrt_plugin_library() -> Result<std::path::PathBuf> {
    let result = TENSORRT_PLUGIN_REGISTRATION.get_or_init(|| {
        let plugin = resolve_tensorrt_plugin_library().map_err(|e| e.to_string())?;
        let env = ort::environment::Environment::current().map_err(|e| e.to_string())?;
        env.register_ep_library("TensorRTEp", &plugin)
            .map_err(|e| {
                format!(
                    "failed to register TensorRT EP library {}: {e}\n\n{}",
                    plugin.display(),
                    tensorrt_runtime_hint()
                )
            })?;
        Ok(plugin)
    });
    result
        .as_ref()
        .cloned()
        .map_err(|message| anyhow::anyhow!("{message}"))
}

#[cfg(feature = "tensorrt")]
fn resolve_tensorrt_plugin_library() -> Result<std::path::PathBuf> {
    let mut candidates = Vec::new();
    for var in ["SINORAG_TENSORRT_EP_DLL", "ORT_TENSORRT_EP_DLL"] {
        if let Some(path) = std::env::var_os(var).map(std::path::PathBuf::from) {
            candidates.push(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            push_tensorrt_plugin_candidates(&mut candidates, dir);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        push_tensorrt_plugin_candidates(&mut candidates, &cwd);
    }
    for var in ["SINORAG_TENSORRT_ROOT", "TENSORRT_ROOT", "TRT_ROOT"] {
        if let Some(root) = std::env::var_os(var).map(std::path::PathBuf::from) {
            push_tensorrt_plugin_candidates(&mut candidates, &root);
            push_tensorrt_plugin_candidates(&mut candidates, &root.join("lib"));
            push_tensorrt_plugin_candidates(&mut candidates, &root.join("bin"));
        }
    }
    #[cfg(windows)]
    candidates.push(std::path::PathBuf::from(
        r"D:\Rust-projects\SinoRAG-runtime\onnxruntime-ep-tensorrt\out\install-relwithdebinfo\bin\ORTTensorRTEp.dll",
    ));

    if let Some(path) = candidates.iter().find(|path| path.is_file()) {
        return Ok(path.clone());
    }

    let searched = candidates
        .iter()
        .map(|path| format!("  - {}", path.display()))
        .collect::<Vec<_>>()
        .join("\n");
    anyhow::bail!(
        "could not find ONNX Runtime TensorRT EP plugin library.\n\n\
         Set SINORAG_TENSORRT_EP_DLL or ORT_TENSORRT_EP_DLL to the plugin path.\n\
         Expected names include: {}\n\n\
         Searched:\n{}\n\n{}",
        tensorrt_plugin_library_names().join(", "),
        searched,
        tensorrt_runtime_hint()
    )
}

#[cfg(feature = "tensorrt")]
fn push_tensorrt_plugin_candidates(
    candidates: &mut Vec<std::path::PathBuf>,
    dir: &std::path::Path,
) {
    for name in tensorrt_plugin_library_names() {
        candidates.push(dir.join(name));
    }
    candidates.push(
        dir.join("onnxruntime-ep-tensorrt")
            .join("out")
            .join("install-relwithdebinfo")
            .join(platform_library_dir())
            .join(primary_tensorrt_plugin_library_name()),
    );
}

#[cfg(feature = "tensorrt")]
fn tensorrt_plugin_library_names() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["ORTTensorRTEp.dll"]
    }
    #[cfg(target_os = "linux")]
    {
        &[
            "libonnxruntime_providers_tensorrt.so",
            "libonnxruntime_ep_tensorrt.so",
            "ORTTensorRTEp.so",
        ]
    }
    #[cfg(all(not(windows), not(target_os = "linux")))]
    {
        &[
            "libonnxruntime_providers_tensorrt.so",
            "libonnxruntime_ep_tensorrt.so",
            "ORTTensorRTEp.so",
        ]
    }
}

#[cfg(feature = "tensorrt")]
fn primary_tensorrt_plugin_library_name() -> &'static str {
    tensorrt_plugin_library_names()[0]
}

#[cfg(feature = "tensorrt")]
fn platform_library_dir() -> &'static str {
    #[cfg(windows)]
    {
        "bin"
    }
    #[cfg(not(windows))]
    {
        "lib"
    }
}

#[cfg(feature = "tensorrt")]
fn tensorrt_runtime_hint() -> String {
    let mut hint = String::from("TensorRT runtime requirements:\n");
    #[cfg(target_os = "linux")]
    {
        hint.push_str("  - Install Linux TensorRT and CUDA runtime libraries.\n");
        hint.push_str("  - Set SINORAG_TENSORRT_ROOT, TENSORRT_ROOT, or TRT_ROOT to the TensorRT root, or pass --tensorrt-root.\n");
        hint.push_str("  - Ensure TensorRT lib dirs are visible to the dynamic linker, e.g. LD_LIBRARY_PATH=$TENSORRT_ROOT/lib:$LD_LIBRARY_PATH.\n");
        hint.push_str("  - Ensure ONNX Runtime can load: libnvinfer.so, libnvinfer_plugin.so, libnvonnxparser.so, libcudart.so, libcublas.so, libcublasLt.so.\n");
    }
    #[cfg(windows)]
    {
        hint.push_str("  - Set SINORAG_TENSORRT_ROOT, TENSORRT_ROOT, or TRT_ROOT to the TensorRT root, or pass --tensorrt-root.\n");
        hint.push_str("  - Ensure TensorRT/CUDA DLLs are on PATH: nvinfer_10.dll, nvonnxparser_10.dll, cudart64_12.dll, cublas64_12.dll, cublasLt64_12.dll.\n");
    }
    hint.push_str("  - ORT_DYLIB_PATH must point to a compatible ONNX Runtime shared library when it is not next to the executable.\n");
    hint
}

#[cfg(feature = "tensorrt")]
fn set_default_plugin_provider_option(key: &str, value: impl std::fmt::Display) {
    let env_key = format!("SINORAG_TENSORRT_EP_OPTION_{}", key.to_ascii_uppercase());
    if std::env::var_os(&env_key).is_none() {
        std::env::set_var(env_key, value.to_string());
    }
}

#[cfg(feature = "tensorrt")]
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

#[cfg(feature = "tensorrt")]
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

#[cfg(feature = "tensorrt")]
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

#[cfg(feature = "tensorrt")]
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
