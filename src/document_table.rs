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

    // Stable doc_id order (index == doc_id).
    pub passage_ids: Vec<String>,

    // Binary-search reverse lookup. Contains doc_ids sorted by the
    // corresponding passage_id string. Replaces passage_id_map HashMap —
    // saves roughly half the serialized file size.
    pub passage_lookup_order: Vec<u32>,

    // Dense doc_id-indexed metadata arrays (index == doc_id).
    pub source_work_ids: Vec<u32>,
    pub period_ranks: Vec<i32>,

    // Work intern table in stable insertion order (index == work_id).
    // work_id() uses linear search — the table is typically a few thousand
    // entries, so O(N) is fine and avoids a serialized HashMap.
    pub work_strings: Vec<String>,

    // Work → doc_ids CSR (Compressed Sparse Row).
    // work_doc_ids[work_doc_offsets[w] .. work_doc_offsets[w+1]] gives all
    // doc_ids belonging to work w, sorted ascending.
    pub work_doc_offsets: Vec<u32>,
    pub work_doc_ids: Vec<u32>,
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
            schema: "readzen-document-table-v2".to_string(),
            source_fingerprint: String::new(),
            passage_ids: Vec::new(),
            passage_lookup_order: Vec::new(),
            source_work_ids: Vec::new(),
            period_ranks: Vec::new(),
            work_strings: Vec::new(),
            work_doc_offsets: Vec::new(),
            work_doc_ids: Vec::new(),
        }
    }

    /// Build from scratch. doc_ids assigned in sorted-passage_id order.
    pub fn from_parquet(parquet_path: &Path) -> Result<Self> {
        let mut rows = scan_parquet_rows(parquet_path)?;
        eprintln!("Total rows scanned: {}", rows.len());

        // Sort by passage_id then dedup — keeps all metadata aligned.
        rows.sort_unstable_by(|a, b| a.passage_id.cmp(&b.passage_id));
        rows.dedup_by(|a, b| a.passage_id == b.passage_id);

        let n = rows.len();
        let mut passage_ids      = Vec::with_capacity(n);
        let mut source_work_ids  = Vec::with_capacity(n);
        let mut period_ranks     = Vec::with_capacity(n);

        let mut work_strings: Vec<String> = Vec::new();
        let mut work_intern: FxHashMap<String, u32> = FxHashMap::default();

        for row in rows {
            let work_id = intern_work(&mut work_strings, &mut work_intern, &row.source_work_id);
            passage_ids.push(row.passage_id);
            source_work_ids.push(work_id);
            period_ranks.push(row.period_rank);
        }

        // passage_ids is already sorted, so lookup_order is the identity.
        let passage_lookup_order: Vec<u32> = (0..n as u32).collect();

        let (work_doc_offsets, work_doc_ids) =
            build_work_doc_csr(&source_work_ids, work_strings.len());

        let fingerprint = fingerprint_passage_ids(&passage_ids, None);

        let dt = Self {
            schema: "readzen-document-table-v2".to_string(),
            source_fingerprint: fingerprint,
            passage_ids,
            passage_lookup_order,
            source_work_ids,
            period_ranks,
            work_strings,
            work_doc_offsets,
            work_doc_ids,
        };
        eprintln!("DocumentTable built: {} passages, {} unique works",
            dt.passage_ids.len(), dt.work_strings.len());
        Ok(dt)
    }

    /// Extend `base` with any passages in `parquet_path` not already present.
    /// Existing doc_ids are preserved exactly.
    pub fn append_from_parquet(base: &DocumentTable, parquet_path: &Path) -> Result<Self> {
        let all_rows = scan_parquet_rows(parquet_path)?;
        eprintln!("Scanned {} rows; base has {} passages",
            all_rows.len(), base.passage_ids.len());

        // Collect only rows whose passage_id isn't already in base.
        let mut new_rows: Vec<DocRow> = all_rows
            .into_iter()
            .filter(|r| base.doc_id(&r.passage_id).is_none())
            .collect();

        // Sort + dedup the new rows by passage_id.
        new_rows.sort_unstable_by(|a, b| a.passage_id.cmp(&b.passage_id));
        new_rows.dedup_by(|a, b| a.passage_id == b.passage_id);
        eprintln!("Appending {} new passages", new_rows.len());

        // Merge: base first (preserves base doc_ids), then new passages.
        let total = base.passage_ids.len() + new_rows.len();
        let mut passage_ids     = Vec::with_capacity(total);
        let mut source_work_ids = Vec::with_capacity(total);
        let mut period_ranks    = Vec::with_capacity(total);

        passage_ids.extend_from_slice(&base.passage_ids);
        source_work_ids.extend_from_slice(&base.source_work_ids);
        period_ranks.extend_from_slice(&base.period_ranks);

        // Extend work table from base, then intern new works by appending.
        let mut work_strings = base.work_strings.clone();
        let mut work_intern: FxHashMap<String, u32> = work_strings
            .iter()
            .enumerate()
            .map(|(i, s)| (s.clone(), i as u32))
            .collect();

        for row in new_rows {
            let work_id = intern_work(&mut work_strings, &mut work_intern, &row.source_work_id);
            passage_ids.push(row.passage_id);
            source_work_ids.push(work_id);
            period_ranks.push(row.period_rank);
        }

        // Rebuild lookup order over the full merged set.
        let mut passage_lookup_order: Vec<u32> = (0..passage_ids.len() as u32).collect();
        passage_lookup_order
            .sort_unstable_by(|&a, &b| passage_ids[a as usize].cmp(&passage_ids[b as usize]));

        let (work_doc_offsets, work_doc_ids) =
            build_work_doc_csr(&source_work_ids, work_strings.len());

        let fingerprint = fingerprint_passage_ids(&passage_ids, Some(&base.source_fingerprint));

        Ok(Self {
            schema: "readzen-document-table-v2".to_string(),
            source_fingerprint: fingerprint,
            passage_ids,
            passage_lookup_order,
            source_work_ids,
            period_ranks,
            work_strings,
            work_doc_offsets,
            work_doc_ids,
        })
    }

    /// O(log N) reverse lookup via sorted `passage_lookup_order`.
    pub fn doc_id(&self, passage_id: &str) -> Option<DocId> {
        self.passage_lookup_order
            .binary_search_by(|&doc_id| {
                self.passage_ids[doc_id as usize].as_str().cmp(passage_id)
            })
            .ok()
            .map(|i| self.passage_lookup_order[i])
    }

    pub fn passage_id(&self, doc_id: DocId) -> Option<&str> {
        self.passage_ids.get(doc_id as usize).map(String::as_str)
    }

    /// O(N) linear search — work table is typically a few thousand entries.
    pub fn work_id(&self, work_name: &str) -> Option<u32> {
        self.work_strings.iter().position(|s| s == work_name).map(|i| i as u32)
    }

    pub fn work_name(&self, work_id: u32) -> Option<&str> {
        self.work_strings.get(work_id as usize).map(String::as_str)
    }

    /// All doc_ids belonging to `work_id`, sorted ascending. O(1) slice.
    pub fn doc_ids_for_work(&self, work_id: u32) -> &[u32] {
        let w = work_id as usize;
        if w + 1 >= self.work_doc_offsets.len() {
            return &[];
        }
        let start = self.work_doc_offsets[w] as usize;
        let end   = self.work_doc_offsets[w + 1] as usize;
        &self.work_doc_ids[start..end]
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

struct DocRow {
    passage_id: String,
    source_work_id: String,
    period_rank: i32,
}

fn scan_parquet_rows(parquet_path: &Path) -> Result<Vec<DocRow>> {
    let files = parquet_files(parquet_path)?;
    eprintln!("Found {} parquet files", files.len());
    let mut rows: Vec<DocRow> = Vec::new();
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
                rows.push(DocRow {
                    passage_id: pid_arr.value(i).to_string(),
                    source_work_id: if work_arr.is_null(i) {
                        String::new()
                    } else {
                        work_arr.value(i).to_string()
                    },
                    period_rank: if period_arr.is_null(i) { 0 } else { period_arr.value(i) },
                });
            }
        }
    }
    Ok(rows)
}

/// Intern `name` into `table` (appending if new) and return its stable index.
/// `map` is a build-time only lookup and is NOT serialized.
fn intern_work(
    table: &mut Vec<String>,
    map: &mut FxHashMap<String, u32>,
    name: &str,
) -> u32 {
    if let Some(&id) = map.get(name) {
        return id;
    }
    let id = table.len() as u32;
    table.push(name.to_string());
    map.insert(name.to_string(), id);
    id
}

/// Build a Compressed Sparse Row (CSR) index: work → sorted doc_ids.
fn build_work_doc_csr(source_work_ids: &[u32], num_works: usize) -> (Vec<u32>, Vec<u32>) {
    if num_works == 0 {
        return (Vec::new(), Vec::new());
    }
    // Count docs per work.
    let mut counts = vec![0u32; num_works];
    for &wid in source_work_ids {
        if (wid as usize) < num_works {
            counts[wid as usize] += 1;
        }
    }
    // Build prefix-sum offsets (length num_works + 1).
    let mut offsets = vec![0u32; num_works + 1];
    for i in 0..num_works {
        offsets[i + 1] = offsets[i] + counts[i];
    }
    // Fill doc_ids into each work's slot.
    let total = offsets[num_works] as usize;
    let mut doc_ids = vec![0u32; total];
    let mut cursor = offsets[..num_works].to_vec();
    for (doc_id, &wid) in source_work_ids.iter().enumerate() {
        let w = wid as usize;
        if w < num_works {
            doc_ids[cursor[w] as usize] = doc_id as u32;
            cursor[w] += 1;
        }
    }
    // Sort each work's slice so callers can binary-search if needed.
    for w in 0..num_works {
        let s = offsets[w] as usize;
        let e = offsets[w + 1] as usize;
        doc_ids[s..e].sort_unstable();
    }
    (offsets, doc_ids)
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
