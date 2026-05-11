// TF-IDF index over CJK character n-grams. Single canonical implementation.
//
// Pipeline:
//   Phase 1 — DF buckets:    write (gram_hash, doc_id) records per bucket.
//   Phase 2 — Build vocab:   merge buckets, count DF, filter by min/max DF +
//                            max_features, assign term_ids.
//   Phase 3 — Rows + posting buckets: re-scan parquet, compute TF, multiply
//             by IDF, L2-normalise, write row entries inline + write
//             (term_id, doc_id, weight) records into posting buckets.
//   Phase 4 — Posting merge: for each posting bucket, sort by (term_id, doc_id)
//             and emit a contiguous posting list per term.
//   Save  —   write the file directly from the build's local Vecs (we never
//             construct an in-memory `TfidfIndex` of an entire corpus).
//
// On-disk format ("SGTFIDF2", version 2): header (512 B) + vocab table +
// IDF array + per-doc row offsets + per-doc row lengths + row blob +
// postings blob. See header constant ranges below.
//
// Query side: `TfidfIndex::open` mmaps the file. Every accessor is a slice
// into the mmap; no full-file copy. Loading a multi-GB index costs O(1) RAM.

use crate::document_table::DocumentTable;
use crate::tfidf::ngram::{
    char_ngram_hashes, char_ngram_hashes_all_into, char_ngram_hashes_into, char_ngrams,
};
use anyhow::Result;
use memmap2::Mmap;
use ordered_float::OrderedFloat;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use xxhash_rust::xxh3::xxh3_64;

pub type DocId = u32;
pub type TermId = u32;

// ---------------------------------------------------------------------------
// File format constants
// ---------------------------------------------------------------------------

const MAGIC_TFIDF_V2: &[u8; 8] = b"SGTFIDF2";
const HEADER_SIZE: usize = 512;
const VOCAB_ENTRY_SIZE: usize = 28;       // u64 + u32 + u32 + u64 + u32
const ROW_ENTRY_SIZE: usize = 8;          // term_id u32 + weight f32
const POSTING_ENTRY_SIZE: usize = 8;      // doc_id u32 + weight f32
const POSTING_BUCKET_RECORD_SIZE: usize = 12; // term_id u32 + doc_id u32 + weight f32

const HDR_VOCAB_COUNT:    std::ops::Range<usize> = 10..14;
const HDR_DOC_COUNT:      std::ops::Range<usize> = 14..18;
const HDR_MIN_NGRAM:      std::ops::Range<usize> = 18..20;
const HDR_MAX_NGRAM:      std::ops::Range<usize> = 20..22;
const HDR_MIN_DF:         std::ops::Range<usize> = 22..26;
const HDR_MAX_FEATURES:   std::ops::Range<usize> = 26..30;
const HDR_MAX_DF_RATIO:   std::ops::Range<usize> = 30..34;
const HDR_FP:             std::ops::Range<usize> = 34..98;
const HDR_VOCAB_OFF:      std::ops::Range<usize> = 98..106;
const HDR_IDF_OFF:        std::ops::Range<usize> = 106..114;
const HDR_ROW_OFFSETS:    std::ops::Range<usize> = 114..122;
const HDR_ROW_LENGTHS:    std::ops::Range<usize> = 122..130;
const HDR_ROW_BLOB_OFF:   std::ops::Range<usize> = 130..138;
const HDR_ROW_BLOB_LEN:   std::ops::Range<usize> = 138..146;
const HDR_POST_BLOB_OFF:  std::ops::Range<usize> = 146..154;
const HDR_POST_BLOB_LEN:  std::ops::Range<usize> = 154..162;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TfidfParams {
    pub min_ngram: usize,
    pub max_ngram: usize,
    pub min_df: u32,
    pub max_df_ratio: f32,
    pub max_features: usize,
    pub dtype: String,
    pub analyzer: String,
}

impl TfidfParams {
    pub fn default_v2() -> Self {
        Self {
            min_ngram: 5,
            max_ngram: 8,
            min_df: 5,
            max_df_ratio: 0.05,
            max_features: 200_000,
            dtype: "float32".to_string(),
            analyzer: "char".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct VocabEntry {
    pub term_hash: u64,
    pub term_id: TermId,
    pub df: u32,
    pub postings_offset: u64,
    pub postings_count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SharedNgram {
    pub ngram: String,
    pub contribution: f32,
    pub seed_weight: f32,
    pub candidate_weight: f32,
}

/// Mmap-backed TF-IDF index. All sections are accessed by slicing into the
/// mmap; no copies of the row blob / postings blob are kept in RAM.
pub struct TfidfIndex {
    params: TfidfParams,
    doc_table_fingerprint: String,
    vocab_count: usize,
    doc_count: usize,
    vocab_off: usize,
    idf_off: usize,
    row_offsets_off: usize,
    row_lengths_off: usize,
    row_blob_off: usize,
    row_blob_len: usize,
    postings_blob_off: usize,
    postings_blob_len: usize,
    mmap: Arc<Mmap>,
}

// ---------------------------------------------------------------------------
// Open + accessors
// ---------------------------------------------------------------------------

impl TfidfIndex {
    /// Open by mmapping the index file. Constant RAM cost regardless of size.
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        Self::from_mmap(mmap)
    }

    /// Backwards-compat alias. Prefer `open`.
    pub fn load(path: &Path) -> Result<Self> {
        Self::open(path)
    }

    fn from_mmap(mmap: Mmap) -> Result<Self> {
        if mmap.len() < HEADER_SIZE {
            anyhow::bail!("TF-IDF index too small");
        }
        if &mmap[0..8] != MAGIC_TFIDF_V2 {
            anyhow::bail!("invalid TF-IDF magic");
        }
        let version = u16::from_le_bytes([mmap[8], mmap[9]]);
        if version != 2 {
            anyhow::bail!("unsupported TF-IDF version: {}", version);
        }

        let vocab_count  = u32::from_le_bytes(mmap[HDR_VOCAB_COUNT].try_into()?) as usize;
        let doc_count    = u32::from_le_bytes(mmap[HDR_DOC_COUNT].try_into()?) as usize;
        let min_ngram    = u16::from_le_bytes(mmap[HDR_MIN_NGRAM].try_into()?) as usize;
        let max_ngram    = u16::from_le_bytes(mmap[HDR_MAX_NGRAM].try_into()?) as usize;
        let min_df       = u32::from_le_bytes(mmap[HDR_MIN_DF].try_into()?);
        let max_features = u32::from_le_bytes(mmap[HDR_MAX_FEATURES].try_into()?) as usize;
        let max_df_ratio = f32::from_le_bytes(mmap[HDR_MAX_DF_RATIO].try_into()?);
        let fp_end = mmap[HDR_FP].iter().position(|&b| b == 0).unwrap_or(HDR_FP.len());
        let doc_table_fingerprint =
            String::from_utf8_lossy(&mmap[HDR_FP.start..HDR_FP.start + fp_end]).to_string();

        let vocab_off       = u64::from_le_bytes(mmap[HDR_VOCAB_OFF].try_into()?) as usize;
        let idf_off         = u64::from_le_bytes(mmap[HDR_IDF_OFF].try_into()?) as usize;
        let row_offsets_off = u64::from_le_bytes(mmap[HDR_ROW_OFFSETS].try_into()?) as usize;
        let row_lengths_off = u64::from_le_bytes(mmap[HDR_ROW_LENGTHS].try_into()?) as usize;
        let row_blob_off    = u64::from_le_bytes(mmap[HDR_ROW_BLOB_OFF].try_into()?) as usize;
        let row_blob_len    = u64::from_le_bytes(mmap[HDR_ROW_BLOB_LEN].try_into()?) as usize;
        let postings_blob_off = u64::from_le_bytes(mmap[HDR_POST_BLOB_OFF].try_into()?) as usize;
        let postings_blob_len = u64::from_le_bytes(mmap[HDR_POST_BLOB_LEN].try_into()?) as usize;

        if postings_blob_off + postings_blob_len > mmap.len() {
            anyhow::bail!("TF-IDF sections exceed file length");
        }

        let params = TfidfParams {
            min_ngram,
            max_ngram,
            min_df,
            max_df_ratio,
            max_features,
            dtype: "float32".to_string(),
            analyzer: "char".to_string(),
        };

        Ok(Self {
            params,
            doc_table_fingerprint,
            vocab_count,
            doc_count,
            vocab_off,
            idf_off,
            row_offsets_off,
            row_lengths_off,
            row_blob_off,
            row_blob_len,
            postings_blob_off,
            postings_blob_len,
            mmap: Arc::new(mmap),
        })
    }

    pub fn params(&self) -> &TfidfParams { &self.params }
    pub fn doc_count(&self) -> usize { self.doc_count }
    pub fn vocab_count(&self) -> usize { self.vocab_count }
    pub fn doc_table_fingerprint(&self) -> &str { &self.doc_table_fingerprint }

    pub fn vocab_entry(&self, term_id: TermId) -> VocabEntry {
        let off = self.vocab_off + (term_id as usize) * VOCAB_ENTRY_SIZE;
        let s = &self.mmap[off..off + VOCAB_ENTRY_SIZE];
        VocabEntry {
            term_hash:       u64::from_le_bytes(s[0..8].try_into().unwrap()),
            term_id:         u32::from_le_bytes(s[8..12].try_into().unwrap()),
            df:              u32::from_le_bytes(s[12..16].try_into().unwrap()),
            postings_offset: u64::from_le_bytes(s[16..24].try_into().unwrap()),
            postings_count:  u32::from_le_bytes(s[24..28].try_into().unwrap()),
        }
    }

    fn idf_at(&self, term_id: TermId) -> f32 {
        let off = self.idf_off + (term_id as usize) * 4;
        f32::from_le_bytes(self.mmap[off..off + 4].try_into().unwrap())
    }

    fn row_offset_at(&self, doc_id: DocId) -> u64 {
        let off = self.row_offsets_off + (doc_id as usize) * 8;
        u64::from_le_bytes(self.mmap[off..off + 8].try_into().unwrap())
    }

    fn row_length_at(&self, doc_id: DocId) -> u32 {
        let off = self.row_lengths_off + (doc_id as usize) * 4;
        u32::from_le_bytes(self.mmap[off..off + 4].try_into().unwrap())
    }

    /// Decode a single document's row into (term_id, weight) pairs.
    pub fn row_entries(&self, doc_id: DocId) -> Vec<(TermId, f32)> {
        let idx = doc_id as usize;
        if idx >= self.doc_count {
            return Vec::new();
        }
        let off = self.row_offset_at(doc_id);
        if off == u64::MAX {
            return Vec::new();
        }
        let len = self.row_length_at(doc_id) as usize;
        let base = self.row_blob_off + off as usize;
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            let e = base + i * ROW_ENTRY_SIZE;
            let tid = u32::from_le_bytes(self.mmap[e..e + 4].try_into().unwrap());
            let w   = f32::from_le_bytes(self.mmap[e + 4..e + 8].try_into().unwrap());
            out.push((tid, w));
        }
        out
    }

    /// Cosine similarity via posting lists. Row vectors are L2-normalised at
    /// build time, so dot product == cosine similarity.
    pub fn similar(&self, doc_id: DocId, limit: usize) -> Result<Vec<(DocId, f32)>> {
        let idx = doc_id as usize;
        if idx >= self.doc_count {
            anyhow::bail!("doc_id {} out of range (doc_count={})", doc_id, self.doc_count);
        }
        let row = self.row_entries(doc_id);
        if row.is_empty() {
            return Ok(Vec::new());
        }

        let mut scores: FxHashMap<DocId, f32> = FxHashMap::default();
        for (tid, seed_w) in row {
            let ve = self.vocab_entry(tid);
            let p_base = self.postings_blob_off + ve.postings_offset as usize;
            let p_count = ve.postings_count as usize;
            for j in 0..p_count {
                let p = p_base + j * POSTING_ENTRY_SIZE;
                let cand_doc = u32::from_le_bytes(self.mmap[p..p + 4].try_into().unwrap());
                if cand_doc == doc_id {
                    continue;
                }
                let cand_w = f32::from_le_bytes(self.mmap[p + 4..p + 8].try_into().unwrap());
                *scores.entry(cand_doc).or_insert(0.0) += seed_w * cand_w;
            }
        }

        let mut ranked: Vec<(DocId, f32)> = scores
            .into_iter()
            .filter(|(_, s)| *s > 0.0)
            .collect();
        ranked.sort_by_key(|&(_, s)| Reverse(OrderedFloat(s)));
        ranked.truncate(limit.max(1));
        Ok(ranked)
    }

    /// Shared n-grams between seed and candidate, with TF-IDF contributions.
    /// Requires the seed text so we can recover n-gram strings from hashes.
    pub fn shared_ngrams_with_seed_text(
        &self,
        seed_doc: DocId,
        cand_doc: DocId,
        seed_text: &str,
        limit: usize,
    ) -> Vec<SharedNgram> {
        let seed_row = self.row_entries(seed_doc);
        let cand_row = self.row_entries(cand_doc);
        if seed_row.is_empty() || cand_row.is_empty() {
            return Vec::new();
        }

        let mut hash_to_str: FxHashMap<u64, String> = FxHashMap::default();
        for gram in char_ngrams(seed_text, self.params.min_ngram, self.params.max_ngram) {
            let h = xxh3_64(gram.as_bytes());
            hash_to_str.entry(h).or_insert(gram);
        }

        let mut shared: Vec<SharedNgram> = Vec::new();
        let (mut i, mut j) = (0usize, 0usize);
        while i < seed_row.len() && j < cand_row.len() {
            match seed_row[i].0.cmp(&cand_row[j].0) {
                std::cmp::Ordering::Equal => {
                    let tid = seed_row[i].0;
                    let term_hash = self.vocab_entry(tid).term_hash;
                    if let Some(ngram) = hash_to_str.get(&term_hash) {
                        let sw = seed_row[i].1;
                        let cw = cand_row[j].1;
                        shared.push(SharedNgram {
                            ngram: ngram.clone(),
                            contribution: round_f32(sw * cw, 8),
                            seed_weight: round_f32(sw, 8),
                            candidate_weight: round_f32(cw, 8),
                        });
                    }
                    i += 1;
                    j += 1;
                }
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
            }
        }
        shared.sort_by_key(|item| {
            Reverse((OrderedFloat(item.contribution), item.ngram.chars().count()))
        });
        shared.truncate(limit);
        shared
    }

    pub fn long_gram_shared_count_with_seed_text(
        &self,
        seed_doc: DocId,
        cand_doc: DocId,
        seed_text: &str,
        min_gram_len: usize,
    ) -> usize {
        let seed_row = self.row_entries(seed_doc);
        let cand_row = self.row_entries(cand_doc);
        if seed_row.is_empty() || cand_row.is_empty() {
            return 0;
        }

        let lo = min_gram_len.max(self.params.min_ngram);
        let hi = self.params.max_ngram;
        if lo > hi {
            return 0;
        }
        let mut long_hashes: FxHashSet<u64> = FxHashSet::default();
        for n in lo..=hi {
            for h in char_ngram_hashes(seed_text, n, n) {
                long_hashes.insert(h);
            }
        }

        let mut count = 0usize;
        let (mut i, mut j) = (0usize, 0usize);
        while i < seed_row.len() && j < cand_row.len() {
            match seed_row[i].0.cmp(&cand_row[j].0) {
                std::cmp::Ordering::Equal => {
                    let tid = seed_row[i].0;
                    if long_hashes.contains(&self.vocab_entry(tid).term_hash) {
                        count += 1;
                    }
                    i += 1;
                    j += 1;
                }
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
            }
        }
        count
    }

    pub fn info_payload(&self) -> serde_json::Value {
        let postings_nnz = self.postings_blob_len / POSTING_ENTRY_SIZE;
        let row_nnz = self.row_blob_len / ROW_ENTRY_SIZE;
        let docs = self.doc_count;
        let cols = self.vocab_count;
        let density = if docs == 0 || cols == 0 {
            0.0
        } else {
            (row_nnz as f64) / ((docs * cols) as f64)
        };
        serde_json::json!({
            "schema": "sinorag-tfidf-v2",
            "version": 2,
            "documents": docs,
            "matrix_shape": [docs, cols],
            "matrix_nnz": row_nnz,
            "postings_nnz": postings_nnz,
            "density": round_f64(density, 8),
            "features": cols,
            "row_blob_bytes": self.row_blob_len,
            "postings_blob_bytes": self.postings_blob_len,
            "params": {
                "analyzer": self.params.analyzer,
                "ngram_range": [self.params.min_ngram, self.params.max_ngram],
                "min_df": self.params.min_df,
                "max_df": self.params.max_df_ratio,
                "max_features": self.params.max_features,
                "dtype": self.params.dtype,
            },
            "doc_table_fingerprint": self.doc_table_fingerprint,
        })
    }

    /// Header-only read for `info` on huge files. Avoids mmapping the whole
    /// file (cheap either way, but explicit and matches phrase-index `header_info`).
    pub fn header_info(path: &Path) -> Result<serde_json::Value> {
        let mut file = File::open(path)?;
        let mut hdr = [0u8; HEADER_SIZE];
        file.read_exact(&mut hdr)?;
        if &hdr[0..8] != MAGIC_TFIDF_V2 {
            anyhow::bail!("invalid TF-IDF magic");
        }
        let version = u16::from_le_bytes([hdr[8], hdr[9]]);
        if version != 2 {
            anyhow::bail!("unsupported TF-IDF version: {}", version);
        }
        let vocab_count  = u32::from_le_bytes(hdr[HDR_VOCAB_COUNT].try_into()?) as usize;
        let doc_count    = u32::from_le_bytes(hdr[HDR_DOC_COUNT].try_into()?) as usize;
        let min_ngram    = u16::from_le_bytes(hdr[HDR_MIN_NGRAM].try_into()?) as usize;
        let max_ngram    = u16::from_le_bytes(hdr[HDR_MAX_NGRAM].try_into()?) as usize;
        let min_df       = u32::from_le_bytes(hdr[HDR_MIN_DF].try_into()?);
        let max_features = u32::from_le_bytes(hdr[HDR_MAX_FEATURES].try_into()?) as usize;
        let max_df_ratio = f32::from_le_bytes(hdr[HDR_MAX_DF_RATIO].try_into()?);
        let fp_end = hdr[HDR_FP].iter().position(|&b| b == 0).unwrap_or(HDR_FP.len());
        let doc_table_fingerprint =
            String::from_utf8_lossy(&hdr[HDR_FP.start..HDR_FP.start + fp_end]).to_string();
        let row_blob_len  = u64::from_le_bytes(hdr[HDR_ROW_BLOB_LEN].try_into()?);
        let post_blob_len = u64::from_le_bytes(hdr[HDR_POST_BLOB_LEN].try_into()?);
        let file_bytes = std::fs::metadata(path)?.len();
        Ok(serde_json::json!({
            "schema": "sinorag-tfidf-v2",
            "version": version,
            "documents": doc_count,
            "features": vocab_count,
            "row_blob_bytes": row_blob_len,
            "postings_blob_bytes": post_blob_len,
            "file_bytes": file_bytes,
            "params": {
                "ngram_range": [min_ngram, max_ngram],
                "min_df": min_df,
                "max_df": max_df_ratio,
                "max_features": max_features,
            },
            "doc_table_fingerprint": doc_table_fingerprint,
        }))
    }
}

// ---------------------------------------------------------------------------
// Free helpers — operate on raw text, no index needed
// ---------------------------------------------------------------------------

pub fn long_common_substrings(a: &str, b: &str, min_len: usize, limit: usize) -> Vec<String> {
    let mut seen: FxHashSet<String> = FxHashSet::default();
    let mut matches = Vec::new();
    let (short, long) = if a.chars().count() > b.chars().count() {
        (b, a)
    } else {
        (a, b)
    };
    let chars: Vec<char> = short.chars().collect();
    let max_len = 24usize.min(chars.len());

    for length in (min_len..=max_len).rev() {
        if chars.len() < length {
            continue;
        }
        for index in 0..=(chars.len() - length) {
            let frag: String = chars[index..index + length].iter().collect();
            if seen.insert(frag.clone()) && long.contains(&frag) {
                matches.push(frag);
                if matches.len() >= limit {
                    return matches;
                }
            }
        }
    }
    matches
}

// ---------------------------------------------------------------------------
// External-sort builder helpers
// ---------------------------------------------------------------------------

/// Stable resume key for a parquet file: the last two path components
/// (partition + filename), e.g. `source_corpus=kanripo/part-000123.parquet`.
/// Invariant to absolute/relative invocation and to moves of the corpus root.
fn progress_key(p: &Path) -> String {
    let comps: Vec<String> = p
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    match comps.len() {
        0 => String::new(),
        1 => comps[0].clone(),
        n => format!("{}/{}", comps[n - 2], comps[n - 1]),
    }
}

struct BucketWriterCache {
    temp_dir: PathBuf,
    max_open: usize,
    writers: HashMap<usize, BufWriter<File>>,
    lru: std::collections::VecDeque<usize>,
}

impl BucketWriterCache {
    fn new(temp_dir: PathBuf, max_open: usize) -> Self {
        Self {
            temp_dir,
            max_open: max_open.max(1),
            writers: HashMap::new(),
            lru: std::collections::VecDeque::new(),
        }
    }

    fn bucket_path(&self, bucket: usize) -> PathBuf {
        self.temp_dir.join(format!("bucket-{:04}.bin", bucket))
    }

    fn touch(&mut self, bucket: usize) {
        if let Some(pos) = self.lru.iter().position(|&b| b == bucket) {
            self.lru.remove(pos);
        }
        self.lru.push_back(bucket);
    }

    fn evict_if_needed(&mut self) -> Result<()> {
        while self.writers.len() >= self.max_open {
            let Some(victim) = self.lru.pop_front() else { break };
            if let Some(mut w) = self.writers.remove(&victim) {
                w.flush()?;
            }
        }
        Ok(())
    }

    fn ensure(&mut self, bucket: usize) -> Result<()> {
        if !self.writers.contains_key(&bucket) {
            self.evict_if_needed()?;
            let path = self.bucket_path(bucket);
            let file = OpenOptions::new().create(true).append(true).open(path)?;
            self.writers.insert(bucket, BufWriter::new(file));
        }
        self.touch(bucket);
        Ok(())
    }

    fn write_df_record(&mut self, bucket: usize, term_hash: u64, doc_id: u32) -> Result<()> {
        self.ensure(bucket)?;
        let w = self.writers.get_mut(&bucket).unwrap();
        w.write_all(&term_hash.to_le_bytes())?;
        w.write_all(&doc_id.to_le_bytes())?;
        Ok(())
    }

    fn write_posting_record(
        &mut self,
        bucket: usize,
        term_id: TermId,
        doc_id: DocId,
        weight: f32,
    ) -> Result<()> {
        self.ensure(bucket)?;
        let w = self.writers.get_mut(&bucket).unwrap();
        w.write_all(&term_id.to_le_bytes())?;
        w.write_all(&doc_id.to_le_bytes())?;
        w.write_all(&weight.to_le_bytes())?;
        Ok(())
    }

    fn flush_all(mut self) -> Result<()> {
        for (_, mut w) in self.writers.drain() {
            w.flush()?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Build entry point
// ---------------------------------------------------------------------------

// Default temp dir is derived from the output index path
// (`<out>.work/`). Set inside `build()` when `temp_dir` is None.

pub fn build(
    parquet_path: PathBuf,
    doc_table_path: PathBuf,
    out_path: PathBuf,
    params: TfidfParams,
    bucket_count: usize,
    temp_dir: Option<PathBuf>,
) -> Result<()> {
    let temp_dir = temp_dir.unwrap_or_else(|| {
        let mut p = out_path.as_os_str().to_os_string();
        p.push(".work");
        PathBuf::from(p)
    });

    eprintln!("=== TF-IDF builder ===");
    eprintln!("Parquet : {}", parquet_path.display());
    eprintln!("DocTable: {}", doc_table_path.display());
    eprintln!("Output  : {}", out_path.display());
    eprintln!(
        "Params  : min_n={} max_n={} min_df={} max_df={} max_features={}",
        params.min_ngram, params.max_ngram, params.min_df, params.max_df_ratio, params.max_features
    );
    eprintln!("Buckets : {}", bucket_count);
    eprintln!("Temp dir: {}", temp_dir.display());

    // Phase marker paths — checked to determine whether to resume or start fresh.
    let phase1_done    = temp_dir.join("phase1.done");
    let phase1_prog    = temp_dir.join("phase1_progress.txt");
    let phase2_vocab   = temp_dir.join("phase2.vocab.bin");
    let phase2_idf_f   = temp_dir.join("phase2.idf.bin");
    let phase2_termid  = temp_dir.join("phase2.termid.bin");

    let resuming = phase1_done.exists()
        || phase1_prog.exists()
        || phase2_vocab.exists();

    if resuming {
        eprintln!("Resume mode: detected partial run in {}", temp_dir.display());
    } else if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }

    fs::create_dir_all(&temp_dir)?;
    let df_dir   = temp_dir.join("df_buckets");
    let post_dir = temp_dir.join("post_buckets");
    if !phase1_done.exists() {
        fs::create_dir_all(&df_dir)?;
    }
    fs::create_dir_all(&post_dir)?;

    // -----------------------------------------------------------------------
    // Phase 0 — load doc table
    // -----------------------------------------------------------------------
    eprintln!("\n[Phase 0] Loading doc table...");
    let doc_table = DocumentTable::load(&doc_table_path)?;
    let doc_table_fingerprint = doc_table.source_fingerprint.clone();
    let doc_count = doc_table.passage_ids.len();
    eprintln!("  {} passages", doc_count);

    let files = crate::phrase_index::parquet_files(&parquet_path)?;
    eprintln!("  {} parquet file(s)", files.len());

    // -----------------------------------------------------------------------
    // Phase 1 — DF bucket writes (parallel, resumable)
    // -----------------------------------------------------------------------
    if phase1_done.exists() {
        eprintln!("\n[Phase 1] Skipping (complete)");
    } else {
        eprintln!("\n[Phase 1] DF buckets...");

        // Use a stable key (last 2 path segments, e.g.
        // `source_corpus=kanripo/part-000123.parquet`) so resume survives
        // relative/absolute invocations and corpus-root moves.
        let mut completed: FxHashSet<String> = FxHashSet::default();
        if phase1_prog.exists() {
            for line in fs::read_to_string(&phase1_prog)?.lines() {
                if !line.is_empty() {
                    completed.insert(progress_key(Path::new(line)));
                }
            }
            eprintln!("  Resume: {}/{} files already processed", completed.len(), files.len());
        }
        let pending: Vec<&PathBuf> = files
            .iter()
            .filter(|f| !completed.contains(&progress_key(f)))
            .collect();

        let nthreads = rayon::current_num_threads();
        // Reserve ~300 handles for OS + parquet readers; distribute the rest
        // across threads.  Assumes the common default ulimit of 1024.
        // Running `ulimit -n 65536` before the binary removes this constraint.
        let handles_per_thread = (700usize / nthreads.max(1)).max(4).min(64);
        eprintln!(
            "  {} pending files on {} threads ({} handles/thread)",
            pending.len(), nthreads, handles_per_thread
        );
        for t in 0..nthreads {
            fs::create_dir_all(df_dir.join(format!("t{}", t)))?;
        }

        let counter  = AtomicUsize::new(completed.len());
        let total    = files.len();
        let prog_f   = Mutex::new(OpenOptions::new().create(true).append(true).open(&phase1_prog)?);
        let min_n    = params.min_ngram;
        let max_n    = params.max_ngram;

        pending.par_iter().try_for_each(|fp| -> Result<()> {
            let t = rayon::current_thread_index().unwrap_or(0);
            let mut w = BucketWriterCache::new(df_dir.join(format!("t{}", t)), handles_per_thread);

            let builder = crate::parquet_metadata::global_cache().get_or_load(fp)?;
            let reader  = builder.build()?;
            for batch in reader {
                let batch = batch?;
                let (pids, texts) = extract_columns(&batch)?;
                let mut hash_buf: Vec<u64> = Vec::new();
                for i in 0..batch.num_rows() {
                    if let Some(&doc_id) = doc_table.passage_id_map.get(pids.value(i)) {
                        char_ngram_hashes_into(texts.value(i), min_n, max_n, &mut hash_buf);
                        for &hash in &hash_buf {
                            w.write_df_record((hash as usize) % bucket_count, hash, doc_id)?;
                        }
                    }
                }
            }
            w.flush_all()?;

            let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 100 == 0 || n == total {
                eprintln!("  {}/{}", n, total);
            }
            writeln!(prog_f.lock().unwrap(), "{}", progress_key(fp))?;
            Ok(())
        })?;

        // Merge per-thread bucket files into the shared bucket files.
        eprintln!("  Merging {} threads × {} buckets...", nthreads, bucket_count);
        for bucket_idx in 0..bucket_count {
            let main = df_dir.join(format!("bucket-{:04}.bin", bucket_idx));
            let mut out = OpenOptions::new().create(true).append(true).open(&main)?;
            for t in 0..nthreads {
                let src = df_dir.join(format!("t{}/bucket-{:04}.bin", t, bucket_idx));
                if src.exists() {
                    let data = fs::read(&src)?;
                    if !data.is_empty() {
                        out.write_all(&data)?;
                    }
                    fs::remove_file(&src)?;
                }
            }
        }
        for t in 0..nthreads {
            let _ = fs::remove_dir(df_dir.join(format!("t{}", t)));
        }
        fs::write(&phase1_done, b"")?;
        eprintln!("  Phase 1 complete.");
    }

    // -----------------------------------------------------------------------
    // Phase 2 — build vocabulary (parallel, resumable)
    // -----------------------------------------------------------------------
    let (mut vocab_entries, idf, term_to_id) = if phase2_vocab.exists() {
        eprintln!("\n[Phase 2] Loading saved vocabulary...");
        let ve: Vec<VocabEntry>       = bincode::deserialize(&fs::read(&phase2_vocab)?)?;
        let iv: Vec<f32>              = bincode::deserialize(&fs::read(&phase2_idf_f)?)?;
        let ti: HashMap<u64, TermId>  = bincode::deserialize(&fs::read(&phase2_termid)?)?;
        eprintln!("  {} terms loaded", ve.len());
        (ve, iv, ti)
    } else {
        eprintln!("\n[Phase 2] Building vocabulary...");

        // Each bucket's hashes are disjoint (hash % bucket_count == bucket_idx),
        // so bucket DF maps can be merged with extend (no key collision).
        let bucket_dfs: Vec<Vec<(u64, u32)>> = (0..bucket_count)
            .into_par_iter()
            .map(|bucket_idx| -> Result<Vec<(u64, u32)>> {
                let path = df_dir.join(format!("bucket-{:04}.bin", bucket_idx));
                if !path.exists() {
                    return Ok(Vec::new());
                }
                let raw = fs::read(&path)?;
                let n = raw.len() / 12;
                let mut records: Vec<(u64, u32)> = Vec::with_capacity(n);
                for k in 0..n {
                    let base = k * 12;
                    let h  = u64::from_le_bytes(raw[base..base + 8].try_into()?);
                    let id = u32::from_le_bytes(raw[base + 8..base + 12].try_into()?);
                    records.push((h, id));
                }
                records.sort_unstable();
                records.dedup();
                // Count distinct doc_ids per hash = DF for that n-gram.
                let mut local: Vec<(u64, u32)> = Vec::new();
                let mut i = 0;
                while i < records.len() {
                    let h = records[i].0;
                    let mut j = i;
                    while j < records.len() && records[j].0 == h {
                        j += 1;
                    }
                    local.push((h, (j - i) as u32));
                    i = j;
                }
                Ok(local)
            })
            .collect::<Result<Vec<_>>>()?;

        let _ = fs::remove_dir_all(&df_dir);

        let total_terms: usize = bucket_dfs.iter().map(|v| v.len()).sum();
        let mut term_df: HashMap<u64, u32> = HashMap::with_capacity(total_terms);
        for local in bucket_dfs {
            term_df.extend(local);
        }

        let max_df_count = if params.max_df_ratio <= 0.0 {
            doc_count as u32
        } else {
            ((doc_count as f32) * params.max_df_ratio).floor().max(1.0) as u32
        };
        let mut vocab: Vec<(u64, u32)> = term_df
            .into_iter()
            .filter(|(_, df)| *df >= params.min_df && *df <= max_df_count)
            .collect();
        eprintln!("  {} terms pass filters", vocab.len());
        vocab.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        if params.max_features > 0 && vocab.len() > params.max_features {
            vocab.truncate(params.max_features);
        }
        vocab.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        eprintln!("  Final vocab: {}", vocab.len());

        let idf: Vec<f32> = vocab
            .iter()
            .map(|(_, df)| {
                let df = *df as f32;
                ((doc_count as f32 - df + 0.5) / (df + 0.5) + 1.0).ln()
            })
            .collect();

        let term_to_id: HashMap<u64, TermId> = vocab
            .iter()
            .enumerate()
            .map(|(i, (h, _))| (*h, i as TermId))
            .collect();

        let vocab_entries: Vec<VocabEntry> = vocab
            .iter()
            .enumerate()
            .map(|(i, (h, df))| VocabEntry {
                term_hash: *h,
                term_id: i as TermId,
                df: *df,
                postings_offset: 0,
                postings_count: 0,
            })
            .collect();

        // Save for resume — if Phase 3+ gets interrupted, we reload these.
        fs::write(&phase2_vocab,  bincode::serialize(&vocab_entries)?)?;
        fs::write(&phase2_idf_f,  bincode::serialize(&idf)?)?;
        fs::write(&phase2_termid, bincode::serialize(&term_to_id)?)?;

        (vocab_entries, idf, term_to_id)
    };

    // -----------------------------------------------------------------------
    // Phase 3 — rows + posting bucket writes (parallel)
    // -----------------------------------------------------------------------
    eprintln!("\n[Phase 3] Rows + posting buckets...");
    let nthreads = rayon::current_num_threads();
    let handles_per_thread = (700usize / nthreads.max(1)).max(4).min(64);
    for t in 0..nthreads {
        fs::create_dir_all(post_dir.join(format!("t{}", t)))?;
    }

    // A dedicated writer thread assembles the row_blob so parallel compute
    // threads never contend on it.  Channel is bounded to apply back-pressure.
    let (row_tx, row_rx) = std::sync::mpsc::sync_channel::<(DocId, Vec<u8>)>(8192);
    let writer_handle = {
        let mut row_offsets = vec![u64::MAX; doc_count];
        let mut row_lengths = vec![0u32; doc_count];
        let mut row_blob: Vec<u8> = Vec::new();
        std::thread::spawn(move || {
            for (doc_id, bytes) in row_rx {
                row_offsets[doc_id as usize] = row_blob.len() as u64;
                row_lengths[doc_id as usize] = (bytes.len() / ROW_ENTRY_SIZE) as u32;
                row_blob.extend_from_slice(&bytes);
            }
            (row_offsets, row_lengths, row_blob)
        })
    };

    let counter3 = AtomicUsize::new(0usize);
    let total3   = files.len();
    let min_n    = params.min_ngram;
    let max_n    = params.max_ngram;

    // try_for_each_with clones row_tx once per rayon worker; when it returns,
    // all clones are dropped and the channel closes, ending the writer loop.
    files.par_iter().try_for_each_with(row_tx, |tx, fp| -> Result<()> {
        let t = rayon::current_thread_index().unwrap_or(0);
        let mut post_w = BucketWriterCache::new(post_dir.join(format!("t{}", t)), handles_per_thread);

        let builder = crate::parquet_metadata::global_cache().get_or_load(fp)?;
        let reader  = builder.build()?;
        for batch in reader {
            let batch = batch?;
            let (pids, texts) = extract_columns(&batch)?;
            let mut hash_buf: Vec<u64> = Vec::new();
            for i in 0..batch.num_rows() {
                let pid  = pids.value(i);
                let text = texts.value(i);
                let Some(&doc_id) = doc_table.passage_id_map.get(pid) else { continue };

                let mut term_counts: FxHashMap<TermId, u32> = FxHashMap::default();
                char_ngram_hashes_all_into(text, min_n, max_n, &mut hash_buf);
                for &h in &hash_buf {
                    if let Some(&tid) = term_to_id.get(&h) {
                        *term_counts.entry(tid).or_insert(0) += 1;
                    }
                }
                if term_counts.is_empty() {
                    continue;
                }

                let total_tf: u32 = term_counts.values().sum();
                let mut row: Vec<(TermId, f32)> = term_counts
                    .iter()
                    .map(|(&tid, &c)| (tid, (c as f32 / total_tf as f32) * idf[tid as usize]))
                    .collect();

                let norm = row.iter().map(|(_, w)| w * w).sum::<f32>().sqrt();
                if norm > 0.0 {
                    for (_, w) in row.iter_mut() {
                        *w /= norm;
                    }
                }
                row.sort_unstable_by_key(|(tid, _)| *tid);

                let mut row_bytes = Vec::with_capacity(row.len() * ROW_ENTRY_SIZE);
                for (tid, w) in &row {
                    row_bytes.extend_from_slice(&tid.to_le_bytes());
                    row_bytes.extend_from_slice(&w.to_le_bytes());
                }
                tx.send((doc_id, row_bytes))
                    .map_err(|e| anyhow::anyhow!("row channel closed: {}", e))?;

                for (tid, w) in &row {
                    post_w.write_posting_record((*tid as usize) % bucket_count, *tid, doc_id, *w)?;
                }
            }
        }
        post_w.flush_all()?;

        let n = counter3.fetch_add(1, Ordering::Relaxed) + 1;
        if n % 100 == 0 || n == total3 {
            eprintln!("  {}/{}", n, total3);
        }
        Ok(())
    })?;

    let (row_offsets, row_lengths, row_blob) = writer_handle
        .join()
        .map_err(|_| anyhow::anyhow!("row writer thread panicked"))?;

    eprintln!(
        "  Row blob: {} bytes ({:.1} MB)",
        row_blob.len(),
        row_blob.len() as f64 / 1e6
    );

    // Merge per-thread posting bucket files.
    eprintln!("  Merging posting thread buckets...");
    for bucket_idx in 0..bucket_count {
        let main = post_dir.join(format!("bucket-{:04}.bin", bucket_idx));
        let mut out = OpenOptions::new().create(true).append(true).open(&main)?;
        for t in 0..nthreads {
            let src = post_dir.join(format!("t{}/bucket-{:04}.bin", t, bucket_idx));
            if src.exists() {
                let data = fs::read(&src)?;
                if !data.is_empty() {
                    out.write_all(&data)?;
                }
                fs::remove_file(&src)?;
            }
        }
    }
    for t in 0..nthreads {
        let _ = fs::remove_dir(post_dir.join(format!("t{}", t)));
    }

    // -----------------------------------------------------------------------
    // Phase 4 — merge posting buckets (parallel)
    // -----------------------------------------------------------------------
    eprintln!("\n[Phase 4] Merging posting buckets...");

    // Each bucket produces a sorted (term_id, count, postings_bytes) list.
    // Buckets contain disjoint term_ids, so the final assembly is a simple
    // sequential extend — no locking needed.
    let bucket_results: Vec<Vec<(TermId, u32, Vec<u8>)>> = (0..bucket_count)
        .into_par_iter()
        .map(|bucket_idx| -> Result<Vec<(TermId, u32, Vec<u8>)>> {
            if bucket_idx % 256 == 0 {
                eprintln!("  bucket {}/{}", bucket_idx, bucket_count);
            }
            let path = post_dir.join(format!("bucket-{:04}.bin", bucket_idx));
            if !path.exists() {
                return Ok(Vec::new());
            }
            let raw = fs::read(&path)?;
            let n = raw.len() / POSTING_BUCKET_RECORD_SIZE;
            let mut records: Vec<(TermId, DocId, f32)> = Vec::with_capacity(n);
            for k in 0..n {
                let base = k * POSTING_BUCKET_RECORD_SIZE;
                let tid = u32::from_le_bytes(raw[base..base + 4].try_into()?);
                let did = u32::from_le_bytes(raw[base + 4..base + 8].try_into()?);
                let w   = f32::from_le_bytes(raw[base + 8..base + 12].try_into()?);
                records.push((tid, did, w));
            }
            records.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

            let mut out: Vec<(TermId, u32, Vec<u8>)> = Vec::new();
            let mut i = 0;
            while i < records.len() {
                let tid = records[i].0;
                let mut j = i;
                while j < records.len() && records[j].0 == tid {
                    j += 1;
                }
                let count = (j - i) as u32;
                let mut bytes = Vec::with_capacity((j - i) * POSTING_ENTRY_SIZE);
                for k in i..j {
                    let (_, did, w) = records[k];
                    bytes.extend_from_slice(&did.to_le_bytes());
                    bytes.extend_from_slice(&w.to_le_bytes());
                }
                out.push((tid, count, bytes));
                i = j;
            }
            Ok(out)
        })
        .collect::<Result<Vec<_>>>()?;

    let mut postings_blob: Vec<u8> = Vec::new();
    for bucket_data in bucket_results {
        for (tid, count, bytes) in bucket_data {
            vocab_entries[tid as usize].postings_offset = postings_blob.len() as u64;
            vocab_entries[tid as usize].postings_count  = count;
            postings_blob.extend_from_slice(&bytes);
        }
    }
    let _ = fs::remove_dir_all(&post_dir);
    eprintln!(
        "  Postings blob: {} bytes ({:.1} MB)",
        postings_blob.len(),
        postings_blob.len() as f64 / 1e6
    );

    // -----------------------------------------------------------------------
    // Save — write directly from build's local Vecs (no in-memory `TfidfIndex`).
    // -----------------------------------------------------------------------
    eprintln!("\n[Save] Writing index...");
    write_index_file(
        &out_path,
        &params,
        &doc_table_fingerprint,
        &vocab_entries,
        &idf,
        doc_count,
        &row_offsets,
        &row_lengths,
        &row_blob,
        &postings_blob,
    )?;

    let _ = fs::remove_dir_all(&temp_dir);
    eprintln!("\n=== Complete ===");
    eprintln!("Output: {}", out_path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Free function: write the on-disk index from the builder's in-memory state
// ---------------------------------------------------------------------------

fn write_index_file(
    path: &Path,
    params: &TfidfParams,
    doc_table_fingerprint: &str,
    vocab: &[VocabEntry],
    idf: &[f32],
    doc_count: usize,
    row_offsets: &[u64],
    row_lengths: &[u32],
    row_blob: &[u8],
    postings_blob: &[u8],
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("index.tmp");
    let mut f = BufWriter::new(File::create(&tmp)?);

    let vocab_count = vocab.len() as u32;
    let doc_count_u32 = doc_count as u32;
    let row_blob_len = row_blob.len() as u64;
    let postings_blob_len = postings_blob.len() as u64;

    let vocab_off     = HEADER_SIZE as u64;
    let idf_off       = vocab_off + vocab_count as u64 * VOCAB_ENTRY_SIZE as u64;
    let row_off_off   = idf_off + vocab_count as u64 * 4;
    let row_len_off   = row_off_off + doc_count as u64 * 8;
    let row_blob_off  = row_len_off + doc_count as u64 * 4;
    let post_blob_off = row_blob_off + row_blob_len;

    let mut hdr = vec![0u8; HEADER_SIZE];
    hdr[0..8].copy_from_slice(MAGIC_TFIDF_V2);
    hdr[8..10].copy_from_slice(&2u16.to_le_bytes());
    hdr[HDR_VOCAB_COUNT].copy_from_slice(&vocab_count.to_le_bytes());
    hdr[HDR_DOC_COUNT].copy_from_slice(&doc_count_u32.to_le_bytes());
    hdr[HDR_MIN_NGRAM].copy_from_slice(&(params.min_ngram as u16).to_le_bytes());
    hdr[HDR_MAX_NGRAM].copy_from_slice(&(params.max_ngram as u16).to_le_bytes());
    hdr[HDR_MIN_DF].copy_from_slice(&params.min_df.to_le_bytes());
    hdr[HDR_MAX_FEATURES].copy_from_slice(&(params.max_features as u32).to_le_bytes());
    hdr[HDR_MAX_DF_RATIO].copy_from_slice(&params.max_df_ratio.to_le_bytes());

    let fp = doc_table_fingerprint.as_bytes();
    let n = fp.len().min(HDR_FP.len() - 1);
    hdr[HDR_FP.start..HDR_FP.start + n].copy_from_slice(&fp[..n]);

    hdr[HDR_VOCAB_OFF].copy_from_slice(&vocab_off.to_le_bytes());
    hdr[HDR_IDF_OFF].copy_from_slice(&idf_off.to_le_bytes());
    hdr[HDR_ROW_OFFSETS].copy_from_slice(&row_off_off.to_le_bytes());
    hdr[HDR_ROW_LENGTHS].copy_from_slice(&row_len_off.to_le_bytes());
    hdr[HDR_ROW_BLOB_OFF].copy_from_slice(&row_blob_off.to_le_bytes());
    hdr[HDR_ROW_BLOB_LEN].copy_from_slice(&row_blob_len.to_le_bytes());
    hdr[HDR_POST_BLOB_OFF].copy_from_slice(&post_blob_off.to_le_bytes());
    hdr[HDR_POST_BLOB_LEN].copy_from_slice(&postings_blob_len.to_le_bytes());
    f.write_all(&hdr)?;

    for e in vocab {
        f.write_all(&e.term_hash.to_le_bytes())?;
        f.write_all(&e.term_id.to_le_bytes())?;
        f.write_all(&e.df.to_le_bytes())?;
        f.write_all(&e.postings_offset.to_le_bytes())?;
        f.write_all(&e.postings_count.to_le_bytes())?;
    }
    for &v in idf {
        f.write_all(&v.to_le_bytes())?;
    }
    for &o in row_offsets {
        f.write_all(&o.to_le_bytes())?;
    }
    for &l in row_lengths {
        f.write_all(&l.to_le_bytes())?;
    }
    f.write_all(row_blob)?;
    f.write_all(postings_blob)?;
    f.flush()?;
    drop(f);
    fs::rename(&tmp, path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn extract_columns(
    batch: &arrow::record_batch::RecordBatch,
) -> Result<(&arrow::array::StringArray, &arrow::array::StringArray)> {
    let passage_col = batch
        .schema()
        .column_with_name("passage_id")
        .ok_or_else(|| anyhow::anyhow!("missing passage_id column"))?
        .0;
    let text_col = batch
        .schema()
        .column_with_name("zh_text_normalized")
        .ok_or_else(|| anyhow::anyhow!("missing zh_text_normalized column"))?
        .0;
    let pids = batch
        .column(passage_col)
        .as_any()
        .downcast_ref::<arrow::array::StringArray>()
        .ok_or_else(|| anyhow::anyhow!("passage_id is not StringArray"))?;
    let texts = batch
        .column(text_col)
        .as_any()
        .downcast_ref::<arrow::array::StringArray>()
        .ok_or_else(|| anyhow::anyhow!("zh_text_normalized is not StringArray"))?;
    Ok((pids, texts))
}

fn round_f32(value: f32, places: i32) -> f32 {
    let factor = 10f32.powi(places);
    (value * factor).round() / factor
}

fn round_f64(value: f64, places: i32) -> f64 {
    let factor = 10f64.powi(places);
    (value * factor).round() / factor
}
