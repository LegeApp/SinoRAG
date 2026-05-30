use crate::ModelInfo;

pub mod image_embedding;
pub mod model_info;
pub mod quantization;
#[cfg(feature = "ort-backend")]
pub mod reranking;
#[cfg(feature = "ort-backend")]
pub mod sparse;
pub mod text_embedding;

#[cfg(feature = "qwen3")]
pub mod qwen3;
#[cfg(feature = "qwen3")]
pub mod qwen3_vl;

#[cfg(feature = "nomic-v2-moe")]
pub mod nomic_v2_moe;

pub trait ModelTrait {
    type Model;
    fn get_model_info(model: &Self::Model) -> Option<&ModelInfo<Self::Model>>;
}
