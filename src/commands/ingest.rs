use crate::models::PassageRecord;
use crate::{ingest, storage, tei};
use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub async fn run(
    corpus: Option<PathBuf>,
    kanripo_input: Option<PathBuf>,
    sorting_data_dir: Option<PathBuf>,
    out: Option<PathBuf>,
    build_phrase_index: bool,
    phrase_index_out: PathBuf,
    phrase_gram_len: usize,
    build_tfidf: bool,
    tfidf_out: Option<PathBuf>,
    catalog_index_out: Option<PathBuf>,
    phrase_max_memory: Option<u64>,
) -> Result<()> {
    let out = out.unwrap_or_else(|| PathBuf::from("GraphDiscovery/Runs/rust"));
    
    if corpus.is_none() && kanripo_input.is_none() {
        anyhow::bail!("ingest requires at least one of --corpus or --kanripo-input");
    }

    let out_jsonl = out.join("passages.jsonl");
    let out_parquet = out.join("passages.parquet");
    let zen_only = false;

    if let Some(parent) = out_jsonl.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Check for resume checkpoint
    let checkpoint_path = out_parquet.join(".ingest_checkpoint.json");
    let mut processed_files: HashSet<String> = HashSet::new();
    let mut resume_mode = false;

    if checkpoint_path.exists() {
        println!("Found checkpoint file, attempting resume...");
        if let Ok(checkpoint_content) = std::fs::read_to_string(&checkpoint_path) {
            if let Ok(checkpoint) = serde_json::from_str::<serde_json::Value>(&checkpoint_content) {
                if let Some(files) = checkpoint.get("processed_files").and_then(|v| v.as_array()) {
                    for file in files {
                        if let Some(f) = file.as_str() {
                            processed_files.insert(f.to_string());
                        }
                    }
                    resume_mode = true;
                    println!("Resuming from checkpoint, {} files already processed", processed_files.len());
                }
            }
        }
    }

    // If not resuming, reset the parquet directory
    if !resume_mode {
        storage::reset_parquet_dir(&out_parquet)?;
    }

    let file = if resume_mode {
        // Append to existing JSONL
        File::options()
            .append(true)
            .open(&out_jsonl)?
    } else {
        // Create new JSONL
        File::create(&out_jsonl)?
    };
    let mut jsonl = BufWriter::new(file);
    
    // Separate batches per source_corpus for partitioned writing
    let mut cbeta_batch = storage::PassageBatch::default();
    let mut kanripo_batch = storage::PassageBatch::default();
    let mut cbeta_part_index = 0usize;
    let mut kanripo_part_index = 0usize;
    let mut total = 0usize;
    let mut cbeta_count = 0usize;
    let mut kanripo_count = 0usize;

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
            storage::write_parquet_part_partitioned(batch, &out_parquet, corpus_name, *part_index)?;
            batch.clear();
            *part_index += 1;
        }
        Ok(())
    };

    let save_checkpoint = |processed: &HashSet<String>, cbeta_c: usize, kanripo_c: usize, tot: usize| -> Result<()> {
        let checkpoint = serde_json::json!({
            "processed_files": processed,
            "cbeta_count": cbeta_c,
            "kanripo_count": kanripo_c,
            "total": tot,
        });
        std::fs::write(&checkpoint_path, checkpoint.to_string())?;
        Ok(())
    };

    if let Some(corpus_root) = corpus.as_ref() {
        let metadata = tei::load_buddhist_metadata(corpus_root, sorting_data_dir.as_deref())?;
        let paths = tei::iter_xml_paths(corpus_root)?;
        let total_files = paths.len();
        let pb = ProgressBar::new(total_files as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("#>-"),
        );
        pb.set_message("Processing CBETA XML files");

        for (xml_path, rel_path) in paths {
            // Skip if already processed in resume mode
            if resume_mode && processed_files.contains(&rel_path) {
                pb.inc(1);
                continue;
            }

            let meta = metadata.get(&rel_path).cloned().unwrap_or_default();
            if zen_only && !meta.traditions.iter().any(|t| t == "Chan/Zen") {
                pb.inc(1);
                continue;
            }

            let file_passage_count = tei::extract_passages_from_file(&xml_path, &rel_path, &meta)?
                .into_iter()
                .inspect(|_| {
                    cbeta_count += 1;
                    total += 1;
                })
                .collect::<Vec<_>>();

            for passage in file_passage_count {
                emit(&passage, &mut cbeta_batch, &mut cbeta_part_index, &mut jsonl, "cbeta")?;
            }

            // Mark as processed and save checkpoint
            processed_files.insert(rel_path.clone());
            if processed_files.len() % 100 == 0 {
                save_checkpoint(&processed_files, cbeta_count, kanripo_count, total)?;
            }

            pb.inc(1);
        }
        pb.finish_with_message("CBETA ingest complete");
    }

    if let Some(kanripo_root) = kanripo_input.as_ref() {
        // For Kanripo, we need to count sections first for progress bar
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
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("#>-"),
        );
        pb.set_message("Processing Kanripo sections");

        let mut processed_sections = 0usize;
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
                    emit(&passage, &mut kanripo_batch, &mut kanripo_part_index, &mut jsonl, "kanripo")?;
                    kanripo_count += 1;
                    total += 1;
                }
                processed_sections += 1;
                pb.inc(1);
                if kanripo_count % 100 == 0 {
                    let msg = format!("Kanripo passages: {}", kanripo_count);
                    pb.set_message(msg);
                }
            }
        }

        pb.finish_with_message("Kanripo ingest complete");
    }

    if !cbeta_batch.is_empty() {
        storage::write_parquet_part_partitioned(&cbeta_batch, &out_parquet, "cbeta", cbeta_part_index)?;
    }
    if !kanripo_batch.is_empty() {
        storage::write_parquet_part_partitioned(&kanripo_batch, &out_parquet, "kanripo", kanripo_part_index)?;
    }
    jsonl.flush()?;

    // Save final checkpoint
    save_checkpoint(&processed_files, cbeta_count, kanripo_count, total)?;

    // Clean up checkpoint file on successful completion
    if checkpoint_path.exists() {
        std::fs::remove_file(&checkpoint_path)?;
    }

    println!("wrote {}", out_jsonl.display());
    println!("wrote {}/", out_parquet.display());
    println!("passages {total}");
    println!("  cbeta {cbeta_count}");
    println!("  kanripo {kanripo_count}");

    // The phrase index and TF-IDF builders both consume the doc_table. Build it
    // first if either index is requested.
    let doc_table_path = out.join("doc_table.bin");
    if (build_phrase_index || build_tfidf) && !doc_table_path.exists() {
        println!("\n=== Building doc table ===");
        crate::commands::document_table::build(out_parquet.clone(), doc_table_path.clone(), None)?;
    }

    if build_phrase_index {
        println!("\n=== Building phrase index ===");
        let _ = phrase_max_memory; // reserved for future tuning of bucket count
        crate::phrase_index::build(
            out_parquet.clone(),
            doc_table_path.clone(),
            phrase_index_out.clone(),
            phrase_gram_len,
            2048,
            None,
        )?;
        println!("wrote {}", phrase_index_out.display());
    }

    if build_tfidf {
        println!("\n=== Building TF-IDF index ===");
        let tfidf_out_path = tfidf_out.unwrap_or_else(|| out.join("tfidf.index"));
        let params = crate::tfidf::index::TfidfParams::default_v2();
        crate::tfidf::index::build(
            out_parquet.clone(),
            doc_table_path.clone(),
            tfidf_out_path.clone(),
            params,
            2048,
            None,
        )?;
        println!("wrote {}", tfidf_out_path.display());
    }

    // Build catalog index if requested
    if let Some(catalog_index_out) = catalog_index_out {
        println!("\n=== Building catalog index ===");
        crate::commands::catalog_index::build(
            out_parquet.clone(),
            catalog_index_out.clone(),
            None,  // debug_json
            None,  // doc_table - will use default location
        )?;
    }

    Ok(())
}
