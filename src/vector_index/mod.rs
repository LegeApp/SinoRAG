use crate::document_table::{match_index_fingerprint, DocumentTable};
use anyhow::{anyhow, Context, Result};
use hnsw_rs::prelude::{DistL2, Hnsw};
use memmap2::Mmap;
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"SINVEC1\0";

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct VectorIndexHeader {
    pub schema: String,
    pub schema_version: u32,
    pub doc_table_fingerprint: String,
    pub source_fingerprint: String,
    pub model_id: String,
    pub model_revision: String,
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

#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingRecord {
    pub doc_id: u32,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct VectorHit {
    pub doc_id: u32,
    pub vector_score: f32,
}

pub struct VectorIndex {
    pub header: VectorIndexHeader,
    doc_ids: Vec<u32>,
    vectors: Vec<Vec<f32>>,
    hnsw: Hnsw<'static, f32, DistL2>,
    _mmap: Mmap,
}

impl VectorIndex {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let mmap = unsafe { Mmap::map(&file)? };
        let (header, doc_ids, vectors) = decode_index(&mmap)?;
        let hnsw = build_hnsw(&vectors, &doc_ids, &header.hnsw);
        Ok(Self {
            header,
            doc_ids,
            vectors,
            hnsw,
            _mmap: mmap,
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

    pub fn vector_for_doc_id(&self, doc_id: u32) -> Option<Vec<f32>> {
        self.doc_ids
            .iter()
            .position(|d| *d == doc_id)
            .map(|idx| self.vectors[idx].clone())
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
                vector_score: round_f32(1.0 / (1.0 + n.distance), 6),
            })
            .collect();
        hits.sort_by(|a, b| {
            b.vector_score
                .partial_cmp(&a.vector_score)
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
    hnsw: HnswParams,
) -> Result<VectorIndexHeader> {
    let doc_table = DocumentTable::load(doc_table_path)?;
    let (mut rows, source_fingerprint) = read_embedding_rows(embeddings_path, &doc_table)?;
    rows.sort_unstable_by_key(|r| r.doc_id);

    let dim = rows
        .first()
        .map(|r| r.embedding.len())
        .ok_or_else(|| anyhow!("embedding JSONL has no usable rows"))?;
    if dim == 0 {
        anyhow::bail!("embedding dimension must be > 0");
    }

    let mut doc_ids = Vec::with_capacity(rows.len());
    let mut vectors = Vec::with_capacity(rows.len());
    for mut row in rows {
        normalize_l2(&mut row.embedding)?;
        doc_ids.push(row.doc_id);
        vectors.push(row.embedding);
    }

    let header = VectorIndexHeader {
        schema: "sinorag-vector-index-v1".to_string(),
        schema_version: 1,
        doc_table_fingerprint: doc_table.source_fingerprint.clone(),
        source_fingerprint,
        model_id,
        model_revision,
        embedding_dim: dim as u32,
        distance: "cosine".to_string(),
        normalized: true,
        row_count: doc_ids.len() as u32,
        created_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        backend: "hnsw_rs".to_string(),
        hnsw,
    };

    write_index(out_path, &header, &doc_ids, &vectors)?;
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
    vectors: &[Vec<f32>],
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
    for vector in vectors {
        for value in vector {
            f.write_all(&value.to_le_bytes())?;
        }
    }
    f.flush()?;
    drop(f);
    fs::rename(tmp, path)?;
    Ok(())
}

fn decode_index(bytes: &[u8]) -> Result<(VectorIndexHeader, Vec<u32>, Vec<Vec<f32>>)> {
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
    let mut vectors = Vec::with_capacity(rows);
    for _ in 0..rows {
        let mut v = Vec::with_capacity(dim);
        for _ in 0..dim {
            v.push(f32::from_le_bytes(bytes[offset..offset + 4].try_into()?));
            offset += 4;
        }
        vectors.push(v);
    }
    Ok((header, doc_ids, vectors))
}

fn build_hnsw(
    vectors: &[Vec<f32>],
    doc_ids: &[u32],
    params: &HnswParams,
) -> Hnsw<'static, f32, DistL2> {
    let nb_layer = params.nb_layer.min(16).max(1);
    let hnsw = Hnsw::<f32, DistL2>::new(
        params.max_nb_connection.max(4),
        vectors.len().max(1),
        nb_layer,
        params.ef_construction.max(8),
        DistL2 {},
    );
    let data: Vec<(&Vec<f32>, usize)> = vectors
        .iter()
        .zip(doc_ids.iter())
        .map(|(v, d)| (v, *d as usize))
        .collect();
    hnsw.parallel_insert(&data);
    hnsw
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
