#[cfg(feature = "local-embeddings")]
use super::models::EmbeddingExecutionProvider;
#[cfg(feature = "local-embeddings")]
use super::models::LocalEmbeddingProfile;
#[cfg(feature = "local-embeddings")]
use super::provider::{EmbeddingInput, EmbeddingProvider, EmbeddingRow};
#[cfg(feature = "local-embeddings")]
use anyhow::Result;

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

        let providers = execution_providers(execution_provider)?;
        if providers.is_empty() {
            eprintln!("       Execution provider: CPU");
        } else {
            eprintln!(
                "       Execution provider: {:?} (ONNX Runtime CPU fallback enabled)",
                execution_provider
            );
            opts = opts.with_execution_providers(providers);
        }

        let model = TextEmbedding::try_new(opts).map_err(|e| {
            let msg = e.to_string();
            let mut hint = String::new();
            if msg.contains("onnxruntime") || msg.contains("ONNX") || msg.contains("dylib") || msg.contains("load") {
                hint.push_str("\n\nHints:\n");
                hint.push_str("  - Set ORT_DYLIB_PATH to a working onnxruntime.dll, or place one next to the executable.\n");
                hint.push_str("  - Retry with `--execution-provider cpu` to continue indexing without GPU provider DLLs.\n");
                #[cfg(windows)]
                hint.push_str("  - On Windows, `--execution-provider directml` is usually easier than CUDA and uses the GPU through DirectML.\n");
                #[cfg(feature = "local-embeddings-cuda")]
                hint.push_str("  - For CUDA: ONNX Runtime's official GPU packages are CUDA-major specific. A CUDA 12.x ORT build needs CUDA 12.x/cuDNN 9.x runtime DLLs; CUDA 13.x does not satisfy that contract.\n");
                #[cfg(all(windows, not(feature = "local-embeddings-cuda")))]
                hint.push_str("  - For DirectML: use Microsoft.ML.OnnxRuntime.DirectML; ship its onnxruntime.dll plus DirectML.dll alongside the executable.\n");
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
fn execution_providers(
    execution_provider: EmbeddingExecutionProvider,
) -> Result<Vec<fastembed::ExecutionProviderDispatch>> {
    let mut providers = Vec::new();
    match execution_provider {
        EmbeddingExecutionProvider::Cpu => {}
        EmbeddingExecutionProvider::Auto => {
            #[cfg(feature = "local-embeddings-cuda")]
            providers.push(ort::ep::CUDA::default().build());
            #[cfg(windows)]
            providers.push(ort::ep::DirectML::default().build());
        }
        EmbeddingExecutionProvider::Cuda => {
            #[cfg(feature = "local-embeddings-cuda")]
            {
                providers.push(ort::ep::CUDA::default().build().error_on_failure());
            }
            #[cfg(not(feature = "local-embeddings-cuda"))]
            {
                anyhow::bail!(
                    "CUDA execution provider requested, but this binary was not built with --features local-embeddings-cuda"
                );
            }
        }
        EmbeddingExecutionProvider::Directml => {
            #[cfg(windows)]
            {
                providers.push(ort::ep::DirectML::default().build().error_on_failure());
            }
            #[cfg(not(windows))]
            {
                anyhow::bail!("DirectML execution provider is only available on Windows");
            }
        }
    }
    Ok(providers)
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
