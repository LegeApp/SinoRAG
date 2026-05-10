use crate::phrase_index::parquet_files;
use anyhow::{anyhow, Result};
use arrow::array::{Array, StringArray};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::path::{Path, PathBuf};

pub type DocId = u32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentTable {
    pub schema: String,
    pub source_fingerprint: String,

    // Core mapping
    pub passage_ids: Vec<String>,
    pub passage_id_map: FxHashMap<String, DocId>,

    // Optional acceleration fields (for catalog/TF-IDF)
    pub source_work_ids: Vec<u32>,
    pub period_ranks: Vec<i32>,

    // Work string table (for catalog work_id references)
    pub work_strings: Vec<String>,
    pub work_id_map: FxHashMap<String, u32>,
}

impl DocumentTable {
    pub fn new() -> Self {
        Self {
            schema: "readzen-document-table-v1".to_string(),
            source_fingerprint: String::new(),
            passage_ids: Vec::new(),
            passage_id_map: FxHashMap::default(),
            source_work_ids: Vec::new(),
            period_ranks: Vec::new(),
            work_strings: Vec::new(),
            work_id_map: FxHashMap::default(),
        }
    }

    pub fn from_parquet(parquet_path: &Path) -> Result<Self> {
        let files = parquet_files(parquet_path)?;
        eprintln!("Found {} parquet files", files.len());

        let mut doc_table = Self::new();
        let mut all_passage_ids: Vec<String> = Vec::new();
        let mut all_source_work_ids: Vec<String> = Vec::new();
        let mut all_period_ranks: Vec<i32> = Vec::new();

        for file_path in files {
            let file = File::open(&file_path)?;
            let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
            let reader = builder.build()?;

            for batch_result in reader {
                let batch = batch_result?;

                // Get passage_id column
                let passage_id_col = batch
                    .schema()
                    .column_with_name("passage_id")
                    .ok_or_else(|| anyhow!("Column 'passage_id' not found in Parquet"))?
                    .0;
                let passage_ids = batch
                    .column(passage_id_col)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| anyhow!("passage_id column is not StringArray"))?;

                // Get source_work_id column
                let source_work_id_col = batch
                    .schema()
                    .column_with_name("source_work_id")
                    .ok_or_else(|| anyhow!("Column 'source_work_id' not found in Parquet"))?
                    .0;
                let source_work_ids = batch
                    .column(source_work_id_col)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| anyhow!("source_work_id column is not StringArray"))?;

                // Get period_rank column
                let period_rank_col = batch
                    .schema()
                    .column_with_name("period_rank")
                    .ok_or_else(|| anyhow!("Column 'period_rank' not found in Parquet"))?
                    .0;
                let period_ranks = batch
                    .column(period_rank_col)
                    .as_any()
                    .downcast_ref::<arrow::array::Int32Array>()
                    .ok_or_else(|| anyhow!("period_rank column is not Int32Array"))?;

                for i in 0..batch.num_rows() {
                    if passage_ids.is_null(i) {
                        continue;
                    }

                    all_passage_ids.push(passage_ids.value(i).to_string());
                    all_source_work_ids.push(source_work_ids.value(i).to_string());
                    all_period_ranks.push(if period_ranks.is_null(i) { 0 } else { period_ranks.value(i) });
                }
            }
        }

        eprintln!("Total passages: {}", all_passage_ids.len());

        // Sort passage_ids to ensure stable doc_id assignment
        all_passage_ids.sort();
        all_passage_ids.dedup();

        // Build passage_id -> doc_id map
        let passage_id_map: FxHashMap<String, DocId> = all_passage_ids
            .iter()
            .enumerate()
            .map(|(idx, pid)| (pid.clone(), idx as DocId))
            .collect();

        // Build work string table
        let mut unique_work_ids: Vec<String> = all_source_work_ids.clone();
        unique_work_ids.sort();
        unique_work_ids.dedup();

        let work_id_map: FxHashMap<String, u32> = unique_work_ids
            .iter()
            .enumerate()
            .map(|(idx, wid)| (wid.clone(), idx as u32))
            .collect();

        // Map source_work_ids to u32
        let source_work_ids_u32: Vec<u32> = all_source_work_ids
            .iter()
            .map(|wid| *work_id_map.get(wid).unwrap_or(&0))
            .collect();

        // Re-align period_ranks with sorted passage_ids
        let mut aligned_period_ranks: Vec<i32> = vec![0; all_passage_ids.len()];
        let mut passage_to_period: FxHashMap<String, i32> = FxHashMap::default();

        for (pid, pr) in all_passage_ids.iter().zip(all_period_ranks.iter()) {
            passage_to_period.insert(pid.clone(), *pr);
        }

        for (idx, pid) in all_passage_ids.iter().enumerate() {
            aligned_period_ranks[idx] = *passage_to_period.get(pid).unwrap_or(&0);
        }

        // Calculate fingerprint
        let mut hasher = Sha256::new();
        for pid in &all_passage_ids {
            hasher.update(pid.as_bytes());
            hasher.update(b"\0");
        }
        let fingerprint = format!("{:x}", hasher.finalize());

        doc_table.source_fingerprint = fingerprint;
        doc_table.passage_ids = all_passage_ids;
        doc_table.passage_id_map = passage_id_map;
        doc_table.source_work_ids = source_work_ids_u32;
        doc_table.period_ranks = aligned_period_ranks;
        doc_table.work_strings = unique_work_ids;
        doc_table.work_id_map = work_id_map;

        eprintln!("DocumentTable built: {} passages, {} unique works", 
                  doc_table.passage_ids.len(), doc_table.work_strings.len());

        Ok(doc_table)
    }

    pub fn doc_id(&self, passage_id: &str) -> Option<DocId> {
        self.passage_id_map.get(passage_id).copied()
    }

    pub fn passage_id(&self, doc_id: DocId) -> Option<&str> {
        self.passage_ids.get(doc_id as usize).map(String::as_str)
    }

    pub fn work_id(&self, work_name: &str) -> Option<u32> {
        self.work_id_map.get(work_name).copied()
    }

    pub fn work_name(&self, work_id: u32) -> Option<&str> {
        self.work_strings.get(work_id as usize).map(String::as_str)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let bytes = bincode::serialize(self)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        let doc_table: Self = bincode::deserialize(&bytes)?;
        Ok(doc_table)
    }

    pub fn save_atomic(&self, path: &Path) -> Result<()> {
        let bytes = bincode::serialize(self)?;
        let temp_path = path.with_extension("tmp");
        std::fs::write(&temp_path, bytes)?;
        std::fs::rename(&temp_path, path)?;
        Ok(())
    }
}
