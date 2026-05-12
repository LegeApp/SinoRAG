//! DocumentTable: canonical `doc_id ↔ passage_id` mapping.
//!
//! Two construction modes:
//! - `from_parquet`: build from scratch. doc_ids assigned in sorted
//!   passage_id order (stable, deterministic).
//! - `append_from_parquet`: extend an existing DocumentTable in place.
//!   Existing passages keep their doc_ids exactly; new passages get appended
//!   ids starting at `existing.passage_ids.len()`. The lineage record
//!   (`base_fingerprint` / `base_doc_count`) is written to a sidecar
//!   `<path>.lineage.json` so the binary layout of `doc_table.bin` doesn't
//!   change — existing on-disk indexes built against the predecessor remain
//!   loadable as long as the validator accepts the base fingerprint.

use crate::phrase_index::parquet_files;
use anyhow::{anyhow, Result};
use arrow::array::{Array, StringArray};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
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

    // Optional acceleration fields
    pub source_work_ids: Vec<u32>,
    pub period_ranks: Vec<i32>,

    // Work string table
    pub work_strings: Vec<String>,
    pub work_id_map: FxHashMap<String, u32>,
}

/// Lineage info stored alongside `doc_table.bin` as `doc_table.bin.lineage.json`.
/// Optional; absent for first-time builds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocTableLineage {
    pub schema: String,
    /// Fingerprint of the predecessor doc table this one extends.
    pub base_fingerprint: String,
    /// Count of doc_ids inherited from the predecessor. New passages have
    /// `doc_id >= base_doc_count`.
    pub base_doc_count: u32,
}

impl DocTableLineage {
    pub fn new(base_fingerprint: String, base_doc_count: u32) -> Self {
        Self {
            schema: "readzen-doc-table-lineage-v1".to_string(),
            base_fingerprint,
            base_doc_count,
        }
    }
    pub fn sidecar_path(doc_table_path: &Path) -> PathBuf {
        let mut s = doc_table_path.as_os_str().to_os_string();
        s.push(".lineage.json");
        PathBuf::from(s)
    }
    pub fn load_if_present(doc_table_path: &Path) -> Result<Option<Self>> {
        let p = Self::sidecar_path(doc_table_path);
        if !p.exists() { return Ok(None); }
        let bytes = std::fs::read(&p)?;
        let v: Self = serde_json::from_slice(&bytes)
            .map_err(|e| anyhow!("parse {}: {}", p.display(), e))?;
        Ok(Some(v))
    }
    pub fn write(&self, doc_table_path: &Path) -> Result<()> {
        let p = Self::sidecar_path(doc_table_path);
        std::fs::write(p, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
    pub fn delete_if_present(doc_table_path: &Path) -> Result<()> {
        let p = Self::sidecar_path(doc_table_path);
        if p.exists() { std::fs::remove_file(p)?; }
        Ok(())
    }
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

    /// Build from scratch. doc_ids assigned in sorted-passage_id order.
    pub fn from_parquet(parquet_path: &Path) -> Result<Self> {
        let scan = scan_parquet(parquet_path)?;
        eprintln!("Total passages scanned: {}", scan.passage_ids.len());

        let mut indices: Vec<usize> = (0..scan.passage_ids.len()).collect();
        indices.sort_by(|a, b| scan.passage_ids[*a].cmp(&scan.passage_ids[*b]));
        let mut seen: FxHashMap<&str, ()> = FxHashMap::default();
        let mut keep: Vec<usize> = Vec::with_capacity(indices.len());
        for i in &indices {
            let pid: &str = &scan.passage_ids[*i];
            if !seen.contains_key(pid) {
                seen.insert(pid, ());
                keep.push(*i);
            }
        }
        let passage_ids: Vec<String> = keep.iter().map(|i| scan.passage_ids[*i].clone()).collect();
        let work_names:  Vec<String> = keep.iter().map(|i| scan.source_work_ids[*i].clone()).collect();
        let periods:     Vec<i32>    = keep.iter().map(|i| scan.period_ranks[*i]).collect();

        let (work_strings, work_id_map, work_id_u32) = build_work_table(&work_names);
        let passage_id_map: FxHashMap<String, DocId> = passage_ids.iter().enumerate()
            .map(|(idx, pid)| (pid.clone(), idx as DocId)).collect();
        let fingerprint = fingerprint_passage_ids(&passage_ids, None);

        let dt = Self {
            schema: "readzen-document-table-v1".to_string(),
            source_fingerprint: fingerprint,
            passage_ids,
            passage_id_map,
            source_work_ids: work_id_u32,
            period_ranks: periods,
            work_strings,
            work_id_map,
        };
        eprintln!("DocumentTable built: {} passages, {} unique works",
            dt.passage_ids.len(), dt.work_strings.len());
        Ok(dt)
    }

    /// Extend `base` with any passages in `parquet_path` not already in
    /// `base.passage_id_map`. Existing doc_ids are preserved exactly.
    pub fn append_from_parquet(base: &DocumentTable, parquet_path: &Path) -> Result<Self> {
        let scan = scan_parquet(parquet_path)?;
        eprintln!("Scanned {} rows; base has {} passages",
            scan.passage_ids.len(), base.passage_ids.len());

        let mut new_indices: Vec<usize> = (0..scan.passage_ids.len())
            .filter(|i| !base.passage_id_map.contains_key(&scan.passage_ids[*i]))
            .collect();
        new_indices.sort_by(|a, b| scan.passage_ids[*a].cmp(&scan.passage_ids[*b]));
        let mut seen: FxHashMap<&str, ()> = FxHashMap::default();
        let mut keep: Vec<usize> = Vec::new();
        for i in &new_indices {
            let pid: &str = &scan.passage_ids[*i];
            if !seen.contains_key(pid) {
                seen.insert(pid, ());
                keep.push(*i);
            }
        }
        eprintln!("Appending {} new passages", keep.len());

        // Merge: base first, then new (preserves base doc_ids).
        let mut passage_ids: Vec<String> = Vec::with_capacity(base.passage_ids.len() + keep.len());
        passage_ids.extend_from_slice(&base.passage_ids);
        for i in &keep { passage_ids.push(scan.passage_ids[*i].clone()); }

        let mut work_strings = base.work_strings.clone();
        let mut work_id_map  = base.work_id_map.clone();
        let mut source_work_ids = base.source_work_ids.clone();
        let mut period_ranks    = base.period_ranks.clone();
        for i in &keep {
            let wname = &scan.source_work_ids[*i];
            let wid = match work_id_map.get(wname) {
                Some(v) => *v,
                None => {
                    let id = work_strings.len() as u32;
                    work_id_map.insert(wname.clone(), id);
                    work_strings.push(wname.clone());
                    id
                }
            };
            source_work_ids.push(wid);
            period_ranks.push(scan.period_ranks[*i]);
        }

        let passage_id_map: FxHashMap<String, DocId> = passage_ids.iter().enumerate()
            .map(|(idx, pid)| (pid.clone(), idx as DocId)).collect();
        let fingerprint = fingerprint_passage_ids(&passage_ids, Some(&base.source_fingerprint));

        Ok(Self {
            schema: "readzen-document-table-v1".to_string(),
            source_fingerprint: fingerprint,
            passage_ids,
            passage_id_map,
            source_work_ids,
            period_ranks,
            work_strings,
            work_id_map,
        })
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
        std::fs::write(path, bincode::serialize(self)?)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        let dt: Self = bincode::deserialize(&bytes)
            .map_err(|e| anyhow!("deserialize {}: {}", path.display(), e))?;
        Ok(dt)
    }

    pub fn save_atomic(&self, path: &Path) -> Result<()> {
        let bytes = bincode::serialize(self)?;
        let temp_path = path.with_extension("tmp");
        std::fs::write(&temp_path, bytes)?;
        std::fs::rename(&temp_path, path)?;
        Ok(())
    }
}

/// How well an index's `doc_table_fingerprint` matches the doc_table on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexCoverage {
    /// Index fingerprint == current doc_table fingerprint. Covers every doc_id.
    Full,
    /// Index fingerprint == lineage `base_fingerprint`. Covers `0..base_doc_count`;
    /// higher doc_ids exist in the doc_table but not in this index.
    Base { base_doc_count: u32 },
}

/// Decide whether an index header's `index_fingerprint` is compatible with
/// the doc_table on disk. Returns `Some(Full|Base)` if compatible, `None`
/// if mismatched.
pub fn match_index_fingerprint(
    doc_table: &DocumentTable,
    doc_table_path: &Path,
    index_fingerprint: &str,
) -> Result<Option<IndexCoverage>> {
    if index_fingerprint == doc_table.source_fingerprint {
        return Ok(Some(IndexCoverage::Full));
    }
    if let Some(lineage) = DocTableLineage::load_if_present(doc_table_path)? {
        if index_fingerprint == lineage.base_fingerprint {
            return Ok(Some(IndexCoverage::Base { base_doc_count: lineage.base_doc_count }));
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------

struct ParquetScan {
    passage_ids: Vec<String>,
    source_work_ids: Vec<String>,
    period_ranks: Vec<i32>,
}

fn scan_parquet(parquet_path: &Path) -> Result<ParquetScan> {
    let files = parquet_files(parquet_path)?;
    eprintln!("Found {} parquet files", files.len());
    let mut pids = Vec::<String>::new();
    let mut wids = Vec::<String>::new();
    let mut prs  = Vec::<i32>::new();
    for file_path in files {
        let f = File::open(&file_path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(f)?;
        let reader = builder.build()?;
        for batch in reader {
            let batch = batch?;
            let passage_col = batch.schema().column_with_name("passage_id")
                .ok_or_else(|| anyhow!("Column 'passage_id' missing"))?.0;
            let work_col = batch.schema().column_with_name("source_work_id")
                .ok_or_else(|| anyhow!("Column 'source_work_id' missing"))?.0;
            let period_col = batch.schema().column_with_name("period_rank")
                .ok_or_else(|| anyhow!("Column 'period_rank' missing"))?.0;
            let pid_arr = batch.column(passage_col).as_any().downcast_ref::<StringArray>()
                .ok_or_else(|| anyhow!("passage_id not StringArray"))?;
            let work_arr = batch.column(work_col).as_any().downcast_ref::<StringArray>()
                .ok_or_else(|| anyhow!("source_work_id not StringArray"))?;
            let period_arr = batch.column(period_col).as_any()
                .downcast_ref::<arrow::array::Int32Array>()
                .ok_or_else(|| anyhow!("period_rank not Int32Array"))?;
            for i in 0..batch.num_rows() {
                if pid_arr.is_null(i) { continue; }
                pids.push(pid_arr.value(i).to_string());
                wids.push(if work_arr.is_null(i) { String::new() } else { work_arr.value(i).to_string() });
                prs.push(if period_arr.is_null(i) { 0 } else { period_arr.value(i) });
            }
        }
    }
    Ok(ParquetScan { passage_ids: pids, source_work_ids: wids, period_ranks: prs })
}

fn build_work_table(work_names: &[String]) -> (Vec<String>, FxHashMap<String, u32>, Vec<u32>) {
    let mut unique: Vec<String> = work_names.to_vec();
    unique.sort();
    unique.dedup();
    let map: FxHashMap<String, u32> = unique.iter().enumerate()
        .map(|(i, w)| (w.clone(), i as u32)).collect();
    let ids: Vec<u32> = work_names.iter().map(|w| *map.get(w).unwrap_or(&0)).collect();
    (unique, map, ids)
}

fn fingerprint_passage_ids(passage_ids: &[String], base: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    if let Some(b) = base { hasher.update(b.as_bytes()); hasher.update(b"\n"); }
    for pid in passage_ids {
        hasher.update(pid.as_bytes());
        hasher.update(b"\0");
    }
    format!("{:x}", hasher.finalize())
}
