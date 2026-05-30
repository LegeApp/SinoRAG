//! Text embedding implementation for non-ORT backends.

#[cfg(feature = "hf-hub")]
use crate::common::load_tokenizer_hf_hub;
use crate::{
    common::{normalize, Embedding},
    init::EmbeddingBackendConfig,
    models::{text_embedding::models_list, ModelTrait},
    pooling::Pooling,
    EmbeddingModel, ModelInfo, QuantizationMode,
};
use anyhow::{Context, Result};
#[cfg(feature = "hf-hub")]
use hf_hub::api::sync::ApiRepo;
#[cfg(feature = "hf-hub")]
use std::path::PathBuf;
use tokenizers::Tokenizer;
#[cfg(feature = "tensorrt")]
use tokenizers::PaddingStrategy;

#[cfg(feature = "hf-hub")]
use super::TextInitOptions;
use super::{
    EmbeddingOutput, OutputKey, TextEmbedding, TextEmbeddingBackend, UserDefinedEmbeddingModel,
    DEFAULT_BATCH_SIZE,
};

impl TextEmbedding {
    #[cfg(feature = "hf-hub")]
    pub fn try_new(options: TextInitOptions) -> Result<Self> {
        let TextInitOptions {
            max_length,
            model_name,
            cache_dir,
            show_download_progress,
            backend,
            ..
        } = options;

        let model_repo =
            TextEmbedding::retrieve_model(model_name.clone(), cache_dir, show_download_progress)?;
        let model_info = TextEmbedding::get_model_info(&model_name)?;
        let model_file_reference = model_repo
            .get(&model_info.model_file)
            .context(format!("Failed to retrieve {}", model_info.model_file))?;

        for file in &model_info.additional_files {
            model_repo
                .get(file)
                .context(format!("Failed to retrieve {}", file))?;
        }

        let mut tokenizer = load_tokenizer_hf_hub(model_repo, max_length)?;
        if matches!(backend, EmbeddingBackendConfig::TensorRt { .. }) {
            set_fixed_padding(&mut tokenizer, max_length)?;
        }
        Ok(Self::new(
            tokenizer,
            TextEmbeddingBackend::from_config(backend, model_file_reference)?,
            TextEmbedding::get_default_pooling_method(&model_name),
            TextEmbedding::get_quantization_mode(&model_name),
            model_info.output_key.clone(),
        ))
    }

    pub fn try_new_from_user_defined(
        model: UserDefinedEmbeddingModel,
        options: super::InitOptionsUserDefined,
    ) -> Result<Self> {
        let mut tokenizer = crate::common::load_tokenizer(model.tokenizer_files, options.max_length)?;
        if matches!(options.backend, EmbeddingBackendConfig::TensorRt { .. }) {
            set_fixed_padding(&mut tokenizer, options.max_length)?;
        }
        let tmp = std::env::temp_dir().join("fastembed-user-defined.onnx");
        std::fs::write(&tmp, model.onnx_file)?;
        Ok(Self::new(
            tokenizer,
            TextEmbeddingBackend::from_config(options.backend, tmp)?,
            model.pooling,
            model.quantization,
            model.output_key,
        ))
    }

    fn new(
        tokenizer: Tokenizer,
        backend: TextEmbeddingBackend,
        post_process: Option<Pooling>,
        quantization: QuantizationMode,
        output_key: Option<OutputKey>,
    ) -> Self {
        let need_token_type_ids = matches!(output_key, Some(OutputKey::ByName("token_type_ids")));
        Self {
            tokenizer,
            pooling: post_process,
            backend,
            need_token_type_ids,
            quantization,
            output_key,
        }
    }

    #[cfg(feature = "hf-hub")]
    fn retrieve_model(
        model: EmbeddingModel,
        cache_dir: PathBuf,
        show_download_progress: bool,
    ) -> anyhow::Result<ApiRepo> {
        use crate::common::pull_from_hf;

        let model_code = TextEmbedding::get_model_info(&model)?.model_code.clone();
        pull_from_hf(model_code, cache_dir, show_download_progress)
    }

    pub fn get_default_pooling_method(model_name: &EmbeddingModel) -> Option<Pooling> {
        match model_name {
            EmbeddingModel::BGESmallZHV15 => Some(Pooling::Cls),
            EmbeddingModel::BGEM3 => Some(Pooling::Cls),
            _ => Some(Pooling::Cls),
        }
    }

    pub fn get_quantization_mode(_model_name: &EmbeddingModel) -> QuantizationMode {
        QuantizationMode::None
    }

    pub fn list_supported_models() -> Vec<ModelInfo<EmbeddingModel>> {
        models_list()
    }

    pub fn get_model_info(model: &EmbeddingModel) -> Result<&ModelInfo<EmbeddingModel>> {
        EmbeddingModel::get_model_info(model).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "Model {model:?} not found. Please check if the model is supported by this build."
            ))
        })
    }

    pub fn transform<S: AsRef<str> + Send + Sync>(
        &mut self,
        texts: impl AsRef<[S]>,
        batch_size: Option<usize>,
    ) -> Result<EmbeddingOutput> {
        self.embed(texts, batch_size)
            .map(EmbeddingOutput::from_embeddings)
    }

    pub fn embed<S: AsRef<str> + Send + Sync>(
        &mut self,
        texts: impl AsRef<[S]>,
        batch_size: Option<usize>,
    ) -> Result<Vec<Embedding>> {
        let batch_size = batch_size.unwrap_or(DEFAULT_BATCH_SIZE).max(1);
        let mut out = Vec::new();
        for batch in texts.as_ref().chunks(batch_size) {
            let encoded = self.encode_batch(batch)?;
            let raw = match &mut self.backend {
                #[cfg(feature = "tensorrt")]
                TextEmbeddingBackend::TensorRt {
                    engine_path,
                    engine,
                } => {
                    run_tensorrt(engine_path, engine, &encoded, self.pooling.clone())?
                }
                #[cfg(not(feature = "tensorrt"))]
                TextEmbeddingBackend::TensorRt { engine_path } => {
                    run_tensorrt(engine_path, &encoded, self.pooling.clone())?
                }
                TextEmbeddingBackend::Cpu { model_path } => {
                    run_cpu(model_path, &encoded, self.pooling.clone())?
                }
            };
            out.extend(raw);
        }
        Ok(out)
    }

    fn encode_batch<S: AsRef<str>>(&self, batch: &[S]) -> Result<EncodedBatch> {
        let inputs = batch.iter().map(|text| text.as_ref()).collect();
        let encodings = self.tokenizer.encode_batch(inputs, true).map_err(|e| {
            anyhow::Error::msg(e.to_string()).context("Failed to encode the batch.")
        })?;
        let seq_len = encodings
            .first()
            .ok_or_else(|| anyhow::anyhow!("Tokenizer returned empty encodings"))?
            .len();
        let mut input_ids = Vec::with_capacity(batch.len() * seq_len);
        let mut attention_mask = Vec::with_capacity(batch.len() * seq_len);
        let mut token_type_ids = Vec::with_capacity(batch.len() * seq_len);
        for encoding in &encodings {
            input_ids.extend(encoding.get_ids().iter().map(|x| *x as i32));
            attention_mask.extend(encoding.get_attention_mask().iter().map(|x| *x as i32));
            token_type_ids.extend(encoding.get_type_ids().iter().map(|x| *x as i32));
        }
        Ok(EncodedBatch {
            batch: batch.len(),
            seq_len,
            input_ids,
            attention_mask,
            token_type_ids,
        })
    }
}

#[cfg(feature = "tensorrt")]
fn set_fixed_padding(tokenizer: &mut Tokenizer, max_length: usize) -> Result<()> {
    let mut padding = tokenizer
        .get_padding()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("TensorRT backend requires tokenizer padding settings"))?;
    padding.strategy = PaddingStrategy::Fixed(max_length);
    tokenizer.with_padding(Some(padding));
    Ok(())
}

#[cfg(not(feature = "tensorrt"))]
fn set_fixed_padding(_tokenizer: &mut Tokenizer, _max_length: usize) -> Result<()> {
    Ok(())
}

struct EncodedBatch {
    batch: usize,
    seq_len: usize,
    input_ids: Vec<i32>,
    attention_mask: Vec<i32>,
    token_type_ids: Vec<i32>,
}

#[cfg(feature = "tensorrt")]
fn run_tensorrt(
    _engine_path: &std::path::Path,
    engine: &mut crate::tensorrt::HostEmbeddingEngine,
    encoded: &EncodedBatch,
    pooling: Option<Pooling>,
) -> Result<Vec<Embedding>> {
    let output = engine.infer_i32(
        &encoded.input_ids,
        &encoded.attention_mask,
        Some(&encoded.token_type_ids),
        encoded.batch,
        encoded.seq_len,
    )?;
    let kind = match output.kind {
        crate::tensorrt::HostOutputKind::SentenceEmbedding => {
            embedding_output::OutputKind::SentenceEmbedding
        }
        crate::tensorrt::HostOutputKind::LastHiddenState => {
            embedding_output::OutputKind::LastHiddenState
        }
        _ => anyhow::bail!("TensorRT returned an unknown embedding output kind"),
    };
    postprocess(
        embedding_output::OutputTensor {
            kind,
            dim: output.dim,
            data: output.data,
        },
        encoded,
        pooling,
    )
}

#[cfg(not(feature = "tensorrt"))]
fn run_tensorrt(
    engine_path: &std::path::Path,
    _encoded: &EncodedBatch,
    _pooling: Option<Pooling>,
) -> Result<Vec<Embedding>> {
    anyhow::bail!(
        "TensorRT backend requested but this fastembed build was compiled without the tensorrt feature; expected engine {}",
        engine_path.display()
    )
}

#[cfg(feature = "tract")]
fn run_cpu(
    model_path: &std::path::Path,
    encoded: &EncodedBatch,
    pooling: Option<Pooling>,
) -> Result<Vec<Embedding>> {
    use tract_onnx::prelude::*;

    let mut model = tract_onnx::onnx()
        .model_for_path(model_path)
        .with_context(|| format!("Failed to load ONNX model {}", model_path.display()))?;
    let input_outlets = model.input_outlets()?.to_vec();
    for slot in 0..input_outlets.len() {
        model.set_input_fact(
            slot,
            InferenceFact::dt_shape(i64::datum_type(), [encoded.batch, encoded.seq_len]),
        )?;
    }

    let input_ids = encoded
        .input_ids
        .iter()
        .map(|&value| value as i64)
        .collect::<Vec<_>>();
    let attention_mask = encoded
        .attention_mask
        .iter()
        .map(|&value| value as i64)
        .collect::<Vec<_>>();
    let token_type_ids = encoded
        .token_type_ids
        .iter()
        .map(|&value| value as i64)
        .collect::<Vec<_>>();

    let mut inputs: TVec<TValue> = tvec!();
    for outlet in input_outlets {
        let input_name = model
            .outlet_label(outlet)
            .unwrap_or(&model.nodes[outlet.node].name)
            .to_ascii_lowercase();
        let data = if input_name.contains("attention") || input_name.contains("mask") {
            &attention_mask
        } else if input_name.contains("token_type")
            || input_name.contains("segment")
            || input_name.contains("type_ids")
        {
            &token_type_ids
        } else {
            &input_ids
        };
        inputs.push(Tensor::from_shape(&[encoded.batch, encoded.seq_len], data)?.into_tvalue());
    }

    let outputs = model
        .into_optimized()
        .context("Tract could not optimize the embedding ONNX model")?
        .into_runnable()
        .context("Tract could not create a runnable embedding model")?
        .run(inputs)
        .context("Tract CPU embedding inference failed")?;
    let output = outputs
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Tract embedding model returned no outputs"))?
        .into_tensor();
    let output_view = output.to_array_view::<f32>()?;
    let shape = output_view.shape().to_vec();
    let (kind, dim) = match shape.as_slice() {
        [batch, dim] if *batch == encoded.batch => {
            (embedding_output::OutputKind::SentenceEmbedding, *dim)
        }
        [batch, seq_len, dim] if *batch == encoded.batch && *seq_len == encoded.seq_len => {
            (embedding_output::OutputKind::LastHiddenState, *dim)
        }
        _ => anyhow::bail!("Unsupported Tract embedding output shape: {:?}", shape),
    };

    postprocess(
        embedding_output::OutputTensor {
            kind,
            dim,
            data: output_view.iter().copied().collect(),
        },
        encoded,
        pooling,
    )
}

#[cfg(not(feature = "tract"))]
fn run_cpu(
    model_path: &std::path::Path,
    _encoded: &EncodedBatch,
    _pooling: Option<Pooling>,
) -> Result<Vec<Embedding>> {
    anyhow::bail!(
        "CPU embedding backend requested but this fastembed build was compiled without the tract feature; model path {}",
        model_path.display()
    )
}

fn postprocess(
    raw: embedding_output::OutputTensor,
    encoded: &EncodedBatch,
    pooling: Option<Pooling>,
) -> Result<Vec<Embedding>> {
    let dim = raw.dim;
    let vectors = match raw.kind {
        embedding_output::OutputKind::SentenceEmbedding => raw.data,
        embedding_output::OutputKind::LastHiddenState => {
            let mut pooled = Vec::with_capacity(encoded.batch * dim);
            match pooling.unwrap_or(Pooling::Cls) {
                Pooling::Cls => {
                    for row in 0..encoded.batch {
                        let start = row * encoded.seq_len * dim;
                        pooled.extend_from_slice(&raw.data[start..start + dim]);
                    }
                }
                Pooling::Mean => {
                    for row in 0..encoded.batch {
                        let mut sums = vec![0.0f32; dim];
                        let mut denom = 0.0f32;
                        for tok in 0..encoded.seq_len {
                            if encoded.attention_mask[row * encoded.seq_len + tok] == 0 {
                                continue;
                            }
                            denom += 1.0;
                            let base = (row * encoded.seq_len + tok) * dim;
                            for d in 0..dim {
                                sums[d] += raw.data[base + d];
                            }
                        }
                        let denom = denom.max(1.0);
                        for value in &mut sums {
                            *value /= denom;
                        }
                        pooled.extend_from_slice(&sums);
                    }
                }
            }
            pooled
        }
    };
    vectors
        .chunks(dim)
        .map(normalize)
        .collect::<Vec<_>>()
        .pipe(Ok)
}

mod embedding_output {
    pub enum OutputKind {
        SentenceEmbedding,
        LastHiddenState,
    }

    pub struct OutputTensor {
        pub kind: OutputKind,
        pub dim: usize,
        pub data: Vec<f32>,
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}
