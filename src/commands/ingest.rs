//! Ingest pipeline with atomic staging-directory output and safe resume.
//!
//! Layout while running:
//! ```text
//!   <out>/.staging/ingest-<utc_id>/
//!     passages.jsonl
//!     passages.parquet/
//!       source_corpus=cbeta/part-*.parquet
//!       source_corpus=kanripo/part-*.parquet
//!     .ingest_checkpoint.json
//! ```
//!
//! On success, each `source_corpus=*` partition is atomically renamed
//! into `<out>/passages.parquet/`; the jsonl is renamed into `<out>/`;
//! the staging dir is deleted. If a target partition already exists,
//! the run aborts before any rename (no silent overwrite).

use crate::models::PassageRecord;
use crate::{ingest, storage, tei};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

const CHECKPOINT_SCHEMA: &str = "sinorag-ingest-checkpoint-v1";
const CHECKPOINT_FLUSH_INTERVAL: usize = 100;

pub struct PostIngestOptions {
    pub out_parquet: PathBuf,
    pub build_phrase_index: bool,
    pub phrase_index_out: PathBuf,
    pub phrase_gram_len: usize,
    pub build_tfidf: bool,
    pub tfidf_out: Option<PathBuf>,
    pub catalog_index_out: Option<PathBuf>,
    pub phrase_max_memory: Option<u64>,
    /// Run phrase + TF-IDF builds in parallel threads after doc_table.
    /// Roughly 1.5–1.7× faster but ~doubles peak memory.
    pub parallel_lexical: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct Checkpoint {
    schema: String,
    run_id: String,
    started_utc: String,
    processed_files: HashSet<String>,
    /// Per-corpus next part index. Already-written part files are kept;
    /// resume continues at this index. Avoids the prior overwrite bug.
    next_part_index: HashMap<String, usize>,
    stats: CheckpointStats,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CheckpointStats {
    cbeta: usize,
    kanripo: usize,
    total: usize,
}

pub async fn run(
    corpus: Option<PathBuf>,
    kanripo_input: Option<PathBuf>,
    out_jsonl: PathBuf,
    out_parquet: PathBuf,
    zen_only: bool,
    resume: Option<PathBuf>,
    append: bool,
    build_phrase_index: bool,
    phrase_index_out: PathBuf,
    phrase_gram_len: usize,
    build_tfidf: bool,
    tfidf_out: Option<PathBuf>,
    catalog_index_out: Option<PathBuf>,
    phrase_max_memory: Option<u64>,
    parallel_lexical: bool,
) -> Result<()> {
    let out = PathBuf::from("data");

    if corpus.is_none() && kanripo_input.is_none() {
        anyhow::bail!("ingest requires at least one of --corpus or --kanripo-input");
    }

    // -- Resolve / set up staging dir + checkpoint -----------------------
    let (staging_root, mut checkpoint, resuming) = match resume {
        Some(path) => {
            let p = if path.as_os_str() == "auto" {
                find_freshest_staging(&out)?.ok_or_else(|| {
                    anyhow!("no .staging/ingest-* dir found under {}", out.display())
                })?
            } else {
                path
            };
            if !p.is_dir() {
                anyhow::bail!("--resume target is not a directory: {}", p.display());
            }
            let cp_path = p.join(".ingest_checkpoint.json");
            if !cp_path.exists() {
                anyhow::bail!(
                    "staging dir has no .ingest_checkpoint.json: {}",
                    p.display()
                );
            }
            let cp: Checkpoint =
                serde_json::from_slice(&std::fs::read(&cp_path)?).context("parse checkpoint")?;
            if cp.schema != CHECKPOINT_SCHEMA {
                anyhow::bail!(
                    "checkpoint schema `{}` (expected `{}`)",
                    cp.schema,
                    CHECKPOINT_SCHEMA
                );
            }
            eprintln!(
                "resuming run {} ({} files already processed)",
                cp.run_id,
                cp.processed_files.len()
            );
            (p, cp, true)
        }
        None => {
            let run_id = format!("ingest-{}", Utc::now().format("%Y%m%dT%H%M%SZ"));
            let staging = out.join(".staging").join(&run_id);
            std::fs::create_dir_all(&staging)?;
            std::fs::create_dir_all(staging.join("passages.parquet"))?;
            let cp = Checkpoint {
                schema: CHECKPOINT_SCHEMA.to_string(),
                run_id,
                started_utc: Utc::now().to_rfc3339(),
                processed_files: HashSet::new(),
                next_part_index: HashMap::new(),
                stats: CheckpointStats::default(),
            };
            (staging, cp, false)
        }
    };

    // -- Pre-flight: refuse if target jsonl exists, unless appending to an
    // existing store (caller already verified the incoming corpus is new).
    if !resuming && !append {
        if out_jsonl.exists() {
            anyhow::bail!(
                "target jsonl already exists: {}. Move/delete or use a different --out-jsonl.",
                out_jsonl.display()
            );
        }
    }

    let staging_parquet = staging_root.join("passages.parquet");
    let staging_jsonl = staging_root.join("passages.jsonl");
    let checkpoint_path = staging_root.join(".ingest_checkpoint.json");

    if let Some(parent) = out_jsonl.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Some(parent) = out_parquet.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // -- jsonl writer ----------------------------------------------------
    let jsonl_file = if resuming && staging_jsonl.exists() {
        File::options().append(true).open(&staging_jsonl)?
    } else {
        File::create(&staging_jsonl)?
    };
    let mut jsonl = BufWriter::new(jsonl_file);

    // -- Per-corpus batches + part counters ------------------------------
    let mut cbeta_batch = storage::PassageBatch::default();
    let mut kanripo_batch = storage::PassageBatch::default();
    let mut cbeta_part_index = checkpoint
        .next_part_index
        .get("cbeta")
        .copied()
        .unwrap_or(0);
    let mut kanripo_part_index = checkpoint
        .next_part_index
        .get("kanripo")
        .copied()
        .unwrap_or(0);
    let mut total = checkpoint.stats.total;
    let mut cbeta_count = checkpoint.stats.cbeta;
    let mut kanripo_count = checkpoint.stats.kanripo;

    let processed_files = std::mem::take(&mut checkpoint.processed_files);
    let mut processed_files = processed_files;

    let emit = |passage: &PassageRecord,
                batch: &mut storage::PassageBatch,
                part_index: &mut usize,
                jsonl: &mut BufWriter<File>,
                corpus_name: &str|
     -> Result<()> {
        serde_json::to_writer(&mut *jsonl, passage)?;
        jsonl.write_all(b"\n")?;
        batch.push(passage)?;
        if batch.len() >= storage::PARQUET_BATCH_SIZE {
            storage::write_parquet_part_partitioned(
                batch,
                &staging_parquet,
                corpus_name,
                *part_index,
            )?;
            batch.clear();
            *part_index += 1;
        }
        Ok(())
    };

    let save_checkpoint = |processed: &HashSet<String>,
                           cbeta_idx: usize,
                           kanripo_idx: usize,
                           cbeta_c: usize,
                           kanripo_c: usize,
                           tot: usize,
                           cp: &Checkpoint|
     -> Result<()> {
        let snap = Checkpoint {
            schema: CHECKPOINT_SCHEMA.to_string(),
            run_id: cp.run_id.clone(),
            started_utc: cp.started_utc.clone(),
            processed_files: processed.clone(),
            next_part_index: {
                let mut m = HashMap::new();
                m.insert("cbeta".to_string(), cbeta_idx);
                m.insert("kanripo".to_string(), kanripo_idx);
                m
            },
            stats: CheckpointStats {
                cbeta: cbeta_c,
                kanripo: kanripo_c,
                total: tot,
            },
        };
        std::fs::write(&checkpoint_path, serde_json::to_vec_pretty(&snap)?)?;
        Ok(())
    };

    eprintln!("staging at {}", staging_root.display());

    // -- CBETA -----------------------------------------------------------
    if let Some(corpus_root) = corpus.as_ref() {
        let paths = tei::iter_xml_paths(corpus_root)?;
        let pb = ProgressBar::new(paths.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
                )
                .unwrap()
                .progress_chars("#>-"),
        );
        pb.set_message("Processing CBETA XML files");

        for (xml_path, rel_path) in paths {
            if resuming && processed_files.contains(&rel_path) {
                pb.inc(1);
                continue;
            }
            let meta = tei::extract_metadata_from_xml(&xml_path, &rel_path);
            if zen_only && !meta.traditions.iter().any(|t| t == "Chan/Zen") {
                pb.inc(1);
                continue;
            }
            for passage in tei::extract_passages_from_file(&xml_path, &rel_path, &meta)? {
                emit(
                    &passage,
                    &mut cbeta_batch,
                    &mut cbeta_part_index,
                    &mut jsonl,
                    "cbeta",
                )?;
                cbeta_count += 1;
                total += 1;
            }
            processed_files.insert(rel_path);
            if processed_files.len() % CHECKPOINT_FLUSH_INTERVAL == 0 {
                save_checkpoint(
                    &processed_files,
                    cbeta_part_index,
                    kanripo_part_index,
                    cbeta_count,
                    kanripo_count,
                    total,
                    &checkpoint,
                )?;
            }
            pb.inc(1);
        }
        pb.finish_with_message("CBETA ingest complete");
    }

    // -- Kanripo ---------------------------------------------------------
    if let Some(kanripo_root) = kanripo_input.as_ref() {
        let scan_root = if kanripo_root.join("texts").is_dir() {
            kanripo_root.join("texts")
        } else {
            kanripo_root.to_path_buf()
        };
        let repos = ingest::discover_work_repos(&scan_root)?;
        let mut total_sections = 0usize;
        for repo in &repos {
            if let Some(work_id) = ingest::work_id_for_repo(repo) {
                if let Ok(sections) = ingest::section_files(repo, &work_id) {
                    total_sections += sections.len();
                }
            }
        }
        let pb = ProgressBar::new(total_sections as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
                )
                .unwrap()
                .progress_chars("#>-"),
        );
        pb.set_message("Processing Kanripo sections");

        for repo in repos {
            let work_id = match ingest::work_id_for_repo(&repo) {
                Some(v) => v,
                None => continue,
            };
            let title = ingest::read_title(&repo).unwrap_or_else(|| work_id.clone());
            let (edition_siglum, edition_label) = ingest::read_edition(&repo);
            let snapshot = ingest::git_head(&repo).unwrap_or_default();
            let rel_repo = repo
                .strip_prefix(&scan_root)?
                .to_string_lossy()
                .replace('\\', "/");
            let sections = ingest::section_files(&repo, &work_id)?;
            for section in sections {
                let section_rel = section.to_string_lossy().into_owned();
                if resuming && processed_files.contains(&section_rel) {
                    pb.inc(1);
                    continue;
                }
                let mut section_passages = Vec::new();
                ingest::extract_section_passages(
                    &section,
                    &work_id,
                    &title,
                    &edition_siglum,
                    &edition_label,
                    &snapshot,
                    &rel_repo,
                    &mut section_passages,
                )?;
                for passage in section_passages {
                    emit(
                        &passage,
                        &mut kanripo_batch,
                        &mut kanripo_part_index,
                        &mut jsonl,
                        "kanripo",
                    )?;
                    kanripo_count += 1;
                    total += 1;
                }
                processed_files.insert(section_rel);
                pb.inc(1);
                if kanripo_count % 1000 == 0 {
                    save_checkpoint(
                        &processed_files,
                        cbeta_part_index,
                        kanripo_part_index,
                        cbeta_count,
                        kanripo_count,
                        total,
                        &checkpoint,
                    )?;
                }
            }
        }
        pb.finish_with_message("Kanripo ingest complete");
    }

    // -- Final flush -----------------------------------------------------
    if !cbeta_batch.is_empty() {
        storage::write_parquet_part_partitioned(
            &cbeta_batch,
            &staging_parquet,
            "cbeta",
            cbeta_part_index,
        )?;
        cbeta_part_index += 1;
    }
    if !kanripo_batch.is_empty() {
        storage::write_parquet_part_partitioned(
            &kanripo_batch,
            &staging_parquet,
            "kanripo",
            kanripo_part_index,
        )?;
        kanripo_part_index += 1;
    }
    jsonl.flush()?;
    save_checkpoint(
        &processed_files,
        cbeta_part_index,
        kanripo_part_index,
        cbeta_count,
        kanripo_count,
        total,
        &checkpoint,
    )?;

    // -- Atomic promotion to final location ------------------------------
    eprintln!("\n=== promoting staging → {} ===", out_parquet.display());
    promote_staging(
        &staging_root,
        &staging_parquet,
        &staging_jsonl,
        &out_jsonl,
        &out_parquet,
        append,
    )?;

    println!("wrote {}", out_jsonl.display());
    println!("wrote {}/", out_parquet.display());
    println!("passages {total}");
    println!("  cbeta {cbeta_count}");
    println!("  kanripo {kanripo_count}");

    post_ingest(PostIngestOptions {
        out_parquet,
        build_phrase_index,
        phrase_index_out,
        phrase_gram_len,
        build_tfidf,
        tfidf_out,
        catalog_index_out,
        phrase_max_memory,
        parallel_lexical,
    })
}

pub fn post_ingest(opts: PostIngestOptions) -> Result<()> {
    let out = opts
        .out_parquet
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("data"));
    let tfidf_out_path = opts
        .tfidf_out
        .clone()
        .unwrap_or_else(|| out.join("derived").join("tfidf.index"));

    // -- Downstream builders --------------------------------------------
    // doc_table.bin is always built or updated — it's small, fast, and
    // required by every later index build plus most research tools.
    let doc_table_path = out.join("derived").join("doc_table.bin");
    println!("\n=== Updating doc table ===");
    if let Some(parent) = doc_table_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let append_to = doc_table_path.exists().then(|| doc_table_path.clone());
    crate::commands::document_table::build(
        opts.out_parquet.clone(),
        doc_table_path.clone(),
        append_to,
    )?;

    // catalog is fast and required by many commands — build it right after
    // doc_table and before the slow lexical indexes so it's always available.
    if let Some(ref catalog_out) = opts.catalog_index_out {
        println!("\n=== Building catalog index ===");
        crate::commands::catalog_index::build(
            opts.out_parquet.clone(),
            catalog_out.clone(),
            None,
            Some(doc_table_path.clone()),
        )?;
        println!("wrote {}", catalog_out.display());
    }

    let parquet_file_count = crate::phrase_index::parquet_files(&opts.out_parquet)
        .map(|v| v.len())
        .unwrap_or(0);

    let phrase_buckets =
        crate::memory::bucket_count_for_corpus(parquet_file_count, opts.phrase_max_memory);
    let tfidf_buckets = phrase_buckets;

    let run_phrase = || -> Result<()> {
        println!("\n=== Building phrase index ===");
        println!(
            "  buckets: {} (parquet files: {}, memory budget: {})",
            phrase_buckets,
            parquet_file_count,
            opts.phrase_max_memory
                .map(|b| format!("{} MB", b / 1024 / 1024))
                .unwrap_or_else(|| "default".into())
        );
        crate::phrase_index::build(
            opts.out_parquet.clone(),
            doc_table_path.clone(),
            opts.phrase_index_out.clone(),
            opts.phrase_gram_len,
            phrase_buckets,
            None,
        )?;
        println!("wrote {}", opts.phrase_index_out.display());
        Ok(())
    };

    let run_tfidf = || -> Result<()> {
        println!("\n=== Building TF-IDF index ===");
        let params = crate::tfidf::index::TfidfParams::default();
        crate::tfidf::index::build(
            opts.out_parquet.clone(),
            doc_table_path.clone(),
            tfidf_out_path.clone(),
            params,
            tfidf_buckets,
            None,
        )?;
        println!("wrote {}", tfidf_out_path.display());
        Ok(())
    };

    if opts.parallel_lexical && opts.build_phrase_index && opts.build_tfidf {
        println!("\n=== Building phrase + TF-IDF indexes in parallel ===");
        std::thread::scope(|s| -> Result<()> {
            let phrase_handle = s.spawn(run_phrase);
            let tfidf_handle = s.spawn(run_tfidf);
            let phrase_res = phrase_handle
                .join()
                .map_err(|_| anyhow!("phrase index thread panicked"))?;
            let tfidf_res = tfidf_handle
                .join()
                .map_err(|_| anyhow!("tfidf index thread panicked"))?;
            phrase_res?;
            tfidf_res?;
            Ok(())
        })?;
    } else {
        if opts.build_phrase_index {
            run_phrase()?;
        }
        if opts.build_tfidf {
            run_tfidf()?;
        }
    }

    initialize_registry_after_ingest(
        &out,
        &doc_table_path,
        opts.catalog_index_out.as_deref(),
        Some(&opts.phrase_index_out),
        Some(&tfidf_out_path),
    )?;

    let parquet_bytes = crate::commands::estimate::dir_size(&opts.out_parquet);
    print_next_steps(opts.build_phrase_index, opts.build_tfidf, parquet_bytes);
    Ok(())
}

pub(crate) fn initialize_registry_after_ingest(
    data_root: &Path,
    doc_table_path: &Path,
    catalog_index_path: Option<&Path>,
    phrase_index_path: Option<&Path>,
    tfidf_index_path: Option<&Path>,
) -> Result<()> {
    let registry_path = data_root.join("derived").join("registry.sqlite");
    crate::registry::init_registry(&registry_path)?;

    let Some(catalog_index_path) = catalog_index_path else {
        println!("initialized {}", registry_path.display());
        return Ok(());
    };
    if !doc_table_path.exists() || !catalog_index_path.exists() {
        println!("initialized {}", registry_path.display());
        return Ok(());
    }

    let doc_table = crate::document_table::DocumentTable::load(doc_table_path)?;
    let catalog = crate::catalog_index::CorpusCatalogIndex::load(catalog_index_path)?;

    let phrase_info = phrase_index_path
        .filter(|p| p.exists())
        .and_then(|p| crate::phrase_index::PhraseIndex::header_info(p).ok());
    let tfidf_info = tfidf_index_path
        .filter(|p| p.exists())
        .and_then(|p| crate::tfidf::index::TfidfIndex::header_info(p).ok());
    let phrase_for_db = phrase_info
        .as_ref()
        .and_then(|info| phrase_index_path.map(|path| (path, info)));
    let tfidf_for_db = tfidf_info
        .as_ref()
        .and_then(|info| tfidf_index_path.map(|path| (path, info)));

    crate::registry::populate_identity_from_pack(
        &registry_path,
        &doc_table,
        &catalog,
        phrase_for_db,
        tfidf_for_db,
        data_root,
    )?;
    println!("initialized {}", registry_path.display());
    Ok(())
}

/// Non-prominent footer after a successful ingest. Surfaces the optional
/// heavy indexes (phrase, tfidf) without making them look mandatory, and
/// points users back to `status` for state inspection.
fn print_next_steps(built_phrase: bool, built_tfidf: bool, parquet_bytes: u64) {
    use crate::commands::estimate::{phrase_index_estimate, tfidf_estimate};

    let need_phrase = !built_phrase;
    let need_tfidf = !built_tfidf;

    println!();
    println!("Ingest complete. The corpus is ready.");
    println!();
    if need_phrase || need_tfidf {
        println!("Optional indexes:");
        println!("  Build them when you need faster exact phrase lookup, source tracing,");
        println!("  similarity search, or frontier discovery.");
        println!();
        if need_phrase && need_tfidf {
            println!("  ./sinorag indexes lexical");
        } else if need_phrase {
            println!("  ./sinorag index phrase");
        } else if need_tfidf {
            println!("  ./sinorag index tfidf");
        }
        println!();
        if need_tfidf {
            println!("  tf-idf index: similarity search and frontier discovery");
            println!("    estimate: {}", tfidf_estimate(parquet_bytes));
        }
        if need_phrase {
            println!("  phrase index: exact CJK phrase lookup and source tracing");
            println!("    estimate: {}", phrase_index_estimate(parquet_bytes));
        }
        println!("  vector index: semantic discovery");
        println!("    build separately with ./sinorag indexes semantic, or external embeddings");
        println!();
    }
    println!("Check what's built:");
    println!("  ./sinorag status");
}

/// Move staging partitions into their final home. Refuses to overwrite an
/// existing same-named partition. When `append_jsonl` is true, appends the
/// staging JSONL to any existing JSONL instead of refusing.
fn promote_staging(
    staging_root: &Path,
    staging_parquet: &Path,
    staging_jsonl: &Path,
    out_jsonl: &Path,
    out_parquet: &Path,
    append_jsonl: bool,
) -> Result<()> {
    // Inventory which partitions the staging produced.
    let mut to_move: Vec<(PathBuf, PathBuf)> = Vec::new();
    if staging_parquet.is_dir() {
        for entry in std::fs::read_dir(staging_parquet)? {
            let entry = entry?;
            let name = entry.file_name();
            let s = name.to_string_lossy();
            if s.starts_with("source_corpus=") && entry.file_type()?.is_dir() {
                let target = out_parquet.join(&*s);
                if target.exists() {
                    anyhow::bail!(
                        "partition {} already exists at {}. Delete it or use a different --out-parquet \
                         to avoid silent overwrite.",
                        s, target.display()
                    );
                }
                to_move.push((entry.path(), target));
            }
        }
    }
    if !append_jsonl && out_jsonl.exists() {
        anyhow::bail!(
            "target jsonl already exists: {}. Move/delete first.",
            out_jsonl.display()
        );
    }

    std::fs::create_dir_all(out_parquet)?;
    for (src, dst) in &to_move {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(src, dst)
            .with_context(|| format!("rename {} → {}", src.display(), dst.display()))?;
    }
    if staging_jsonl.exists() {
        if let Some(parent) = out_jsonl.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if append_jsonl && out_jsonl.exists() {
            use std::io::copy;
            let mut src = std::fs::File::open(staging_jsonl)
                .with_context(|| format!("open staging jsonl {}", staging_jsonl.display()))?;
            let mut dst = std::fs::File::options()
                .append(true)
                .open(out_jsonl)
                .with_context(|| format!("open {} for append", out_jsonl.display()))?;
            copy(&mut src, &mut dst).with_context(|| {
                format!("append {} → {}", staging_jsonl.display(), out_jsonl.display())
            })?;
            let _ = std::fs::remove_file(staging_jsonl);
        } else {
            std::fs::rename(staging_jsonl, out_jsonl).with_context(|| {
                format!(
                    "rename {} → {}",
                    staging_jsonl.display(),
                    out_jsonl.display()
                )
            })?;
        }
    }
    // Sweep staging dir; if anything unexpected remains, leave it.
    let _ = std::fs::remove_file(staging_root.join(".ingest_checkpoint.json"));
    let _ = std::fs::remove_dir(staging_parquet);
    let _ = std::fs::remove_dir(staging_root);
    Ok(())
}

fn find_freshest_staging(out: &Path) -> Result<Option<PathBuf>> {
    let dir = out.join(".staging");
    if !dir.is_dir() {
        return Ok(None);
    }
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("ingest-") {
            continue;
        }
        let mtime = entry.metadata()?.modified()?;
        match &best {
            Some((m, _)) if *m >= mtime => {}
            _ => best = Some((mtime, entry.path())),
        }
    }
    Ok(best.map(|(_, p)| p))
}
