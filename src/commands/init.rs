//! `sinorag init` — bootstrap the CBETA corpus from a pre-built pack.
//!
//! Default path (once a pack is published):
//!   1. curl downloads cbeta-pack.7z from GitHub Releases
//!   2. sevenz-rust decompresses the .7z in-process (pure Rust LZMA2)
//!   3. passages.parquet/, dict.parquet/, persons.parquet/, places.parquet/
//!      are extracted into the data root
//!   4. doc_table and catalog are built locally (fast)
//!
//! Phrase and TF-IDF indexes are intentionally excluded from the pack —
//! they are 3-10× larger than the parquet and take hours to build.
//! Build them separately with `sinorag indexes lexical`.
//!
//! Alternative: `sinorag init --from-raw <PATH>` ingests from a local
//! CBETA corpus directory (GitHub xml-p5 or ISO xml-iso layout) and
//! produces an identical result without downloading anything.

use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Canonical pack URL for the current release.
///
/// Set to `None` until the first pack is built and uploaded to GitHub Releases.
/// When ready, replace with the direct download URL to `cbeta-pack.7z`.
///
const PACK_URL: Option<&str> = Some("https://github.com/LegeApp/SinoRAG/releases/download/corpus-release-1/cbeta-pack.7z");

const PACK_FILENAME: &str = "cbeta-pack.7z";

pub async fn run(
    url_override: Option<String>,
    force: bool,
    from_raw: Option<PathBuf>,
    out_parquet: PathBuf,
) -> Result<()> {
    let data_root = out_parquet
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("data"));

    let cbeta_partition = out_parquet.join("source_corpus=cbeta");

    if cbeta_partition.exists() && !force {
        eprintln!(
            "CBETA corpus is already present at {}.",
            cbeta_partition.display()
        );
        eprintln!(
            "Run `sinorag status` to inspect it, or use --force to re-initialize."
        );
        eprintln!(
            "To rebuild from a local CBETA source instead: sinorag init --from-raw <PATH>"
        );
        return Ok(());
    }

    // --from-raw: bypass download and ingest from local corpus files.
    if let Some(raw_path) = from_raw {
        return ingest_from_raw(raw_path, out_parquet, data_root).await;
    }

    // -- Download ----------------------------------------------------------
    let url = url_override
        .as_deref()
        .or(PACK_URL)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No official CBETA pack is published yet for this release.\n\
                 \n\
                 To ingest from your own CBETA source files:\n\
                 \n  \
                   sinorag init --from-raw /path/to/cbeta\n\
                 \n\
                 This accepts both the GitHub xml-p5 layout (one file per work)\n\
                 and the ISO xml-iso layout (one file per fascicle).\n\
                 \n\
                 To use a custom pack URL or local file path:\n\
                 \n  \
                   sinorag init --url file:///path/to/cbeta-pack.7z\n\
                   sinorag init --url https://example.com/cbeta-pack.7z"
            )
        })?;

    // Download into a subdirectory so we stay on the same filesystem as data/.
    let tmp_dir = data_root.join(".init-download");
    std::fs::create_dir_all(&tmp_dir).context("creating temporary download directory")?;
    let arc_path = tmp_dir.join(PACK_FILENAME);

    eprintln!("Downloading pack from {}", url);

    // download_with_curl, extract_7z, and build_local_indexes are all synchronous
    // and can block for minutes on large packs — run them on a dedicated blocking thread
    // so the Tokio runtime stays responsive.
    let url_owned = url.to_string();
    tokio::task::spawn_blocking(move || {
        download_with_curl(&url_owned, &arc_path)?;
        eprintln!("\nExtracting pack...");
        extract_7z(&arc_path, &data_root)?;
        let _ = std::fs::remove_file(&arc_path);
        let _ = std::fs::remove_dir(&tmp_dir);
        build_local_indexes(&data_root, &out_parquet)
    })
    .await
    .context("init blocking task panicked")?
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

fn download_with_curl(url: &str, dest: &Path) -> Result<()> {
    let status = Command::new("curl")
        .args([
            "-L",
            "--progress-bar",
            "--fail",
            "-o",
            dest.to_str().context("non-UTF-8 destination path")?,
            url,
        ])
        .status()
        .context(
            "failed to invoke curl — ensure curl is installed and on PATH.\n\
             On Windows it ships with the OS since version 1803.",
        )?;

    if !status.success() {
        bail!("curl exited with {status}; check the URL and your network connection.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

/// Extract a .7z archive into `data_root` using sevenz-rust (pure Rust LZMA2).
///
/// After extraction, `passages-raw.parquet/` is renamed to `passages.parquet/`
/// if present — this handles archives built from the raw (uncompressed) source
/// directory before the canonical name was established.
fn extract_7z(arc_path: &Path, data_root: &Path) -> Result<()> {
    std::fs::create_dir_all(data_root)
        .with_context(|| format!("creating {}", data_root.display()))?;

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} {elapsed_precise} {msg}")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
    );
    pb.set_message("Decompressing (LZMA2)…");
    pb.enable_steady_tick(Duration::from_millis(80));

    sevenz_rust::decompress_file(arc_path, data_root)
        .map_err(|e| anyhow::anyhow!("failed to extract {}: {}", arc_path.display(), e))?;

    pb.finish_and_clear();

    // Rename passages-raw.parquet → passages.parquet if the archive used
    // the raw source directory name instead of the canonical pack name.
    let raw = data_root.join("passages-raw.parquet");
    let canonical = data_root.join("passages.parquet");
    if raw.exists() && !canonical.exists() {
        std::fs::rename(&raw, &canonical)
            .context("renaming passages-raw.parquet to passages.parquet")?;
        eprintln!("Renamed passages-raw.parquet → passages.parquet");
    }

    let file_count = walkdir::WalkDir::new(data_root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .count();
    eprintln!("Extracted {file_count} files.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Local index build (doc_table + catalog; phrase/tfidf left for user)
// ---------------------------------------------------------------------------

fn build_local_indexes(data_root: &Path, out_parquet: &Path) -> Result<()> {
    let doc_table_path = data_root.join("derived").join("doc_table.bin");
    let catalog_path = data_root.join("derived").join("catalog.index");

    if let Some(parent) = doc_table_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    eprintln!("\n=== Building doc_table ===");
    crate::commands::document_table::build(
        out_parquet.to_path_buf(),
        doc_table_path.clone(),
        None,
    )?;

    eprintln!("\n=== Building catalog index ===");
    crate::commands::catalog_index::build(
        out_parquet.to_path_buf(),
        catalog_path.clone(),
        None,
        Some(doc_table_path.clone()),
    )?;

    crate::commands::ingest::initialize_registry_after_ingest(
        data_root,
        &doc_table_path,
        Some(&catalog_path),
        None,
        None,
    )?;

    eprintln!("\nCorpus ready.");
    eprintln!("  Check state:          sinorag status");
    eprintln!("  Build lexical indexes (optional, slow, several GB):");
    eprintln!("                        sinorag indexes lexical");
    eprintln!("  Add a CEF corpus:     sinorag ingest cef <path>");
    Ok(())
}

// ---------------------------------------------------------------------------
// --from-raw: ingest from local CBETA source files
// ---------------------------------------------------------------------------

async fn ingest_from_raw(
    corpus_path: PathBuf,
    out_parquet: PathBuf,
    data_root: PathBuf,
) -> Result<()> {
    let out_jsonl = data_root.join("passages.jsonl");
    let catalog_path = data_root.join("derived").join("catalog.index");
    let phrase_path = data_root.join("derived").join("phrase.index");
    let tfidf_path = data_root.join("derived").join("tfidf.index");

    eprintln!(
        "Ingesting from local CBETA corpus at {}",
        corpus_path.display()
    );
    eprintln!("(phrase and TF-IDF indexes will NOT be built automatically)");

    crate::commands::ingest::run(
        Some(corpus_path),
        None,           // no kanripo
        out_jsonl,
        out_parquet,
        false,          // zen_only
        None,           // resume
        false,          // append (fresh init)
        false,          // build_phrase_index
        phrase_path,
        4,              // phrase_gram_len default
        false,          // build_tfidf
        Some(tfidf_path),
        Some(catalog_path),
        None,           // phrase_max_memory
        false,          // parallel_lexical
        crate::storage::ParquetCompression::Zstd,
    )
    .await
}
