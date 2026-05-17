use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LocalEmbeddingProfile {
    /// Fast Chinese model (BAAI/bge-small-zh-v1.5, 512 dims). Good default for Chinese corpora.
    #[value(name = "bge-small-zh-v1.5")]
    BgeSmallZhV15,
    /// Multilingual model (BAAI/bge-m3, 1024 dims). Better for cross-language discovery.
    #[value(name = "bge-m3")]
    BgeM3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum EmbeddingExecutionProvider {
    /// Prefer available GPU providers but fall back to CPU if provider loading fails.
    Auto,
    /// ONNX Runtime CPU execution provider.
    Cpu,
    /// ONNX Runtime DirectML execution provider on Windows.
    Directml,
    /// ONNX Runtime CUDA execution provider.
    Cuda,
}

impl LocalEmbeddingProfile {
    pub fn model_id(self) -> &'static str {
        match self {
            Self::BgeSmallZhV15 => "BAAI/bge-small-zh-v1.5",
            Self::BgeM3 => "BAAI/bge-m3",
        }
    }

    pub fn dim(self) -> usize {
        match self {
            Self::BgeSmallZhV15 => 512,
            Self::BgeM3 => 1024,
        }
    }

    pub fn document_prefix(self) -> &'static str {
        "passage: "
    }

    pub fn query_prefix(self) -> &'static str {
        "query: "
    }

    pub fn default_batch_size(self) -> usize {
        match self {
            Self::BgeSmallZhV15 => 128,
            Self::BgeM3 => 32,
        }
    }

    pub fn cache_filename(self) -> &'static str {
        match self {
            Self::BgeSmallZhV15 => "vector_embeddings.bge-small-zh-v1.5.jsonl",
            Self::BgeM3 => "vector_embeddings.bge-m3.jsonl",
        }
    }
}
