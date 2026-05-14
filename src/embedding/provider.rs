use anyhow::Result;

pub struct EmbeddingInput {
    pub doc_id: u32,
    pub passage_id: String,
    pub embedding_text: String,
    pub input_hash: String,
}

pub struct EmbeddingRow {
    pub doc_id: u32,
    pub passage_id: String,
    pub model_id: String,
    pub model_revision: Option<String>,
    pub input_hash: String,
    pub dim: usize,
    pub vector: Vec<f32>,
}

pub trait EmbeddingProvider: Send {
    fn provider_id(&self) -> &'static str;
    fn model_id(&self) -> &str;
    fn model_revision(&self) -> Option<&str>;
    fn embedding_dim(&self) -> usize;
    fn document_prefix(&self) -> &'static str;
    fn query_prefix(&self) -> &'static str;

    fn embed_documents(&mut self, inputs: &[EmbeddingInput]) -> Result<Vec<EmbeddingRow>>;
    fn embed_query(&mut self, query: &str) -> Result<Vec<f32>>;
}
