// Streaming, memory-bounded CJK phrase index for the SinoRAGD/ReadZen toolset.
//
// Pipeline (single canonical version):
//   Phase 1 — scan parquet, normalise text, write (gram_hash, doc_id) records
//             into bucket files (bucketed by hash % bucket_count).
//   Phase 2 — sort each bucket in fixed-size in-memory chunks, write sorted runs.
//   Phase 3 — k-way merge each bucket's sorted runs, dedup (gram, doc_id),
//             delta-varint encode posting lists, stream to a single postings file.
//   Phase 4 — write the final index file: header + gram entries + postings blob.
//
// On-disk format ("SGV2", version 2):
//   header (256 B)
//     [0..4]    magic = "SGV2"
//     [4..6]    version u16 = 2
//     [6..8]    gram_len u16
//     [8..12]   num_grams u32
//     [12..76]  doc_table_fingerprint (null-padded utf8, max 64 B)
//     [76..140] schema string (null-padded utf8, max 64 B)
//     [140..148] gram_table_offset u64 (= 256)
//     [148..152] gram_table_size u32 (in bytes)
//     [152..160] postings_blob_size u64
//     [160..256] reserved
//   gram entries (20 B each, sorted by gram_hash for binary search)
//     gram_hash u64 | offset u64 | len u32
//   postings blob — concatenated delta-varint encoded sorted DocId lists

use crate::datafusion_store::{sql_literal, DataFusionStore};
use crate::document_table::DocumentTable;
use crate::normalize::normalize_zh;
use crate::parquet_metadata::global_cache;
use anyhow::Result;
use arrow::array::{Array, StringArray};
use memmap2::Mmap;
use rustc_hash::FxHashSet;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use xxhash_rust::xxh3::xxh3_64;

pub type DocId = u32;

const MAGIC: &[u8; 4] = b"SGV2";
const HEADER_SIZE: usize = 256;
const GRAM_ENTRY_SIZE: usize = 20;
const SCHEMA_LABEL: &[u8] = b"sinorag-phrase-index-v2";

const HDR_GRAM_LEN: std::ops::Range<usize>      = 6..8;
const HDR_NUM_GRAMS: std::ops::Range<usize>     = 8..12;
const HDR_FP: std::ops::Range<usize>            = 12..76;
const HDR_SCHEMA: std::ops::Range<usize>        = 76..140;
const HDR_GRAM_TABLE_OFF: std::ops::Range<usize>  = 140..148;
const HDR_GRAM_TABLE_SIZE: std::ops::Range<usize> = 148..152;
const HDR_POSTINGS_SIZE: std::ops::Range<usize>   = 152..160;

#[derive(Debug, Clone)]
pub struct PhraseIndex {
    pub schema: String,
    pub gram_len: usize,
    pub doc_table_fingerprint: String,
    pub gram_entries: Vec<GramEntry>,
    pub postings_blob: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub struct GramEntry {
    pub gram_hash: u64,
    pub offset: u64,
    pub len: u32,
}

impl PhraseIndex {
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("index.tmp");
        let mut f = BufWriter::new(File::create(&tmp)?);

        let mut hdr = vec![0u8; HEADER_SIZE];
        hdr[0..4].copy_from_slice(MAGIC);
        hdr[4..6].copy_from_slice(&2u16.to_le_bytes());
        hdr[HDR_GRAM_LEN].copy_from_slice(&(self.gram_len as u16).to_le_bytes());
        hdr[HDR_NUM_GRAMS].copy_from_slice(&(self.gram_entries.len() as u32).to_le_bytes());

        write_padded_string(&mut hdr[HDR_FP], &self.doc_table_fingerprint);
        write_padded_string(&mut hdr[HDR_SCHEMA], &self.schema);

        let gram_table_off = HEADER_SIZE as u64;
        let gram_table_sz = (self.gram_entries.len() * GRAM_ENTRY_SIZE) as u32;
        hdr[HDR_GRAM_TABLE_OFF].copy_from_slice(&gram_table_off.to_le_bytes());
        hdr[HDR_GRAM_TABLE_SIZE].copy_from_slice(&gram_table_sz.to_le_bytes());
        hdr[HDR_POSTINGS_SIZE].copy_from_slice(&(self.postings_blob.len() as u64).to_le_bytes());

        f.write_all(&hdr)?;
        for e in &self.gram_entries {
            f.write_all(&e.gram_hash.to_le_bytes())?;
            f.write_all(&e.offset.to_le_bytes())?;
            f.write_all(&e.len.to_le_bytes())?;
        }
        f.write_all(&self.postings_blob)?;
        f.flush()?;
        drop(f);
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let data = fs::read(path)?;
        Self::from_bytes(&data)
    }

    pub fn load_mmap(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        Self::from_bytes(&mmap)
    }

    fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < HEADER_SIZE {
            anyhow::bail!("PhraseIndex file too small");
        }
        if &data[0..4] != MAGIC {
            anyhow::bail!("Not a PhraseIndex file (bad magic)");
        }
        let version = u16::from_le_bytes([data[4], data[5]]);
        if version != 2 {
            anyhow::bail!("Unsupported PhraseIndex version: {}", version);
        }

        let gram_len = u16::from_le_bytes(data[HDR_GRAM_LEN].try_into()?) as usize;
        let num_grams = u32::from_le_bytes(data[HDR_NUM_GRAMS].try_into()?) as usize;
        let doc_table_fingerprint = read_padded_string(&data[HDR_FP]);
        let schema = read_padded_string(&data[HDR_SCHEMA]);
        let gram_table_off = u64::from_le_bytes(data[HDR_GRAM_TABLE_OFF].try_into()?) as usize;
        let gram_table_sz  = u32::from_le_bytes(data[HDR_GRAM_TABLE_SIZE].try_into()?) as usize;
        let postings_sz    = u64::from_le_bytes(data[HDR_POSTINGS_SIZE].try_into()?) as usize;

        if gram_table_off + gram_table_sz + postings_sz > data.len() {
            anyhow::bail!("PhraseIndex sections exceed file length");
        }

        let mut gram_entries = Vec::with_capacity(num_grams);
        for i in 0..num_grams {
            let off = gram_table_off + i * GRAM_ENTRY_SIZE;
            let e = &data[off..off + GRAM_ENTRY_SIZE];
            gram_entries.push(GramEntry {
                gram_hash: u64::from_le_bytes(e[0..8].try_into()?),
                offset:    u64::from_le_bytes(e[8..16].try_into()?),
                len:       u32::from_le_bytes(e[16..20].try_into()?),
            });
        }

        let postings_off = gram_table_off + gram_table_sz;
        let postings_blob = data[postings_off..postings_off + postings_sz].to_vec();

        Ok(Self {
            schema,
            gram_len,
            doc_table_fingerprint,
            gram_entries,
            postings_blob,
        })
    }

    /// Resolve a phrase to candidate doc ids (intersection of all n-gram posting lists).
    pub fn candidate_ids(&self, phrase: &str, limit: usize) -> Vec<DocId> {
        let normalized = normalize_zh(phrase);
        self.candidate_ids_for_normalized(&normalized, limit)
    }

    pub fn candidate_ids_for_normalized(&self, normalized: &str, limit: usize) -> Vec<DocId> {
        let grams = phrase_index_grams(normalized, self.gram_len);
        if grams.is_empty() {
            return Vec::new();
        }

        let mut posting_lists: Vec<Vec<DocId>> = Vec::with_capacity(grams.len());
        for gram in grams {
            let hash = xxh3_64(gram.as_bytes());
            let Ok(idx) = self.gram_entries.binary_search_by_key(&hash, |e| e.gram_hash) else {
                return Vec::new();
            };
            let entry = &self.gram_entries[idx];
            let slice = &self.postings_blob[entry.offset as usize..][..entry.len as usize];
            let doc_ids = decode_delta_varint_docids(slice).unwrap_or_default();
            if doc_ids.is_empty() {
                return Vec::new();
            }
            posting_lists.push(doc_ids);
        }

        posting_lists.sort_by_key(|p| p.len());
        let mut current = posting_lists.swap_remove(0);
        for posting in &posting_lists {
            current = intersect_sorted(&current, posting);
            if current.is_empty() {
                break;
            }
        }
        current.truncate(limit.max(1));
        current
    }

    pub fn info_payload(&self) -> serde_json::Value {
        let posting_bytes: usize = self.postings_blob.len();
        serde_json::json!({
            "schema": self.schema,
            "gram_len": self.gram_len,
            "doc_table_fingerprint": self.doc_table_fingerprint,
            "num_grams": self.gram_entries.len(),
            "postings_bytes": posting_bytes,
            "version": 2,
        })
    }
}

// ---------------------------------------------------------------------------
// Tokenisation — applied identically at build and query time
// ---------------------------------------------------------------------------

/// Tokenise text into n-grams of length `gram_len`. Whitespace is stripped;
/// remaining characters are kept (CJK ideographs, residual letters, digits)
/// since `normalize_zh` removes all punctuation/symbols ahead of this step.
fn phrase_index_grams(text: &str, gram_len: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    if chars.len() < gram_len {
        return Vec::new();
    }
    chars
        .windows(gram_len)
        .map(|w| w.iter().collect())
        .collect()
}

fn intersect_sorted(a: &[DocId], b: &[DocId]) -> Vec<DocId> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Varint codec — zig-zag delta encoding of sorted DocId lists
// ---------------------------------------------------------------------------

fn encode_sorted_docids_delta_varint(doc_ids: &[DocId], output: &mut Vec<u8>) {
    if doc_ids.is_empty() {
        return;
    }
    let mut prev = doc_ids[0] as i64;
    encode_varint(prev, output);
    for &doc_id in doc_ids.iter().skip(1) {
        let delta = doc_id as i64 - prev;
        encode_varint(delta, output);
        prev = doc_id as i64;
    }
}

fn encode_varint(mut value: i64, output: &mut Vec<u8>) {
    let mut zz: u64 = if value < 0 {
        ((!value as u64) << 1) | 1
    } else {
        (value as u64) << 1
    };
    // (We re-derive `zz` then write it as a uvarint.)
    let _ = &mut value;
    while zz >= 0x80 {
        output.push((zz as u8 & 0x7F) | 0x80);
        zz >>= 7;
    }
    output.push(zz as u8);
}

fn decode_delta_varint_docids(data: &[u8]) -> Result<Vec<DocId>> {
    let mut doc_ids = Vec::new();
    let mut pos = 0;
    let mut prev: i64 = 0;
    while pos < data.len() {
        let (zz, consumed) = decode_varint(&data[pos..])?;
        pos += consumed;
        let value = if zz & 1 == 0 {
            (zz >> 1) as i64
        } else {
            -((zz >> 1) as i64) - 1
        };
        prev += value;
        doc_ids.push(prev as DocId);
    }
    Ok(doc_ids)
}

fn decode_varint(data: &[u8]) -> Result<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    let mut pos = 0usize;
    for byte in data.iter() {
        pos += 1;
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok((result, pos));
        }
        shift += 7;
        if shift >= 64 {
            anyhow::bail!("varint overflow");
        }
    }
    anyhow::bail!("truncated varint")
}

// ---------------------------------------------------------------------------
// Builder — Phase 1 (bucket writes) → 2 (sort runs) → 3 (k-way merge) → 4 (write)
// ---------------------------------------------------------------------------

struct BucketWriterCache {
    temp_dir: PathBuf,
    max_open: usize,
    writers: HashMap<usize, BufWriter<File>>,
    lru: VecDeque<usize>,
}

impl BucketWriterCache {
    fn new(temp_dir: PathBuf, max_open: usize) -> Self {
        Self {
            temp_dir,
            max_open: max_open.max(1),
            writers: HashMap::new(),
            lru: VecDeque::new(),
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

    fn write_record(&mut self, bucket: usize, gram_hash: u64, doc_id: u32) -> Result<()> {
        if !self.writers.contains_key(&bucket) {
            self.evict_if_needed()?;
            let path = self.bucket_path(bucket);
            let file = OpenOptions::new().create(true).append(true).open(path)?;
            self.writers.insert(bucket, BufWriter::new(file));
        }
        self.touch(bucket);
        let w = self.writers.get_mut(&bucket).unwrap();
        w.write_all(&gram_hash.to_le_bytes())?;
        w.write_all(&doc_id.to_le_bytes())?;
        Ok(())
    }

    fn flush_all(mut self) -> Result<()> {
        for (_, mut w) in self.writers.drain() {
            w.flush()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct BucketRecord {
    gram_hash: u64,
    doc_id: DocId,
}

struct SortedChunkReader {
    file: File,
    buf: [u8; 12],
    exhausted: bool,
}

impl SortedChunkReader {
    fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            file: File::open(path)?,
            buf: [0u8; 12],
            exhausted: false,
        })
    }

    fn next(&mut self) -> Result<Option<BucketRecord>> {
        if self.exhausted {
            return Ok(None);
        }
        match self.file.read_exact(&mut self.buf) {
            Ok(()) => {
                let gram_hash = u64::from_le_bytes(self.buf[0..8].try_into().unwrap());
                let doc_id = u32::from_le_bytes(self.buf[8..12].try_into().unwrap());
                Ok(Some(BucketRecord { gram_hash, doc_id }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                self.exhausted = true;
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
struct HeapItem {
    record: BucketRecord,
    reader_idx: usize,
}

impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Min-heap on (gram_hash, doc_id)
        other
            .record
            .gram_hash
            .cmp(&self.record.gram_hash)
            .then_with(|| other.record.doc_id.cmp(&self.record.doc_id))
    }
}

impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

const SORT_CHUNK_RECORDS: usize = 1_000_000;
const DEFAULT_TEMP_DIR: &str =
    "/mnt/Samsung980_1TB/Rust-projects/not-rust-projects/ReadZen/GraphDiscovery/tmp_phrase_v2";

pub fn build(
    parquet_path: PathBuf,
    doc_table_path: PathBuf,
    out_path: PathBuf,
    gram_len: usize,
    bucket_count: usize,
    temp_dir: Option<PathBuf>,
) -> Result<()> {
    let temp_dir = temp_dir.unwrap_or_else(|| PathBuf::from(DEFAULT_TEMP_DIR));

    eprintln!("=== PhraseIndex builder ===");
    eprintln!("Parquet : {}", parquet_path.display());
    eprintln!("DocTable: {}", doc_table_path.display());
    eprintln!("Output  : {}", out_path.display());
    eprintln!("Gram len: {}", gram_len);
    eprintln!("Buckets : {}", bucket_count);
    eprintln!("Temp dir: {}", temp_dir.display());

    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }
    fs::create_dir_all(&temp_dir)?;

    let doc_table = DocumentTable::load(&doc_table_path)?;
    let doc_table_fingerprint = doc_table.source_fingerprint.clone();
    eprintln!("Loaded {} passages from doc_table", doc_table.passage_ids.len());

    let files = parquet_files(&parquet_path)?;
    eprintln!("Found {} parquet file(s)", files.len());

    // -----------------------------------------------------------------------
    // Phase 1 — write (gram_hash, doc_id) records to per-bucket files.
    // -----------------------------------------------------------------------
    eprintln!("\n[Phase 1] Bucket writes...");
    {
        const MAX_OPEN: usize = 64;
        let mut writers = BucketWriterCache::new(temp_dir.clone(), MAX_OPEN);
        let mut total_records: u64 = 0;
        let mut processed = 0usize;

        for file_path in &files {
            processed += 1;
            if processed % 100 == 0 {
                eprintln!("  {}/{}  ({} records)", processed, files.len(), total_records);
            }
            let builder = global_cache().get_or_load(file_path)?;
            let reader = builder.build()?;
            for batch in reader {
                let batch = batch?;
                let (passage_ids, text_arr) = extract_columns(&batch)?;
                for i in 0..batch.num_rows() {
                    let pid = passage_ids.value(i);
                    let text = text_arr.value(i);
                    let Some(&doc_id) = doc_table.passage_id_map.get(pid) else { continue };

                    let normalized = normalize_zh(text);
                    let chars: Vec<char> = normalized.chars().filter(|c| !c.is_whitespace()).collect();
                    if chars.len() < gram_len {
                        continue;
                    }
                    let mut seen: FxHashSet<u64> = FxHashSet::default();
                    for window in chars.windows(gram_len) {
                        let gram: String = window.iter().collect();
                        let hash = xxh3_64(gram.as_bytes());
                        if seen.insert(hash) {
                            let bucket = (hash as usize) % bucket_count;
                            writers.write_record(bucket, hash, doc_id)?;
                            total_records += 1;
                        }
                    }
                }
            }
        }
        writers.flush_all()?;
        eprintln!("  Total records: {}", total_records);
    }

    // -----------------------------------------------------------------------
    // Phase 2 — sort each bucket in 1M-record chunks.
    // -----------------------------------------------------------------------
    eprintln!("\n[Phase 2] Sorting chunks per bucket...");
    let mut all_sorted_chunks: Vec<Vec<PathBuf>> = Vec::with_capacity(bucket_count);
    for bucket_idx in 0..bucket_count {
        let bucket_path = temp_dir.join(format!("bucket-{:04}.bin", bucket_idx));
        if !bucket_path.exists() {
            all_sorted_chunks.push(Vec::new());
            continue;
        }
        if bucket_idx % 256 == 0 {
            eprintln!("  bucket {}/{}", bucket_idx, bucket_count);
        }

        let file_size = fs::metadata(&bucket_path)?.len() as usize;
        let record_count = file_size / 12;
        let mut sorted_chunks = Vec::new();
        let mut bucket_file = File::open(&bucket_path)?;
        let mut chunk_idx = 0usize;
        let mut remaining = record_count;

        while remaining > 0 {
            let to_read = SORT_CHUNK_RECORDS.min(remaining);
            let mut buf = vec![0u8; to_read * 12];
            bucket_file.read_exact(&mut buf)?;

            let mut records: Vec<BucketRecord> = Vec::with_capacity(to_read);
            for chunk in buf.chunks_exact(12) {
                let gram_hash = u64::from_le_bytes(chunk[0..8].try_into().unwrap());
                let doc_id = u32::from_le_bytes(chunk[8..12].try_into().unwrap());
                records.push(BucketRecord { gram_hash, doc_id });
            }
            records.sort_unstable_by_key(|r| (r.gram_hash, r.doc_id));

            let sorted_path =
                temp_dir.join(format!("sorted-{:04}-{:04}.bin", bucket_idx, chunk_idx));
            let mut sorted_file = BufWriter::new(File::create(&sorted_path)?);
            for r in &records {
                sorted_file.write_all(&r.gram_hash.to_le_bytes())?;
                sorted_file.write_all(&r.doc_id.to_le_bytes())?;
            }
            sorted_file.flush()?;
            drop(sorted_file);

            sorted_chunks.push(sorted_path);
            chunk_idx += 1;
            remaining -= to_read;
        }
        drop(bucket_file);
        fs::remove_file(&bucket_path)?;
        all_sorted_chunks.push(sorted_chunks);
    }

    // -----------------------------------------------------------------------
    // Phase 3 — k-way merge each bucket's sorted runs; encode posting lists.
    // -----------------------------------------------------------------------
    eprintln!("\n[Phase 3] K-way merge...");
    let entries_path = temp_dir.join("gram_entries.tmp");
    let postings_path = temp_dir.join("postings.tmp");
    let mut entries_writer = BufWriter::new(File::create(&entries_path)?);
    let mut postings_writer = BufWriter::new(File::create(&postings_path)?);
    let mut postings_offset: u64 = 0;
    let mut gram_count: u64 = 0;

    for (bucket_idx, sorted_chunks) in all_sorted_chunks.iter().enumerate() {
        if sorted_chunks.is_empty() {
            continue;
        }
        if bucket_idx % 256 == 0 {
            eprintln!("  bucket {}/{}", bucket_idx, bucket_count);
        }

        let mut readers: Vec<SortedChunkReader> = sorted_chunks
            .iter()
            .filter_map(|p| SortedChunkReader::open(p).ok())
            .collect();
        if readers.is_empty() {
            continue;
        }

        let mut heap = BinaryHeap::new();
        for (idx, reader) in readers.iter_mut().enumerate() {
            if let Some(record) = reader.next()? {
                heap.push(HeapItem { record, reader_idx: idx });
            }
        }

        let mut current_hash: Option<u64> = None;
        let mut current_docs: Vec<DocId> = Vec::new();
        let mut last_doc: Option<DocId> = None;

        while let Some(item) = heap.pop() {
            let r = item.record;
            if current_hash != Some(r.gram_hash) {
                emit_gram(
                    current_hash,
                    &mut current_docs,
                    &mut entries_writer,
                    &mut postings_writer,
                    &mut postings_offset,
                    &mut gram_count,
                )?;
                current_hash = Some(r.gram_hash);
                current_docs.clear();
                last_doc = None;
            }
            if last_doc != Some(r.doc_id) {
                current_docs.push(r.doc_id);
                last_doc = Some(r.doc_id);
            }
            if let Some(next) = readers[item.reader_idx].next()? {
                heap.push(HeapItem { record: next, reader_idx: item.reader_idx });
            }
        }
        emit_gram(
            current_hash,
            &mut current_docs,
            &mut entries_writer,
            &mut postings_writer,
            &mut postings_offset,
            &mut gram_count,
        )?;

        for p in sorted_chunks {
            let _ = fs::remove_file(p);
        }
    }
    entries_writer.flush()?;
    postings_writer.flush()?;
    drop(entries_writer);
    drop(postings_writer);
    eprintln!("  Total grams: {}", gram_count);

    // -----------------------------------------------------------------------
    // Phase 4 — write final index file (header + entries + postings).
    // -----------------------------------------------------------------------
    eprintln!("\n[Phase 4] Writing final index...");
    write_final_index(
        &out_path,
        &entries_path,
        &postings_path,
        gram_len,
        &doc_table_fingerprint,
        gram_count,
    )?;

    let _ = fs::remove_file(&entries_path);
    let _ = fs::remove_file(&postings_path);
    let _ = fs::remove_dir(&temp_dir);

    eprintln!("\n=== Complete ===");
    eprintln!("Output: {}", out_path.display());
    Ok(())
}

fn emit_gram(
    current_hash: Option<u64>,
    current_docs: &mut Vec<DocId>,
    entries_writer: &mut BufWriter<File>,
    postings_writer: &mut BufWriter<File>,
    postings_offset: &mut u64,
    gram_count: &mut u64,
) -> Result<()> {
    let Some(hash) = current_hash else { return Ok(()) };
    if current_docs.is_empty() {
        return Ok(());
    }
    current_docs.sort_unstable();
    current_docs.dedup();

    let mut encoded = Vec::new();
    encode_sorted_docids_delta_varint(current_docs, &mut encoded);

    let before = *postings_offset;
    postings_writer.write_all(&encoded)?;
    *postings_offset += encoded.len() as u64;

    entries_writer.write_all(&hash.to_le_bytes())?;
    entries_writer.write_all(&before.to_le_bytes())?;
    entries_writer.write_all(&(encoded.len() as u32).to_le_bytes())?;
    *gram_count += 1;
    Ok(())
}

fn write_final_index(
    out_path: &Path,
    entries_path: &Path,
    postings_path: &Path,
    gram_len: usize,
    doc_table_fingerprint: &str,
    gram_count: u64,
) -> Result<()> {
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let entries_size = fs::metadata(entries_path)?.len();
    let postings_size = fs::metadata(postings_path)?.len();
    let tmp = out_path.with_extension("index.tmp");
    let mut out = BufWriter::new(File::create(&tmp)?);

    let mut hdr = vec![0u8; HEADER_SIZE];
    hdr[0..4].copy_from_slice(MAGIC);
    hdr[4..6].copy_from_slice(&2u16.to_le_bytes());
    hdr[HDR_GRAM_LEN].copy_from_slice(&(gram_len as u16).to_le_bytes());
    hdr[HDR_NUM_GRAMS].copy_from_slice(&(gram_count as u32).to_le_bytes());
    write_padded_string(&mut hdr[HDR_FP], doc_table_fingerprint);
    write_padded_string(
        &mut hdr[HDR_SCHEMA],
        std::str::from_utf8(SCHEMA_LABEL).unwrap_or("sinorag-phrase-index-v2"),
    );
    hdr[HDR_GRAM_TABLE_OFF].copy_from_slice(&(HEADER_SIZE as u64).to_le_bytes());
    hdr[HDR_GRAM_TABLE_SIZE].copy_from_slice(&(entries_size as u32).to_le_bytes());
    hdr[HDR_POSTINGS_SIZE].copy_from_slice(&postings_size.to_le_bytes());

    out.write_all(&hdr)?;
    let mut entries = File::open(entries_path)?;
    std::io::copy(&mut entries, &mut out)?;
    let mut postings = File::open(postings_path)?;
    std::io::copy(&mut postings, &mut out)?;
    out.flush()?;
    drop(out);
    fs::rename(&tmp, out_path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Public utility helpers (formerly scattered between v1/v2)
// ---------------------------------------------------------------------------

pub fn parquet_files(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    if !path.is_dir() {
        anyhow::bail!("Parquet path not found: {}", path.display());
    }
    let mut files = Vec::new();
    for entry in WalkDir::new(path).into_iter().filter_map(Result::ok) {
        let p = entry.path();
        if p.is_file() && p.extension().and_then(|v| v.to_str()) == Some("parquet") {
            files.push(p.to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

pub fn ids_to_sql_list(ids: &[String]) -> String {
    ids.iter()
        .map(|id| sql_literal(id))
        .collect::<Vec<_>>()
        .join(", ")
}

pub async fn load_passage_texts_from_store(
    store: &DataFusionStore,
    limit: Option<usize>,
) -> Result<Vec<(String, String)>> {
    store.passage_texts(limit).await
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn extract_columns(
    batch: &arrow::record_batch::RecordBatch,
) -> Result<(&StringArray, &StringArray)> {
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
        .downcast_ref::<StringArray>()
        .ok_or_else(|| anyhow::anyhow!("passage_id is not StringArray"))?;
    let texts = batch
        .column(text_col)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| anyhow::anyhow!("zh_text_normalized is not StringArray"))?;
    Ok((pids, texts))
}

fn write_padded_string(field: &mut [u8], value: &str) {
    let bytes = value.as_bytes();
    let n = bytes.len().min(field.len() - 1); // leave at least one null byte
    field[..n].copy_from_slice(&bytes[..n]);
    for b in &mut field[n..] {
        *b = 0;
    }
}

fn read_padded_string(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).to_string()
}
