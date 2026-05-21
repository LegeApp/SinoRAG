use crate::document_table::{match_index_fingerprint, DocumentTable};
use anyhow::{anyhow, Context, Result};
use hnsw_rs::prelude::{AnnT, DistL2, Hnsw, HnswIo};
use memmap2::Mmap;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

// v2: HNSW graph is persisted on disk via hnsw_rs::file_dump and loaded back at
// open time, so VectorIndex::open is O(file read) rather than O(graph rebuild).
const MAGIC: &[u8; 8] = b"SINVEC2\0";

// Filename suffixes used by hnsw_rs::file_dump. The graph dump is written next
// to the .index file with basename = file_name(index_path) (e.g.
// `vector.index.hnsw.graph` and `vector.index.hnsw.data`).
const HNSW_GRAPH_SUFFIX: &str = ".hnsw.graph";
const HNSW_DATA_SUFFIX: &str = ".hnsw.data";

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct VectorIndexHeader {
    pub schema: String,
    pub schema_version: u32,
    pub doc_table_fingerprint: String,
    #[serde(default)]
    pub embeddings_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_fingerprint: Option<String>,
    pub model_id: String,
    pub model_revision: String,
    #[serde(default = "default_embedding_text_template")]
    pub embedding_text_template: String,
    #[serde(default = "default_input_text_field_policy")]
    pub input_text_field_policy: String,
    #[serde(default = "default_truncation_policy")]
    pub truncation_policy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_chars: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pooling: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
    pub embedding_dim: u32,
    pub distance: String,
    pub normalized: bool,
    pub row_count: u32,
    pub created_at_unix: u64,
    pub backend: String,
    pub hnsw: HnswParams,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HnswParams {
    pub max_nb_connection: usize,
    pub ef_construction: usize,
    pub nb_layer: usize,
}

impl Default for HnswParams {
    fn default() -> Self {
        Self {
            max_nb_connection: 32,
            ef_construction: 200,
            nb_layer: 16,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct VectorExportRecord {
    pub doc_id: u32,
    pub passage_id: String,
    pub source_work_id: Option<String>,
    pub main_title: Option<String>,
    pub heading: Option<String>,
    pub period: Option<String>,
    pub text: String,
    pub embedding_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRecord {
    pub doc_id: u32,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct VectorHit {
    pub doc_id: u32,
    pub ann_distance: f32,
    pub ann_score: f32,
}

pub struct VectorIndex {
    pub header: VectorIndexHeader,
    vectors: Vec<f32>,
    doc_id_to_row: FxHashMap<u32, usize>,
    hnsw: Hnsw<'static, f32, DistL2>,
    _mmap: Mmap,
    pub hnsw_build_ms: u128,
}

impl VectorIndex {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let mmap = unsafe { Mmap::map(&file)? };
        let (header, doc_ids, vectors) = decode_index(&mmap)?;
        let doc_id_to_row = doc_ids
            .iter()
            .enumerate()
            .map(|(row, doc_id)| (*doc_id, row))
            .collect();

        // Load the persisted HNSW graph from disk. The graph was written by
        // hnsw_rs::file_dump at build time as two files next to the index file:
        //   <index_filename>.hnsw.graph
        //   <index_filename>.hnsw.data
        // If they're missing, the index needs to be rebuilt.
        let (dump_dir, dump_basename) = hnsw_dump_location(path)?;
        let graph_path = dump_dir.join(format!("{dump_basename}{HNSW_GRAPH_SUFFIX}"));
        let data_path = dump_dir.join(format!("{dump_basename}{HNSW_DATA_SUFFIX}"));
        if !graph_path.is_file() || !data_path.is_file() {
            anyhow::bail!(
                "vector index HNSW dump missing (expected {} and {}); rebuild required",
                graph_path.display(),
                data_path.display(),
            );
        }
        let started = std::time::Instant::now();
        // HnswIo's API ties the returned Hnsw's lifetime to the reloader, even
        // when mmap is not used. The reloader itself is cheap (file readers +
        // metadata) and we want the loaded Hnsw to outlive any borrowing of
        // this index. Box::leak makes the reloader live for the rest of the
        // process — a single-use cost per index open, not per query.
        let reloader: &'static mut HnswIo =
            Box::leak(Box::new(HnswIo::new(&dump_dir, &dump_basename)));
        let hnsw: Hnsw<'static, f32, DistL2> =
            reloader.load_hnsw::<f32, DistL2>().with_context(|| {
                format!(
                    "load HNSW graph dump from {} / {}",
                    graph_path.display(),
                    data_path.display(),
                )
            })?;
        let hnsw_build_ms = started.elapsed().as_millis();
        Ok(Self {
            header,
            vectors,
            doc_id_to_row,
            hnsw,
            _mmap: mmap,
            hnsw_build_ms,
        })
    }

    pub fn header_info(path: &Path) -> Result<serde_json::Value> {
        let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let mut magic = [0u8; 8];
        file.read_exact(&mut magic)?;
        if &magic != MAGIC {
            anyhow::bail!("invalid vector index magic; rebuild required");
        }
        let mut len = [0u8; 4];
        file.read_exact(&mut len)?;
        let header_len = u32::from_le_bytes(len) as usize;
        let mut header_bytes = vec![0u8; header_len];
        file.read_exact(&mut header_bytes)?;
        let mut payload =
            serde_json::to_value(serde_json::from_slice::<VectorIndexHeader>(&header_bytes)?)?;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "file_bytes".to_string(),
                serde_json::json!(fs::metadata(path)?.len()),
            );
        }
        Ok(payload)
    }

    pub fn doc_table_fingerprint(&self) -> &str {
        &self.header.doc_table_fingerprint
    }

    pub fn vector_for_doc_id(&self, doc_id: u32) -> Option<&[f32]> {
        let row = *self.doc_id_to_row.get(&doc_id)?;
        Some(self.row_vector(row))
    }

    fn row_vector(&self, row: usize) -> &[f32] {
        let dim = self.header.embedding_dim as usize;
        &self.vectors[row * dim..(row + 1) * dim]
    }

    pub fn search_embedding(
        &self,
        query: &[f32],
        k: usize,
        ef_search: usize,
    ) -> Result<Vec<VectorHit>> {
        if query.len() != self.header.embedding_dim as usize {
            anyhow::bail!(
                "query embedding dimension {} does not match vector index dimension {}",
                query.len(),
                self.header.embedding_dim
            );
        }
        let mut q = query.to_vec();
        normalize_l2(&mut q)?;
        let raw = self.hnsw.search(&q, k.max(1), ef_search.max(k.max(1)));
        let mut hits: Vec<VectorHit> = raw
            .into_iter()
            .map(|n| VectorHit {
                doc_id: n.d_id as u32,
                ann_distance: round_f32(n.distance, 6),
                ann_score: round_f32(1.0 / (1.0 + n.distance), 6),
            })
            .collect();
        hits.sort_by(|a, b| {
            b.ann_score
                .partial_cmp(&a.ann_score)
                .unwrap_or(Ordering::Equal)
        });
        hits.truncate(k.max(1));
        Ok(hits)
    }
}

pub fn build_from_embeddings(
    doc_table_path: &Path,
    embeddings_path: &Path,
    out_path: &Path,
    model_id: String,
    model_revision: String,
    metadata: VectorBuildMetadata,
    hnsw: HnswParams,
) -> Result<VectorIndexHeader> {
    let doc_table = DocumentTable::load(doc_table_path)?;
    let (mut rows, embeddings_fingerprint) = read_embedding_rows(embeddings_path, &doc_table)?;
    rows.sort_unstable_by_key(|r| r.doc_id);

    let dim = rows
        .first()
        .map(|r| r.embedding.len())
        .ok_or_else(|| anyhow!("embedding JSONL has no usable rows"))?;
    if dim == 0 {
        anyhow::bail!("embedding dimension must be > 0");
    }

    let mut doc_ids = Vec::with_capacity(rows.len());
    let mut vectors = Vec::with_capacity(rows.len() * dim);
    for mut row in rows {
        normalize_l2(&mut row.embedding)?;
        doc_ids.push(row.doc_id);
        vectors.extend_from_slice(&row.embedding);
    }

    let header = VectorIndexHeader {
        schema: "sinorag-vector-index-v2".to_string(),
        schema_version: 2,
        doc_table_fingerprint: doc_table.source_fingerprint.clone(),
        embeddings_fingerprint,
        source_fingerprint: metadata.source_fingerprint,
        model_id,
        model_revision,
        embedding_text_template: metadata.embedding_text_template,
        input_text_field_policy: metadata.input_text_field_policy,
        truncation_policy: metadata.truncation_policy,
        max_input_chars: metadata.max_input_chars,
        pooling: metadata.pooling,
        instruction: metadata.instruction,
        embedding_dim: dim as u32,
        distance: "normalized_l2_cosine_ordering".to_string(),
        normalized: true,
        row_count: doc_ids.len() as u32,
        created_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        backend: "hnsw_rs".to_string(),
        hnsw: hnsw.clone(),
    };

    write_index(out_path, &header, &doc_ids, &vectors)?;
    build_and_dump_hnsw(out_path, &vectors, &doc_ids, dim, &hnsw)?;
    Ok(header)
}

#[derive(Debug, Clone)]
pub struct VectorBuildMetadata {
    pub source_fingerprint: Option<String>,
    pub embedding_text_template: String,
    pub input_text_field_policy: String,
    pub truncation_policy: String,
    pub max_input_chars: Option<u32>,
    pub pooling: Option<String>,
    pub instruction: Option<String>,
}

impl Default for VectorBuildMetadata {
    fn default() -> Self {
        Self {
            source_fingerprint: None,
            embedding_text_template: default_embedding_text_template(),
            input_text_field_policy: default_input_text_field_policy(),
            truncation_policy: default_truncation_policy(),
            max_input_chars: None,
            pooling: None,
            instruction: None,
        }
    }
}

/// Build a vector index directly from in-memory embedding rows (no intermediate JSONL file).
///
/// Validates that every row's doc_id is present in the doc_table and that doc_ids are unique.
/// Normalises vectors to unit length before writing.
pub fn build_from_rows(
    doc_table: &DocumentTable,
    mut rows: Vec<EmbeddingRecord>,
    out_path: &Path,
    model_id: String,
    model_revision: String,
    metadata: VectorBuildMetadata,
    hnsw: HnswParams,
) -> Result<VectorIndexHeader> {
    if rows.is_empty() {
        anyhow::bail!("no embedding rows provided");
    }

    let mut seen = FxHashSet::default();
    for row in &rows {
        if !seen.insert(row.doc_id) {
            anyhow::bail!(
                "duplicate embedding doc_id {} in embedding rows",
                row.doc_id
            );
        }
    }

    rows.sort_unstable_by_key(|r| r.doc_id);

    // Validate
    for row in &rows {
        if doc_table.passage_id(row.doc_id).is_none() {
            anyhow::bail!(
                "embedding row references unknown doc_id {} (not in doc_table)",
                row.doc_id
            );
        }
    }

    let dim = rows[0].embedding.len();
    if dim == 0 {
        anyhow::bail!("embedding dimension must be > 0");
    }
    for row in &rows {
        if row.embedding.len() != dim {
            anyhow::bail!(
                "embedding dimension mismatch for doc_id {}: expected {}, got {}",
                row.doc_id,
                dim,
                row.embedding.len()
            );
        }
    }

    // Fingerprint the raw embeddings for provenance
    let mut hasher = Sha256::new();
    for row in &rows {
        hasher.update(&row.doc_id.to_le_bytes());
        for v in &row.embedding {
            hasher.update(&v.to_le_bytes());
        }
    }
    let embeddings_fingerprint = hex::encode(hasher.finalize());

    let mut doc_ids = Vec::with_capacity(rows.len());
    let mut vectors = Vec::with_capacity(rows.len() * dim);
    for mut row in rows {
        normalize_l2(&mut row.embedding)?;
        doc_ids.push(row.doc_id);
        vectors.extend_from_slice(&row.embedding);
    }

    let header = VectorIndexHeader {
        schema: "sinorag-vector-index-v2".to_string(),
        schema_version: 2,
        doc_table_fingerprint: doc_table.source_fingerprint.clone(),
        embeddings_fingerprint,
        source_fingerprint: metadata.source_fingerprint,
        model_id,
        model_revision,
        embedding_text_template: metadata.embedding_text_template,
        input_text_field_policy: metadata.input_text_field_policy,
        truncation_policy: metadata.truncation_policy,
        max_input_chars: metadata.max_input_chars,
        pooling: metadata.pooling,
        instruction: metadata.instruction,
        embedding_dim: dim as u32,
        distance: "normalized_l2_cosine_ordering".to_string(),
        normalized: true,
        row_count: doc_ids.len() as u32,
        created_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        backend: "hnsw_rs".to_string(),
        hnsw: hnsw.clone(),
    };

    write_index(out_path, &header, &doc_ids, &vectors)?;
    build_and_dump_hnsw(out_path, &vectors, &doc_ids, dim, &hnsw)?;
    Ok(header)
}

pub fn ensure_matches_doc_table(
    index: &VectorIndex,
    doc_table: &DocumentTable,
    doc_table_path: &Path,
) -> Result<()> {
    if match_index_fingerprint(doc_table, doc_table_path, index.doc_table_fingerprint())?.is_none()
    {
        anyhow::bail!(
            "vector index fingerprint does not match active doc_table; rebuild vector index for {}",
            doc_table_path.display()
        );
    }
    Ok(())
}

pub fn normalize_l2(v: &mut [f32]) -> Result<()> {
    if v.iter().any(|x| !x.is_finite()) {
        anyhow::bail!("embedding contains NaN or infinite value");
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        anyhow::bail!("embedding vector has zero norm");
    }
    for x in v {
        *x /= norm;
    }
    Ok(())
}

fn read_embedding_rows(
    path: &Path,
    doc_table: &DocumentTable,
) -> Result<(Vec<EmbeddingRecord>, String)> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut rows = Vec::new();
    let mut seen = FxHashSet::default();
    let mut dim: Option<usize> = None;
    let mut hasher = Sha256::new();
    let mut line = String::new();
    let mut line_no = 0usize;

    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        line_no += 1;
        if line.trim().is_empty() {
            continue;
        }
        hasher.update(line.as_bytes());
        let row: EmbeddingRecord = serde_json::from_str(&line)
            .with_context(|| format!("parse embedding JSONL line {line_no}"))?;
        if doc_table.passage_id(row.doc_id).is_none() {
            anyhow::bail!(
                "embedding line {line_no} references unknown doc_id {}",
                row.doc_id
            );
        }
        if !seen.insert(row.doc_id) {
            anyhow::bail!(
                "duplicate embedding doc_id {} at line {line_no}",
                row.doc_id
            );
        }
        match dim {
            Some(d) if d != row.embedding.len() => anyhow::bail!(
                "embedding dimension mismatch at line {line_no}: expected {d}, got {}",
                row.embedding.len()
            ),
            None => dim = Some(row.embedding.len()),
            _ => {}
        }
        rows.push(row);
    }
    if rows.is_empty() {
        anyhow::bail!("embedding JSONL has no rows");
    }
    let source_fingerprint = hex::encode(hasher.finalize());
    Ok((rows, source_fingerprint))
}

fn write_index(
    path: &Path,
    header: &VectorIndexHeader,
    doc_ids: &[u32],
    vectors: &[f32],
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("index.tmp");
    let mut f = BufWriter::new(File::create(&tmp)?);
    let header_bytes = serde_json::to_vec(header)?;
    f.write_all(MAGIC)?;
    f.write_all(&(header_bytes.len() as u32).to_le_bytes())?;
    f.write_all(&header_bytes)?;
    for doc_id in doc_ids {
        f.write_all(&doc_id.to_le_bytes())?;
    }
    for value in vectors {
        f.write_all(&value.to_le_bytes())?;
    }
    f.flush()?;
    drop(f);
    fs::rename(tmp, path)?;
    Ok(())
}

fn decode_index(bytes: &[u8]) -> Result<(VectorIndexHeader, Vec<u32>, Vec<f32>)> {
    if bytes.len() < 12 {
        anyhow::bail!("vector index too small");
    }
    if &bytes[0..8] != MAGIC {
        anyhow::bail!("invalid vector index magic; rebuild required");
    }
    let header_len = u32::from_le_bytes(bytes[8..12].try_into()?) as usize;
    let header_start = 12;
    let header_end = header_start + header_len;
    if header_end > bytes.len() {
        anyhow::bail!("vector index header exceeds file length");
    }
    let header: VectorIndexHeader = serde_json::from_slice(&bytes[header_start..header_end])?;
    let rows = header.row_count as usize;
    let dim = header.embedding_dim as usize;
    let row_table_bytes = rows * 4;
    let vector_bytes = rows
        .checked_mul(dim)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| anyhow!("vector index dimensions overflow"))?;
    let expected = header_end + row_table_bytes + vector_bytes;
    if expected > bytes.len() {
        anyhow::bail!("vector index sections exceed file length");
    }

    let mut doc_ids = Vec::with_capacity(rows);
    let mut offset = header_end;
    for _ in 0..rows {
        doc_ids.push(u32::from_le_bytes(bytes[offset..offset + 4].try_into()?));
        offset += 4;
    }
    let mut vectors = Vec::with_capacity(rows * dim);
    for _ in 0..rows * dim {
        vectors.push(f32::from_le_bytes(bytes[offset..offset + 4].try_into()?));
        offset += 4;
    }
    Ok((header, doc_ids, vectors))
}

/// Resolve the directory + basename used by hnsw_rs::file_dump for the dump
/// files associated with `index_path`. The dump files live next to the
/// .index file with basename equal to the .index filename, so the on-disk
/// files are e.g. `vector.index.hnsw.graph` and `vector.index.hnsw.data`.
fn hnsw_dump_location(index_path: &Path) -> Result<(PathBuf, String)> {
    let dir = index_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let basename = index_path
        .file_name()
        .ok_or_else(|| anyhow!("index path {} has no file name", index_path.display()))?
        .to_string_lossy()
        .into_owned();
    Ok((dir, basename))
}

/// Build the HNSW graph from normalised vectors and persist it next to the
/// index file via hnsw_rs::file_dump. Removes any stale dump files first so
/// the dump is always the one paired with the current index file.
fn build_and_dump_hnsw(
    index_path: &Path,
    vectors: &[f32],
    doc_ids: &[u32],
    dim: usize,
    params: &HnswParams,
) -> Result<()> {
    let nb_layer = params.nb_layer.min(16).max(1);
    let hnsw = Hnsw::<f32, DistL2>::new(
        params.max_nb_connection.max(4),
        doc_ids.len().max(1),
        nb_layer,
        params.ef_construction.max(8),
        DistL2 {},
    );
    let row_vectors: Vec<Vec<f32>> = doc_ids
        .iter()
        .enumerate()
        .map(|(row, _)| vectors[row * dim..(row + 1) * dim].to_vec())
        .collect();
    let data: Vec<(&Vec<f32>, usize)> = row_vectors
        .iter()
        .zip(doc_ids.iter())
        .map(|(v, doc_id)| (v, *doc_id as usize))
        .collect();
    hnsw.parallel_insert(&data);

    let (dump_dir, basename) = hnsw_dump_location(index_path)?;
    // Remove stale dumps so file_dump (which uses overwrite-style unique-name
    // logic when the file exists) always writes to the canonical basename.
    for suffix in [HNSW_GRAPH_SUFFIX, HNSW_DATA_SUFFIX] {
        let p = dump_dir.join(format!("{basename}{suffix}"));
        if p.exists() {
            fs::remove_file(&p)
                .with_context(|| format!("remove stale HNSW dump {}", p.display()))?;
        }
    }
    hnsw.file_dump(&dump_dir, &basename)
        .map_err(|e| anyhow!("HNSW file_dump failed: {e}"))?;
    Ok(())
}

fn default_embedding_text_template() -> String {
    "Work: {main_title}\\nSection: {heading}\\nPeriod: {period}\\nText:\\n{text}".to_string()
}

fn default_input_text_field_policy() -> String {
    "vector-export embedding_text field".to_string()
}

fn default_truncation_policy() -> String {
    "external_provider_policy".to_string()
}

fn round_f32(value: f32, places: i32) -> f32 {
    let factor = 10f32.powi(places);
    (value * factor).round() / factor
}

pub async fn export_jsonl(
    parquet_path: PathBuf,
    doc_table_path: PathBuf,
    out_path: PathBuf,
    limit: Option<usize>,
) -> Result<usize> {
    let store = crate::datafusion_store::DataFusionStore::open(&parquet_path).await?;
    let doc_table = DocumentTable::load(&doc_table_path)?;
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let limit_sql = limit.map(|n| format!(" LIMIT {n}")).unwrap_or_default();
    let rows = store
        .query_json(&format!(
            "SELECT passage_id, source_work_id, main_title, heading, period, zh_text_raw, zh_text_normalized \
             FROM passages WHERE passage_id IS NOT NULL ORDER BY passage_id{limit_sql}"
        ))
        .await?;
    let mut writer = BufWriter::new(File::create(out_path)?);
    let mut written = 0usize;
    for row in rows {
        let passage_id = row
            .get("passage_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let Some(doc_id) = doc_table.doc_id(passage_id) else {
            continue;
        };
        let text = value_str(&row, "zh_text_raw").or_else(|| value_str(&row, "zh_text_normalized"));
        let embedding_text = format!(
            "Work: {}\nSection: {}\nPeriod: {}\nText:\n{}",
            value_str(&row, "main_title").unwrap_or_default(),
            value_str(&row, "heading").unwrap_or_default(),
            value_str(&row, "period").unwrap_or_default(),
            text.clone().unwrap_or_default()
        );
        let record = VectorExportRecord {
            doc_id,
            passage_id: passage_id.to_string(),
            source_work_id: value_str(&row, "source_work_id"),
            main_title: value_str(&row, "main_title"),
            heading: value_str(&row, "heading"),
            period: value_str(&row, "period"),
            text: text.unwrap_or_default(),
            embedding_text,
        };
        serde_json::to_writer(&mut writer, &record)?;
        writer.write_all(b"\n")?;
        written += 1;
    }
    writer.flush()?;
    Ok(written)
}

fn value_str(row: &serde_json::Value, key: &str) -> Option<String> {
    row.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rejects_zero_vector() {
        let mut v = vec![0.0, 0.0];
        assert!(normalize_l2(&mut v).is_err());
    }

    #[test]
    fn normalize_unit_scales_nonzero_vector() {
        let mut v = vec![3.0, 4.0];
        normalize_l2(&mut v).unwrap();
        assert!((v[0] - 0.6).abs() < 0.0001);
        assert!((v[1] - 0.8).abs() < 0.0001);
    }

    #[test]
    fn build_rejects_duplicate_doc_ids() {
        let dir = tempfile::tempdir().unwrap();
        let doc_table_path = dir.path().join("doc_table.bin");
        tiny_doc_table().save(&doc_table_path).unwrap();
        let embeddings = dir.path().join("embeddings.jsonl");
        fs::write(
            &embeddings,
            "{\"doc_id\":0,\"embedding\":[1.0,0.0]}\n{\"doc_id\":0,\"embedding\":[0.0,1.0]}\n",
        )
        .unwrap();
        let out = dir.path().join("vector.index");
        let err = build_from_embeddings(
            &doc_table_path,
            &embeddings,
            &out,
            "test-model".to_string(),
            "test".to_string(),
            VectorBuildMetadata::default(),
            HnswParams::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("duplicate embedding doc_id"));
    }

    #[test]
    fn build_rejects_dimension_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let doc_table_path = dir.path().join("doc_table.bin");
        tiny_doc_table().save(&doc_table_path).unwrap();
        let embeddings = dir.path().join("embeddings.jsonl");
        fs::write(
            &embeddings,
            "{\"doc_id\":0,\"embedding\":[1.0,0.0]}\n{\"doc_id\":1,\"embedding\":[0.0,1.0,0.0]}\n",
        )
        .unwrap();
        let out = dir.path().join("vector.index");
        let err = build_from_embeddings(
            &doc_table_path,
            &embeddings,
            &out,
            "test-model".to_string(),
            "test".to_string(),
            VectorBuildMetadata::default(),
            HnswParams::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("dimension mismatch"));
    }

    #[test]
    fn build_open_and_search_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let doc_table_path = dir.path().join("doc_table.bin");
        tiny_doc_table().save(&doc_table_path).unwrap();
        let embeddings = dir.path().join("embeddings.jsonl");
        fs::write(
            &embeddings,
            "{\"doc_id\":0,\"embedding\":[1.0,0.0]}\n{\"doc_id\":1,\"embedding\":[0.9,0.1]}\n{\"doc_id\":2,\"embedding\":[0.0,1.0]}\n",
        )
        .unwrap();
        let out = dir.path().join("vector.index");
        build_from_embeddings(
            &doc_table_path,
            &embeddings,
            &out,
            "test-model".to_string(),
            "test".to_string(),
            VectorBuildMetadata::default(),
            HnswParams::default(),
        )
        .unwrap();
        let index = VectorIndex::open(&out).unwrap();
        assert_eq!(index.header.embedding_dim, 2);
        let hits = index.search_embedding(&[0.8, 0.2], 2, 16).unwrap();
        assert_eq!(hits[0].doc_id, 1);
    }

    fn tiny_doc_table() -> DocumentTable {
        let mut dt = DocumentTable::new();
        dt.source_fingerprint = "abcdef".to_string();
        dt.passage_ids = vec!["p0".to_string(), "p1".to_string(), "p2".to_string()];
        dt.passage_lookup_order = vec![0, 1, 2];
        dt.source_work_ids = vec![0, 0, 0];
        dt.period_ranks = vec![0, 0, 0];
        dt.work_strings = vec!["w".to_string()];
        dt.work_doc_offsets = vec![0, 3];
        dt.work_doc_ids = vec![0, 1, 2];
        dt
    }
}
