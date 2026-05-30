use crate::get_cache_dir;
use std::path::PathBuf;

#[cfg(feature = "ort-backend")]
pub use ort::execution_providers::ExecutionProviderDispatch;

#[cfg(not(feature = "ort-backend"))]
#[derive(Debug, Clone)]
pub struct ExecutionProviderDispatch;

pub trait HasMaxLength {
    const MAX_LENGTH: usize;
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct InitOptionsWithLength<M> {
    pub model_name: M,
    pub execution_providers: Vec<ExecutionProviderDispatch>,
    pub disable_cpu_fallback: bool,
    pub cache_dir: PathBuf,
    pub show_download_progress: bool,
    pub max_length: usize,
    pub backend: EmbeddingBackendConfig,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct InitOptions<M> {
    pub model_name: M,
    pub execution_providers: Vec<ExecutionProviderDispatch>,
    pub disable_cpu_fallback: bool,
    pub cache_dir: PathBuf,
    pub show_download_progress: bool,
    pub backend: EmbeddingBackendConfig,
}

#[derive(Debug, Clone)]
pub enum EmbeddingBackendConfig {
    TensorRt {
        engine_dir: Option<PathBuf>,
        engine_path: Option<PathBuf>,
    },
    Cpu,
    #[cfg(feature = "ort-backend")]
    Ort,
}

impl Default for EmbeddingBackendConfig {
    fn default() -> Self {
        Self::TensorRt {
            engine_dir: None,
            engine_path: None,
        }
    }
}

impl<M: Default + HasMaxLength> Default for InitOptionsWithLength<M> {
    fn default() -> Self {
        Self {
            model_name: M::default(),
            execution_providers: Default::default(),
            disable_cpu_fallback: false,
            cache_dir: get_cache_dir().into(),
            show_download_progress: true,
            max_length: M::MAX_LENGTH,
            backend: Default::default(),
        }
    }
}

impl<M: Default> Default for InitOptions<M> {
    fn default() -> Self {
        Self {
            model_name: M::default(),
            execution_providers: Default::default(),
            disable_cpu_fallback: false,
            cache_dir: get_cache_dir().into(),
            show_download_progress: true,
            backend: Default::default(),
        }
    }
}

impl<M: Default + HasMaxLength> InitOptionsWithLength<M> {
    /// Create a new InitOptionsWithLength with the given model name
    pub fn new(model_name: M) -> Self {
        Self {
            model_name,
            ..Default::default()
        }
    }

    /// Set the maximum length
    pub fn with_max_length(mut self, max_length: usize) -> Self {
        self.max_length = max_length;
        self
    }

    /// Set the cache directory for the model file
    pub fn with_cache_dir(mut self, cache_dir: PathBuf) -> Self {
        self.cache_dir = cache_dir;
        self
    }

    /// Set the execution providers for the model
    pub fn with_execution_providers(
        mut self,
        execution_providers: Vec<ExecutionProviderDispatch>,
    ) -> Self {
        self.execution_providers = execution_providers;
        self
    }

    /// Disable ONNX Runtime CPU fallback for nodes not assigned to a requested execution provider.
    pub fn with_disable_cpu_fallback(mut self, disable_cpu_fallback: bool) -> Self {
        self.disable_cpu_fallback = disable_cpu_fallback;
        self
    }

    /// Set whether to show download progress
    pub fn with_show_download_progress(mut self, show_download_progress: bool) -> Self {
        self.show_download_progress = show_download_progress;
        self
    }

    pub fn with_backend(mut self, backend: EmbeddingBackendConfig) -> Self {
        self.backend = backend;
        self
    }

    pub fn with_tensorrt_engine_dir(mut self, engine_dir: PathBuf) -> Self {
        self.backend = EmbeddingBackendConfig::TensorRt {
            engine_dir: Some(engine_dir),
            engine_path: None,
        };
        self
    }

    pub fn with_tensorrt_engine_path(mut self, engine_path: PathBuf) -> Self {
        self.backend = EmbeddingBackendConfig::TensorRt {
            engine_dir: None,
            engine_path: Some(engine_path),
        };
        self
    }

    pub fn with_cpu_backend(mut self) -> Self {
        self.backend = EmbeddingBackendConfig::Cpu;
        self
    }
}

impl<M: Default> InitOptions<M> {
    /// Create a new InitOptions with the given model name
    pub fn new(model_name: M) -> Self {
        Self {
            model_name,
            ..Default::default()
        }
    }

    /// Set the cache directory for the model file
    pub fn with_cache_dir(mut self, cache_dir: PathBuf) -> Self {
        self.cache_dir = cache_dir;
        self
    }

    /// Set the execution providers for the model
    pub fn with_execution_providers(
        mut self,
        execution_providers: Vec<ExecutionProviderDispatch>,
    ) -> Self {
        self.execution_providers = execution_providers;
        self
    }

    /// Disable ONNX Runtime CPU fallback for nodes not assigned to a requested execution provider.
    pub fn with_disable_cpu_fallback(mut self, disable_cpu_fallback: bool) -> Self {
        self.disable_cpu_fallback = disable_cpu_fallback;
        self
    }

    /// Set whether to show download progress
    pub fn with_show_download_progress(mut self, show_download_progress: bool) -> Self {
        self.show_download_progress = show_download_progress;
        self
    }

    pub fn with_backend(mut self, backend: EmbeddingBackendConfig) -> Self {
        self.backend = backend;
        self
    }
}
