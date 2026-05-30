//! `sinorag ingest-dict` — convert Buddhist dictionaries to parquet.
//!
//! Reads JSON dictionary files from the cbeta-reader `dict/` directory and
//! writes a Hive-partitioned parquet store at `data/dict.parquet/source={name}/`.
//!
//! Sources:
//!   soothill.json      — Soothill-Hodous (English + Sanskrit)
//!   dfb.json           — 丁福保佛學大辭典
//!   fk.json.gz         — 佛光大辭典
//!   abdm.json          — 阿含辭典
//!   fymyj.json         — 翻譯名義集
//!   pentaglot.json     — Pentaglot (5-language)
//!   ccc.json           — CBETA composite characters
//!   cyx.json           — 佛學辭典

use crate::storage::{self, DictBatch};
use anyhow::{Context, Result};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_GLOSS_CHARS: usize = 1000;

pub fn run(
    dict_dir: PathBuf,
    out_parquet: PathBuf,
    parquet_compression: crate::storage::ParquetCompression,
) -> Result<()> {
    if !dict_dir.is_dir() {
        anyhow::bail!("dictionary directory not found: {}", dict_dir.display());
    }
    std::fs::create_dir_all(&out_parquet)?;

    let sources: Vec<(&str, &str, SourceFormat)> = vec![
        ("soothill", "soothill.json", SourceFormat::Soothill),
        ("dingfubao", "dfb.json", SourceFormat::DingFubao),
        ("foguang", "fk.json.gz", SourceFormat::SimpleMap),
        ("agama", "abdm.json", SourceFormat::SimpleMap),
        ("fymyj", "fymyj.json", SourceFormat::SimpleMap),
        ("pentaglot", "pentaglot.json", SourceFormat::Pentaglot),
        ("ccc", "ccc.json", SourceFormat::SimpleMap),
        ("cyx", "cyx.json", SourceFormat::SimpleMap),
    ];

    let mut total = 0usize;

    for (name, filename, format) in &sources {
        let path = dict_dir.join(filename);
        if !path.exists() {
            eprintln!("  skip {name}: {filename} not found");
            continue;
        }

        eprintln!("  ingesting {name} from {filename}...");
        let bytes = read_file_maybe_gz(&path)?;
        let count = ingest_source(name, &bytes, format, &out_parquet, parquet_compression)?;
        eprintln!("    {count} entries");
        total += count;
    }

    eprintln!(
        "\nDict ingest complete: {total} entries across {} sources",
        sources.len()
    );
    eprintln!("Output: {}", out_parquet.display());
    Ok(())
}

fn read_file_maybe_gz(path: &Path) -> Result<Vec<u8>> {
    let raw = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if path.extension().and_then(|e| e.to_str()) == Some("gz") {
        let mut decoder = flate2::read::GzDecoder::new(&raw[..]);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .with_context(|| format!("decompress {}", path.display()))?;
        Ok(out)
    } else {
        Ok(raw)
    }
}

enum SourceFormat {
    Soothill,
    DingFubao,
    SimpleMap,
    Pentaglot,
}

fn ingest_source(
    source_name: &str,
    bytes: &[u8],
    format: &SourceFormat,
    out_parquet: &Path,
    compression: crate::storage::ParquetCompression,
) -> Result<usize> {
    let mut batch = DictBatch::default();
    let mut count = 0usize;
    let mut part_index = 0usize;

    let entries = parse_entries(bytes, format)?;

    for (term, sanskrit, gloss, usage_cat) in entries {
        if term.chars().count() < 2 || gloss.is_empty() {
            continue;
        }
        let gloss = crate::dict::truncate_gloss(&gloss, MAX_GLOSS_CHARS);
        batch.push(term, source_name.to_string(), sanskrit, gloss, usage_cat);
        count += 1;

        if batch.len() >= storage::DICT_BATCH_SIZE {
            storage::write_dict_parquet_partitioned(
                &batch,
                out_parquet,
                source_name,
                part_index,
                compression,
            )?;
            batch.clear();
            part_index += 1;
        }
    }

    if !batch.is_empty() {
        storage::write_dict_parquet_partitioned(
            &batch,
            out_parquet,
            source_name,
            part_index,
            compression,
        )?;
    }

    Ok(count)
}

type EntryTuple = (String, Option<String>, String, Option<String>);

fn parse_entries(bytes: &[u8], format: &SourceFormat) -> Result<Vec<EntryTuple>> {
    match format {
        SourceFormat::Soothill => parse_soothill(bytes),
        SourceFormat::DingFubao => parse_dingfubao(bytes),
        SourceFormat::SimpleMap => parse_simple_map(bytes),
        SourceFormat::Pentaglot => parse_pentaglot(bytes),
    }
}

/// Soothill preprocessed JSON: array of {"t": term, "s": sanskrit, "g": gloss}
fn parse_soothill(bytes: &[u8]) -> Result<Vec<EntryTuple>> {
    #[derive(serde::Deserialize)]
    struct E {
        t: String,
        #[serde(default)]
        s: Option<String>,
        g: String,
    }
    let entries: Vec<E> = serde_json::from_slice(bytes)?;
    Ok(entries.into_iter().map(|e| (e.t, e.s, e.g, None)).collect())
}

/// 丁福保: map of term → array of {"usg": category, "def": definition}
fn parse_dingfubao(bytes: &[u8]) -> Result<Vec<EntryTuple>> {
    let map: serde_json::Map<String, Value> = serde_json::from_slice(bytes)?;
    let mut out = Vec::with_capacity(map.len());
    for (term, val) in map {
        if let Some(arr) = val.as_array() {
            for entry in arr {
                let def = entry
                    .get("def")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let usg = entry
                    .get("usg")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                if !def.is_empty() {
                    out.push((term.clone(), None, def, usg));
                }
            }
        }
    }
    Ok(out)
}

/// Simple map: term → string definition (佛光, 阿含, 翻譯名義集, ccc, cyx).
/// Some entries are strings, some are objects with a "content" or "def" field.
fn parse_simple_map(bytes: &[u8]) -> Result<Vec<EntryTuple>> {
    let map: serde_json::Map<String, Value> = serde_json::from_slice(bytes)?;
    let mut out = Vec::with_capacity(map.len());
    for (term, val) in map {
        let gloss = match &val {
            Value::String(s) => s.clone(),
            Value::Object(obj) => obj
                .get("def")
                .or_else(|| obj.get("content"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            _ => continue,
        };
        if !gloss.is_empty() {
            out.push((term, None, gloss, None));
        }
    }
    Ok(out)
}

/// Pentaglot: term → {"san": sanskrit, "mnc": manchurian, "mon": mongolian}
fn parse_pentaglot(bytes: &[u8]) -> Result<Vec<EntryTuple>> {
    let map: serde_json::Map<String, Value> = serde_json::from_slice(bytes)?;
    let mut out = Vec::with_capacity(map.len());
    for (term, val) in map {
        if let Some(obj) = val.as_object() {
            let san = obj
                .get("san")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let parts: Vec<String> = obj
                .iter()
                .map(|(k, v)| format!("{}: {}", k, v.as_str().unwrap_or("")))
                .collect();
            let gloss = parts.join("; ");
            if !gloss.is_empty() {
                out.push((term, san, gloss, None));
            }
        }
    }
    Ok(out)
}
