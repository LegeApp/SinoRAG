//! `sinorag patch-metadata` — one-time pack maintenance.
//!
//! Fills blank `author`, `period`, `period_rank`, and `main_title` columns in
//! `passages-raw.parquet` using the authoritative CBETA work catalog embedded
//! in the binary (`sutra_sch.lst`).
//!
//! Only overwrites values that are empty / "Unknown Period" — rows with
//! existing non-blank data are never touched.
//!
//! Run this after `sinorag ingest` and before `sinorag pack-create`.

use crate::arrow_helpers::{col_i32, col_str};
use anyhow::{Context, Result};
use arrow::array::{ArrayRef, Int32Array, StringArray};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

pub fn run(parquet_dir: PathBuf, dry_run: bool) -> Result<()> {
    let patch_map = build_patch_map();
    eprintln!("Patch entries from catalog: {}", patch_map.len());

    let files = crate::phrase_index::parquet_files(&parquet_dir)
        .with_context(|| format!("reading parquet dir {}", parquet_dir.display()))?;

    if files.is_empty() {
        anyhow::bail!("no parquet files found in {}", parquet_dir.display());
    }

    let mut total_rows = 0u64;
    let mut patched_rows = 0u64;
    let mut patched_works: std::collections::BTreeSet<String> = Default::default();

    for file_path in &files {
        let (rows, patched, works) = patch_file(file_path, &patch_map, dry_run)
            .with_context(|| format!("patching {}", file_path.display()))?;
        total_rows += rows;
        patched_rows += patched;
        patched_works.extend(works);
    }

    eprintln!("\nTotal rows:    {total_rows}");
    eprintln!("Patched rows:  {patched_rows}");
    eprintln!("Patched works: {}", patched_works.len());
    if !patched_works.is_empty() {
        let preview = patched_works.iter().take(20).cloned().collect::<Vec<_>>();
        for w in &preview {
            eprintln!("  {w}");
        }
        if patched_works.len() > 20 {
            eprintln!("  … and {} more", patched_works.len() - 20);
        }
    }
    if dry_run {
        eprintln!("\n(dry run — no files written)");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-file patch logic
// ---------------------------------------------------------------------------

fn patch_file(
    path: &std::path::Path,
    patch_map: &HashMap<String, PatchEntry>,
    dry_run: bool,
) -> Result<(u64, u64, Vec<String>)> {
    // Read all batches from file.
    let file = File::open(path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let schema = builder.schema().clone();
    let reader = builder.build()?;

    let mut batches: Vec<RecordBatch> = Vec::new();
    let mut total_rows = 0u64;
    let mut patched_rows = 0u64;
    let mut patched_works: Vec<String> = Vec::new();

    // Column indices (validated once from first schema read).
    let work_id_idx = schema.index_of("source_work_id")?;
    let author_idx = schema.index_of("author")?;
    let period_idx = schema.index_of("period")?;
    let period_rank_idx = schema.index_of("period_rank")?;
    let main_title_idx = schema.index_of("main_title")?;

    for batch_result in reader {
        let batch = batch_result?;
        let n = batch.num_rows();
        total_rows += n as u64;

        let work_ids = col_str(&batch, work_id_idx)?;
        let authors = col_str(&batch, author_idx)?;
        let periods = col_str(&batch, period_idx)?;
        let period_ranks = col_i32(&batch, period_rank_idx)?;
        let main_titles = col_str(&batch, main_title_idx)?;

        // Build new column vecs; start as clones of existing values.
        let mut new_authors: Vec<String> = (0..n).map(|i| authors.value(i).to_string()).collect();
        let mut new_periods: Vec<String> = (0..n).map(|i| periods.value(i).to_string()).collect();
        let mut new_ranks: Vec<i32> = (0..n).map(|i| period_ranks.value(i)).collect();
        let mut new_titles: Vec<String> =
            (0..n).map(|i| main_titles.value(i).to_string()).collect();

        for i in 0..n {
            let work_id = work_ids.value(i);
            let needs_author = new_authors[i].is_empty();
            let needs_period = new_periods[i].is_empty() || new_periods[i] == "Unknown Period";
            let needs_title = new_titles[i].is_empty();

            if !needs_author && !needs_period && !needs_title {
                continue;
            }

            let Some(patch) = patch_map.get(work_id) else {
                continue;
            };

            let mut row_patched = false;

            if needs_author {
                if let Some(ref a) = patch.author {
                    new_authors[i] = a.clone();
                    row_patched = true;
                }
            }
            if needs_period {
                if let Some(ref p) = patch.period {
                    new_periods[i] = p.clone();
                    new_ranks[i] = patch.period_rank;
                    row_patched = true;
                }
            }
            if needs_title {
                if !patch.title.is_empty() {
                    new_titles[i] = patch.title.clone();
                    row_patched = true;
                }
            }

            if row_patched {
                patched_rows += 1;
                patched_works.push(work_id.to_string());
            }
        }

        // Rebuild the batch with replaced columns.
        let new_batch = replace_columns(
            &batch,
            &[
                (
                    author_idx,
                    Arc::new(StringArray::from(new_authors)) as ArrayRef,
                ),
                (
                    period_idx,
                    Arc::new(StringArray::from(new_periods)) as ArrayRef,
                ),
                (
                    period_rank_idx,
                    Arc::new(Int32Array::from(new_ranks)) as ArrayRef,
                ),
                (
                    main_title_idx,
                    Arc::new(StringArray::from(new_titles)) as ArrayRef,
                ),
            ],
        )?;
        batches.push(new_batch);
    }

    if dry_run || patched_rows == 0 {
        return Ok((total_rows, patched_rows, patched_works));
    }

    // Write patched batches to a temp file then rename over original.
    let tmp_path = path.with_extension("parquet.tmp");
    {
        let tmp_file = File::create(&tmp_path)
            .with_context(|| format!("creating temp file {}", tmp_path.display()))?;
        let props = WriterProperties::builder()
            .set_compression(parquet::basic::Compression::UNCOMPRESSED)
            .build();
        let mut writer = ArrowWriter::try_new(tmp_file, schema.clone(), Some(props))?;
        for batch in &batches {
            writer.write(batch)?;
        }
        writer.close()?;
    }
    std::fs::rename(&tmp_path, path)
        .with_context(|| format!("renaming {} over {}", tmp_path.display(), path.display()))?;

    Ok((total_rows, patched_rows, patched_works))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn replace_columns(batch: &RecordBatch, replacements: &[(usize, ArrayRef)]) -> Result<RecordBatch> {
    let schema = batch.schema();
    let cols: Vec<ArrayRef> = (0..batch.num_columns())
        .map(|i| {
            replacements
                .iter()
                .find(|(idx, _)| *idx == i)
                .map(|(_, arr)| arr.clone())
                .unwrap_or_else(|| batch.column(i).clone())
        })
        .collect();
    Ok(RecordBatch::try_new(schema, cols)?)
}

// ---------------------------------------------------------------------------
// Patch map: work_id → (title, author, period, period_rank)
// ---------------------------------------------------------------------------

struct PatchEntry {
    title: String,
    author: Option<String>,
    period: Option<String>,
    period_rank: i32,
}

fn build_patch_map() -> HashMap<String, PatchEntry> {
    let catalog = crate::cbeta_sidecar::work_catalog();
    let mut map = HashMap::with_capacity(catalog.len());

    for work_id in catalog.work_ids() {
        let Some(entry) = catalog.get(work_id) else {
            continue;
        };
        let (author, period, rank) =
            crate::cbeta_sidecar::parse_catalog_translator(&entry.translator_field);
        map.insert(
            work_id.clone(),
            PatchEntry {
                title: entry.title.clone(),
                author,
                period,
                period_rank: rank,
            },
        );
    }
    map
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_parse_translator() {
        use crate::cbeta_sidecar::parse_catalog_translator;
        let (a, p, r) = parse_catalog_translator("後秦 佛陀耶舍共竺佛念譯");
        assert_eq!(a.as_deref(), Some("佛陀耶舍共竺佛念"));
        assert_eq!(p.as_deref(), Some("Northern and Southern"));
        assert_eq!(r, 4);

        let (a, p, r) = parse_catalog_translator("唐 不空譯");
        assert_eq!(a.as_deref(), Some("不空"));
        assert_eq!(p.as_deref(), Some("Tang"));
        assert_eq!(r, 6);

        let (a, p, r) = parse_catalog_translator("宋 施護等譯");
        assert_eq!(a.as_deref(), Some("施護"));
        assert_eq!(p.as_deref(), Some("Song"));
        assert_eq!(r, 8);

        let (a, p, _) = parse_catalog_translator("失譯");
        assert_eq!(a.as_deref(), Some("失譯"));
        assert!(p.is_none());

        let (a, p, _) = parse_catalog_translator("");
        assert!(a.is_none());
        assert!(p.is_none());

        let (a, p, _) = parse_catalog_translator("黃謹良譯");
        assert_eq!(a.as_deref(), Some("黃謹良"));
        assert!(p.is_none());
    }

    #[test]
    fn test_strip_verb() {
        use crate::cbeta_sidecar::strip_translation_verb;
        assert_eq!(strip_translation_verb("不空譯"), "不空");
        assert_eq!(strip_translation_verb("施護等譯"), "施護");
        assert_eq!(strip_translation_verb("吉藏撰"), "吉藏");
        // "失" — '譯' is correctly stripped; parse_catalog_translator handles
        // the "失譯" anonymous-translator convention at a higher level.
        assert_eq!(strip_translation_verb("失譯"), "失");
    }
}
