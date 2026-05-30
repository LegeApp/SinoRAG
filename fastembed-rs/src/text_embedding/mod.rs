//! Text embedding module, containing the main struct [TextEmbedding] and its
//! initialization options.

// Constants.
const DEFAULT_BATCH_SIZE: usize = 256;
const DEFAULT_MAX_LENGTH: usize = 512;

// Output precedence and transforming functions.
#[cfg(feature = "ort-backend")]
pub mod output;

#[cfg(not(feature = "ort-backend"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputKey {
    OnlyOne,
    ByOrder(usize),
    ByName(&'static str),
}

#[cfg(not(feature = "ort-backend"))]
pub struct EmbeddingOutput {
    embeddings: Vec<crate::Embedding>,
}

#[cfg(not(feature = "ort-backend"))]
impl EmbeddingOutput {
    pub fn from_embeddings(embeddings: Vec<crate::Embedding>) -> Self {
        Self { embeddings }
    }

    pub fn into_embeddings(self) -> Vec<crate::Embedding> {
        self.embeddings
    }
}

// Initialization options.
mod init;
pub use init::*;

// The implementation of the embedding models.
#[cfg(feature = "ort-backend")]
mod r#impl;
#[cfg(not(feature = "ort-backend"))]
mod impl_direct;
