#[cfg(feature = "local-embeddings")]
use super::cache::{self, CacheRecord, CACHE_SCHEMA};
use super::cache::{EmbeddingCache, DOCUMENT_TEMPLATE_ID};
use super::models::LocalEmbeddingProfile;
use crate::datafusion_store::DataFusionStore;
use crate::document_table::DocumentTable;
use crate::vector_index::{self, EmbeddingRecord, HnswParams, VectorBuildMetadata};
use anyhow::{Context, Result};
#[cfg(feature = "local-embeddings")]
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub struct VectorUpdateConfig {
    pub parquet_path: PathBuf,
    pub doc_table_path: PathBuf,
    pub cache_path: PathBuf,
    pub vector_out: PathBuf,
    pub profile: LocalEmbeddingProfile,
    pub batch_size: usize,
    pub model_cache_dir: Option<PathBuf>,
    pub show_download_progress: bool,
    pub hnsw: HnswParams,
    /// When true, bail if the binary was built without the local-embeddings feature.
    /// When false, print a notice and return Ok.
    pub fail_if_feature_missing: bool,
}

pub async fn run_vector_update(config: VectorUpdateConfig) -> Result<()> {
    let model_id = config.profile.model_id();
    let dim = config.profile.dim();

    // 1. Load doc_table
    let doc_table = DocumentTable::load(&config.doc_table_path)
        .with_context(|| format!("load doc_table {}", config.doc_table_path.display()))?;
    eprintln!(
        "[1/5] doc_table: {} passages, fingerprint {}…",
        doc_table.passage_ids.len(),
        &doc_table.source_fingerprint[..8.min(doc_table.source_fingerprint.len())]
    );

    // 2. Read passage metadata from parquet
    eprintln!("[2/5] Reading passage metadata from parquet...");
    let store = DataFusionStore::open(&config.parquet_path)
        .await
        .with_context(|| format!("open parquet {}", config.parquet_path.display()))?;
    let rows = store
        .query_json(
            "SELECT passage_id, main_title, heading, period, zh_text_raw, zh_text_normalized \
             FROM passages WHERE passage_id IS NOT NULL ORDER BY passage_id",
        )
        .await?;

    // Build passage_id -> (embedding_text, input_hash)
    let mut passage_meta: rustc_hash::FxHashMap<String, (String, String)> =
        rustc_hash::FxHashMap::default();
    for row in rows {
        let Some(passage_id) = row
            .get("passage_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let text = row
            .get("zh_text_raw")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                row.get("zh_text_normalized")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or("");
        let embedding_text = build_embedding_text(
            row.get("main_title").and_then(|v| v.as_str()).unwrap_or(""),
            row.get("heading").and_then(|v| v.as_str()).unwrap_or(""),
            row.get("period").and_then(|v| v.as_str()).unwrap_or(""),
            text,
        );
        let input_hash = compute_input_hash(&embedding_text, DOCUMENT_TEMPLATE_ID);
        passage_meta.insert(passage_id.to_string(), (embedding_text, input_hash));
    }
    eprintln!("       {} passages read from parquet", passage_meta.len());

    // 3. Load embedding cache
    eprintln!(
        "[3/5] Loading embedding cache for {} (dim {})...",
        model_id, dim
    );
    #[cfg_attr(not(feature = "local-embeddings"), allow(unused_mut))]
    let mut embedding_cache = EmbeddingCache::load_or_empty(&config.cache_path, model_id, dim)?;
    eprintln!(
        "       {} cache hits for this model",
        embedding_cache.records.len()
    );

    // 4. Collect passages needing embedding
    let mut pending: Vec<super::provider::EmbeddingInput> = Vec::new();
    let mut missing_from_parquet = 0usize;
    for (doc_id, passage_id) in doc_table.passage_ids.iter().enumerate() {
        let doc_id = doc_id as u32;
        let Some((embedding_text, input_hash)) = passage_meta.get(passage_id) else {
            missing_from_parquet += 1;
            continue;
        };
        if embedding_cache.has_valid(doc_id, passage_id, input_hash) {
            continue;
        }
        pending.push(super::provider::EmbeddingInput {
            doc_id,
            passage_id: passage_id.clone(),
            embedding_text: embedding_text.clone(),
            input_hash: input_hash.clone(),
        });
    }
    if missing_from_parquet > 0 {
        eprintln!(
            "       Warning: {} doc_table passages have no parquet row",
            missing_from_parquet
        );
    }
    eprintln!(
        "[4/5] {} passages to embed ({} already cached)",
        pending.len(),
        doc_table.passage_ids.len() - pending.len() - missing_from_parquet
    );

    // 5. Embed missing passages with fastembed
    if !pending.is_empty() {
        #[cfg(feature = "local-embeddings")]
        {
            use super::fastembed_provider::FastEmbedProvider;
            use super::provider::EmbeddingProvider;

            eprintln!("       Initializing model {}...", model_id);
            let mut provider = FastEmbedProvider::new(
                config.profile,
                config.model_cache_dir.clone(),
                config.batch_size,
                config.show_download_progress,
            )?;

            let bar = ProgressBar::new(pending.len() as u64);
            bar.set_style(
                ProgressStyle::default_bar()
                    .template(
                        "[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} passages embedded",
                    )
                    .unwrap_or_else(|_| ProgressStyle::default_bar()),
            );

            let mut pending_cache_records: Vec<CacheRecord> = Vec::new();
            let mut appended_cache_records = 0usize;
            for chunk in pending.chunks(config.batch_size.max(1)) {
                let embed_rows = provider.embed_documents(chunk)?;
                for row in embed_rows {
                    let rec = CacheRecord {
                        schema: CACHE_SCHEMA.to_string(),
                        doc_id: row.doc_id,
                        passage_id: row.passage_id.clone(),
                        input_hash: row.input_hash.clone(),
                        provider: provider.provider_id().to_string(),
                        model_id: row.model_id.clone(),
                        model_revision: row.model_revision.clone(),
                        dim: row.dim,
                        document_template_id: DOCUMENT_TEMPLATE_ID.to_string(),
                        document_prefix: provider.document_prefix().to_string(),
                        embedding: row.vector.clone(),
                    };
                    embedding_cache.records.insert(row.doc_id, rec.clone());
                    pending_cache_records.push(rec);
                }
                if pending_cache_records.len() >= 4096 {
                    cache::append_records(&config.cache_path, &pending_cache_records)?;
                    appended_cache_records += pending_cache_records.len();
                    pending_cache_records.clear();
                }
                bar.inc(chunk.len() as u64);
            }
            if !pending_cache_records.is_empty() {
                cache::append_records(&config.cache_path, &pending_cache_records)?;
                appended_cache_records += pending_cache_records.len();
            }
            bar.finish_with_message("done");

            eprintln!(
                "       Appended {} new embeddings to cache {}",
                appended_cache_records,
                config.cache_path.display()
            );
        }

        #[cfg(not(feature = "local-embeddings"))]
        {
            if config.fail_if_feature_missing {
                anyhow::bail!(
                    "{} passages need embedding but this binary was built without the \
                     `local-embeddings` feature. Rebuild with:\n  \
                     cargo +stable-x86_64-pc-windows-msvc build --release --features local-embeddings",
                    pending.len()
                );
            } else {
                eprintln!(
                    "       Note: vector indexing skipped ({} passages unembedded). \
                     Binary was not built with --features local-embeddings.",
                    pending.len()
                );
                return Ok(());
            }
        }
    }

    // 6. Build vector index from cache (all valid doc_ids for current doc_table)
    let mut index_rows: Vec<EmbeddingRecord> = Vec::new();
    for (doc_id, passage_id) in doc_table.passage_ids.iter().enumerate() {
        let doc_id = doc_id as u32;
        let Some((_, input_hash)) = passage_meta.get(passage_id) else {
            continue;
        };
        let Some(rec) = embedding_cache.valid_record(doc_id, passage_id, input_hash) else {
            continue;
        };
        index_rows.push(EmbeddingRecord {
            doc_id,
            embedding: rec.embedding.clone(),
        });
    }

    eprintln!(
        "[5/5] Building vector index from {} embeddings -> {}",
        index_rows.len(),
        config.vector_out.display()
    );

    let metadata = VectorBuildMetadata {
        source_fingerprint: None,
        embedding_text_template: format!(
            "Work: {{main_title}}\\nSection: {{heading}}\\nPeriod: {{period}}\\nText:\\n{{text}}"
        ),
        input_text_field_policy: format!("sinorag_doc_v1 / fastembed-rs / {}", model_id),
        truncation_policy: "model_default".to_string(),
        max_input_chars: None,
        pooling: None,
        instruction: None,
    };

    let header = vector_index::build_from_rows(
        &doc_table,
        index_rows,
        &config.vector_out,
        model_id.to_string(),
        "local".to_string(),
        metadata,
        config.hnsw,
    )?;

    eprintln!(
        "Done. Vector index: {} rows, dim {}, at {}",
        header.row_count,
        header.embedding_dim,
        config.vector_out.display()
    );
    Ok(())
}

pub fn build_embedding_text(main_title: &str, heading: &str, period: &str, text: &str) -> String {
    let mut s = String::new();
    if !main_title.is_empty() {
        s.push_str("Work: ");
        s.push_str(main_title);
        s.push('\n');
    }
    if !heading.is_empty() {
        s.push_str("Section: ");
        s.push_str(heading);
        s.push('\n');
    }
    if !period.is_empty() {
        s.push_str("Period: ");
        s.push_str(period);
        s.push('\n');
    }
    s.push_str("Text:\n");
    s.push_str(text);
    s
}

pub fn compute_input_hash(embedding_text: &str, template_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(template_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(embedding_text.as_bytes());
    hex::encode(hasher.finalize())
}
