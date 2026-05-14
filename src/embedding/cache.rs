use anyhow::{Context, Result};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

pub const CACHE_SCHEMA: &str = "sinorag_embedding_cache_v1";
pub const DOCUMENT_TEMPLATE_ID: &str = "sinorag_doc_v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheRecord {
    pub schema: String,
    pub doc_id: u32,
    pub passage_id: String,
    pub input_hash: String,
    pub provider: String,
    pub model_id: String,
    pub model_revision: Option<String>,
    pub dim: usize,
    pub document_template_id: String,
    pub document_prefix: String,
    pub embedding: Vec<f32>,
}

pub struct EmbeddingCache {
    /// Latest valid record per doc_id (for the configured model/dim).
    pub records: FxHashMap<u32, CacheRecord>,
}

impl EmbeddingCache {
    /// Load cache from disk, keeping only entries matching model_id and dim.
    /// If the file doesn't exist, returns an empty cache.
    /// Latest row per doc_id wins (supports append-only growth).
    pub fn load_or_empty(path: &Path, model_id: &str, dim: usize) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                records: FxHashMap::default(),
            });
        }
        let file = File::open(path).with_context(|| format!("open cache {}", path.display()))?;
        let reader = BufReader::new(file);
        let mut records: FxHashMap<u32, CacheRecord> = FxHashMap::default();
        let mut line_no = 0usize;
        for line in reader.lines() {
            line_no += 1;
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let rec: CacheRecord = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!(
                        "Warning: skipping unparseable cache line {}: {}",
                        line_no, e
                    );
                    continue;
                }
            };
            if rec.model_id != model_id || rec.dim != dim {
                continue;
            }
            // Latest row for a doc_id supersedes earlier rows
            records.insert(rec.doc_id, rec);
        }
        Ok(Self { records })
    }

    /// Returns true if the cache has a valid entry for the current passage and text.
    pub fn has_valid(&self, doc_id: u32, passage_id: &str, input_hash: &str) -> bool {
        self.valid_record(doc_id, passage_id, input_hash).is_some()
    }

    pub fn valid_record(
        &self,
        doc_id: u32,
        passage_id: &str,
        input_hash: &str,
    ) -> Option<&CacheRecord> {
        let rec = self.records.get(&doc_id)?;
        if rec.schema == CACHE_SCHEMA
            && rec.document_template_id == DOCUMENT_TEMPLATE_ID
            && rec.passage_id == passage_id
            && rec.input_hash == input_hash
        {
            Some(rec)
        } else {
            None
        }
    }
}

/// Append records to the cache file (creates it if missing).
pub fn append_records(path: &Path, records: &[CacheRecord]) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open cache for append {}", path.display()))?;
    let mut writer = BufWriter::new(&mut file);
    for rec in records {
        serde_json::to_writer(&mut writer, rec)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_record_requires_passage_and_template_match() {
        let rec = CacheRecord {
            schema: CACHE_SCHEMA.to_string(),
            doc_id: 7,
            passage_id: "p7".to_string(),
            input_hash: "hash".to_string(),
            provider: "test".to_string(),
            model_id: "model".to_string(),
            model_revision: None,
            dim: 2,
            document_template_id: DOCUMENT_TEMPLATE_ID.to_string(),
            document_prefix: "passage: ".to_string(),
            embedding: vec![1.0, 0.0],
        };
        let mut records = FxHashMap::default();
        records.insert(7, rec);
        let cache = EmbeddingCache { records };

        assert!(cache.has_valid(7, "p7", "hash"));
        assert!(!cache.has_valid(7, "other", "hash"));
        assert!(!cache.has_valid(7, "p7", "other"));
        assert!(!cache.has_valid(8, "p7", "hash"));
    }
}
