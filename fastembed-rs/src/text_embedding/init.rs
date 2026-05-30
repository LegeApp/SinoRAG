//! Initialization options for the text embedding models.
//!

use crate::{
    common::TokenizerFiles,
    init::{EmbeddingBackendConfig, HasMaxLength, InitOptionsWithLength},
    pooling::Pooling,
    EmbeddingModel, QuantizationMode,
};
use std::path::PathBuf;
use tokenizers::Tokenizer;

#[cfg(feature = "ort-backend")]
use ort::{execution_providers::ExecutionProviderDispatch, session::Session};
#[cfg(not(feature = "ort-backend"))]
use crate::ExecutionProviderDispatch;
#[cfg(feature = "ort-backend")]
use crate::OutputKey;
#[cfg(not(feature = "ort-backend"))]
use super::OutputKey;

use super::DEFAULT_MAX_LENGTH;

impl HasMaxLength for EmbeddingModel {
    const MAX_LENGTH: usize = DEFAULT_MAX_LENGTH;
}

/// Options for initializing the TextEmbedding model
pub type TextInitOptions = InitOptionsWithLength<EmbeddingModel>;

/// Options for initializing UserDefinedEmbeddingModel
///
/// Model files are held by the UserDefinedEmbeddingModel struct
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct InitOptionsUserDefined {
    pub execution_providers: Vec<ExecutionProviderDispatch>,
    pub disable_cpu_fallback: bool,
    pub max_length: usize,
    pub backend: EmbeddingBackendConfig,
}

impl InitOptionsUserDefined {
    pub fn new() -> Self {
        Self {
            ..Default::default()
        }
    }

    pub fn with_execution_providers(
        mut self,
        execution_providers: Vec<ExecutionProviderDispatch>,
    ) -> Self {
        self.execution_providers = execution_providers;
        self
    }

    pub fn with_disable_cpu_fallback(mut self, disable_cpu_fallback: bool) -> Self {
        self.disable_cpu_fallback = disable_cpu_fallback;
        self
    }

    pub fn with_max_length(mut self, max_length: usize) -> Self {
        self.max_length = max_length;
        self
    }

    pub fn with_backend(mut self, backend: EmbeddingBackendConfig) -> Self {
        self.backend = backend;
        self
    }
}

impl Default for InitOptionsUserDefined {
    fn default() -> Self {
        Self {
            execution_providers: Default::default(),
            disable_cpu_fallback: false,
            max_length: DEFAULT_MAX_LENGTH,
            backend: Default::default(),
        }
    }
}

/// Convert InitOptions to InitOptionsUserDefined
///
/// This is useful for when the user wants to use the same options for both the default and user-defined models
impl From<TextInitOptions> for InitOptionsUserDefined {
    fn from(options: TextInitOptions) -> Self {
        InitOptionsUserDefined {
            execution_providers: options.execution_providers,
            disable_cpu_fallback: options.disable_cpu_fallback,
            max_length: options.max_length,
            backend: options.backend,
        }
    }
}

/// Struct for "bring your own" embedding models
///
/// The onnx_file and tokenizer_files are expecting the files' bytes
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDefinedEmbeddingModel {
    pub onnx_file: Vec<u8>,
    pub external_initializers: Vec<ExternalInitializerFile>,
    pub tokenizer_files: TokenizerFiles,
    pub pooling: Option<Pooling>,
    pub quantization: QuantizationMode,
    pub output_key: Option<OutputKey>,
}

/// Struct for adding external initializers to "bring your own" embedding models
///
/// The buffer is expecting the data of the external initializer and the file_name
/// must match the one referenced by the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalInitializerFile {
    pub file_name: String,
    pub buffer: Vec<u8>,
}

impl UserDefinedEmbeddingModel {
    pub fn new(onnx_file: Vec<u8>, tokenizer_files: TokenizerFiles) -> Self {
        Self {
            onnx_file,
            external_initializers: Vec::new(),
            tokenizer_files,
            quantization: QuantizationMode::None,
            pooling: None,
            output_key: None,
        }
    }

    pub fn with_quantization(mut self, quantization: QuantizationMode) -> Self {
        self.quantization = quantization;
        self
    }

    pub fn with_pooling(mut self, pooling: Pooling) -> Self {
        self.pooling = Some(pooling);
        self
    }

    pub fn with_external_initializer(mut self, file_name: String, buffer: Vec<u8>) -> Self {
        self.external_initializers
            .push(ExternalInitializerFile { file_name, buffer });
        self
    }
}

/// Rust representation of the TextEmbedding model
pub struct TextEmbedding {
    pub tokenizer: Tokenizer,
    pub(crate) pooling: Option<Pooling>,
    #[allow(dead_code)]
    pub(crate) backend: TextEmbeddingBackend,
    #[cfg(feature = "ort-backend")]
    pub(crate) session: Session,
    #[allow(dead_code)]
    pub(crate) need_token_type_ids: bool,
    #[allow(dead_code)]
    pub(crate) quantization: QuantizationMode,
    #[allow(dead_code)]
    pub(crate) output_key: Option<OutputKey>,
}

#[allow(dead_code)]
pub(crate) enum TextEmbeddingBackend {
    #[cfg(feature = "tensorrt")]
    TensorRt {
        engine_path: PathBuf,
        engine: crate::tensorrt::HostEmbeddingEngine,
    },
    #[cfg(not(feature = "tensorrt"))]
    TensorRt { engine_path: PathBuf },
    Cpu { model_path: PathBuf },
    #[cfg(feature = "ort-backend")]
    Ort,
}

impl TextEmbeddingBackend {
    #[allow(dead_code)]
    pub(crate) fn from_config(
        config: EmbeddingBackendConfig,
        model_path: PathBuf,
    ) -> anyhow::Result<Self> {
        match config {
            EmbeddingBackendConfig::TensorRt {
                engine_dir,
                engine_path,
            } => {
                let engine_path = engine_path
                    .or_else(|| engine_dir.map(|dir| dir.join("engine.plan")))
                    .unwrap_or_else(|| model_path.with_extension("engine"));
                #[cfg(feature = "tensorrt")]
                {
                    let engine =
                        crate::tensorrt::HostEmbeddingEngine::new(&engine_path).map_err(|e| {
                            anyhow::anyhow!(
                                "failed to load TensorRT engine {}: {}",
                                engine_path.display(),
                                e
                            )
                        })?;
                    Ok(Self::TensorRt {
                        engine_path,
                        engine,
                    })
                }
                #[cfg(not(feature = "tensorrt"))]
                {
                    Ok(Self::TensorRt { engine_path })
                }
            }
            EmbeddingBackendConfig::Cpu => Ok(Self::Cpu { model_path }),
            #[cfg(feature = "ort-backend")]
            EmbeddingBackendConfig::Ort => Ok(Self::Ort),
        }
    }
}
