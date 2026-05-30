use cxx::{Exception, UniquePtr};

#[cxx::bridge]
mod ffi {
    #[derive(Debug, Clone)]
    enum HostOutputKind {
        SentenceEmbedding,
        LastHiddenState,
    }

    #[derive(Debug, Clone)]
    struct Options {
        path: String,
    }

    struct HostOutput {
        kind: HostOutputKind,
        dim: usize,
        data: Vec<f32>,
    }

    unsafe extern "C++" {
        include!("fastembed/src/tensorrt_engine.h");

        type Engine;

        fn load_engine(options: &Options) -> Result<UniquePtr<Engine>>;

        fn infer_i32_host(
            self: Pin<&mut Engine>,
            input_ids: &[i32],
            attention_mask: &[i32],
            token_type_ids: &[i32],
            batch_size: u32,
            seq_len: u32,
        ) -> Result<HostOutput>;
    }
}

pub use ffi::{HostOutput, HostOutputKind, Options};

unsafe impl Send for ffi::Engine {}

pub struct HostEmbeddingEngine {
    inner: UniquePtr<ffi::Engine>,
}

impl HostEmbeddingEngine {
    pub fn new(path: &std::path::Path) -> Result<Self, Exception> {
        let options = Options {
            path: path.to_string_lossy().to_string(),
        };
        Ok(Self {
            inner: ffi::load_engine(&options)?,
        })
    }

    pub fn infer_i32(
        &mut self,
        input_ids: &[i32],
        attention_mask: &[i32],
        token_type_ids: Option<&[i32]>,
        batch_size: usize,
        seq_len: usize,
    ) -> Result<HostOutput, Exception> {
        self.inner.pin_mut().infer_i32_host(
            input_ids,
            attention_mask,
            token_type_ids.unwrap_or(&[]),
            batch_size as u32,
            seq_len as u32,
        )
    }
}

unsafe impl Send for HostEmbeddingEngine {}
