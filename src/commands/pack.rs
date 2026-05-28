//! `sinorag pack-create` — build a distribution .7z file for GitHub Releases.
//!
//! Packs these directories from data_root into a single LZMA2 .7z file:
//!   passages.parquet/       (from passages-raw.parquet/ on disk)
//!   dict.parquet/
//!   persons.parquet/
//!   places.parquet/
//!
//! The on-disk source for passages is `passages-raw.parquet/` (uncompressed,
//! produced by `ingest --no-parquet-compression`), stored in the archive as
//! `passages.parquet/` so `sinorag init` extracts to the expected location.
//!
//! Requires `7z` (7-Zip) to be installed and on PATH.
//! Recommended flags used: lzma2, level 9, multithreaded, 512 MB dictionary.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Command;

struct DatasetSpec {
    /// Directory name on disk inside data_root.
    source_dir: &'static str,
    /// Name as it should appear inside the archive.
    archive_name: &'static str,
    optional: bool,
}

const DATASETS: &[DatasetSpec] = &[
    DatasetSpec {
        source_dir: "passages-raw.parquet",
        archive_name: "passages.parquet",
        optional: false,
    },
    DatasetSpec {
        source_dir: "dict.parquet",
        archive_name: "dict.parquet",
        optional: false,
    },
    DatasetSpec {
        source_dir: "persons.parquet",
        archive_name: "persons.parquet",
        optional: true,
    },
    DatasetSpec {
        source_dir: "places.parquet",
        archive_name: "places.parquet",
        optional: true,
    },
];

pub fn run(data_root: PathBuf, out: PathBuf) -> Result<()> {
    // Verify 7z is available.
    let seven_z = find_7z().context(
        "7z not found on PATH. Install p7zip-full (Linux) or 7-Zip (Windows/macOS) \
         and ensure the binary is on your PATH.",
    )?;

    // Validate sources and summarise what will be packed.
    let mut present: Vec<&DatasetSpec> = Vec::new();
    for spec in DATASETS {
        let src = data_root.join(spec.source_dir);
        if src.exists() {
            let (n, b) = scan_directory(&src)?;
            eprintln!(
                "  {} → {} — {n} files, {:.1} MiB",
                spec.source_dir,
                spec.archive_name,
                b as f64 / (1024.0 * 1024.0)
            );
            present.push(spec);
        } else if spec.optional {
            eprintln!(
                "  Warning: {} not found — skipping",
                src.display()
            );
        } else {
            bail!(
                "Required dataset not found: {}\n\
                 Run `sinorag ingest` (with --no-parquet-compression) to produce it.",
                src.display()
            );
        }
    }
    eprintln!();

    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating output directory {}", parent.display()))?;
        }
    }

    // Build a staging directory where each dataset lives under its archive name.
    // We use symlinks to avoid copying gigabytes of data.
    let staging = data_root.join(".pack-staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging).context("removing old staging directory")?;
    }
    std::fs::create_dir_all(&staging).context("creating staging directory")?;

    for spec in &present {
        let src = data_root.join(spec.source_dir);
        let link = staging.join(spec.archive_name);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&src, &link)
            .with_context(|| format!("symlinking {} → {}", link.display(), src.display()))?;
        #[cfg(not(unix))]
        {
            // On Windows, use junction points or just copy small dirs.
            // For simplicity, fall back to copying (passages-raw will be large).
            copy_dir_all(&src, &link)?;
        }
    }

    // Run 7z: add all entries from the staging directory.
    //   -t7z        7z container format
    //   -m0=lzma2   LZMA2 codec
    //   -mx=9       maximum compression effort
    //   -mmt=on     multithreaded
    //   -md=512m    512 MB dictionary (key for long-range repetition in corpus)
    //   -ms=on      solid archive
    eprintln!("Running 7z (this may take several minutes)…");

    let mut args = vec![
        "a".to_string(),
        "-t7z".to_string(),
        "-m0=lzma2".to_string(),
        "-mx=9".to_string(),
        "-mmt=on".to_string(),
        "-md=512m".to_string(),
        "-ms=on".to_string(),
        out.to_str()
            .context("non-UTF-8 output path")?
            .to_string(),
    ];
    // Add each staged entry by name (not the staging dir itself, so 7z
    // stores `passages.parquet/…` not `.pack-staging/passages.parquet/…`).
    for spec in &present {
        args.push(
            staging
                .join(spec.archive_name)
                .to_str()
                .context("non-UTF-8 path")?
                .to_string(),
        );
    }

    let status = Command::new(&seven_z)
        .args(&args)
        .status()
        .with_context(|| format!("failed to invoke {seven_z}"))?;

    // Clean up staging symlinks regardless of outcome.
    let _ = std::fs::remove_dir_all(&staging);

    if !status.success() {
        bail!("7z exited with {status}");
    }

    let arc_size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "Done. Archive: {} ({:.1} MiB)",
        out.display(),
        arc_size as f64 / (1024.0 * 1024.0)
    );
    Ok(())
}

/// Find the 7z binary: tries `7zz`, `7z`, `7za` in order.
fn find_7z() -> Result<String> {
    for name in &["7zz", "7z", "7za"] {
        if Command::new(name)
            .arg("i")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
        {
            return Ok(name.to_string());
        }
    }
    bail!("none of 7zz / 7z / 7za found on PATH");
}

fn scan_directory(dir: &std::path::Path) -> Result<(u64, u64)> {
    let mut count = 0u64;
    let mut bytes = 0u64;
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry.with_context(|| format!("scanning {}", dir.display()))?;
        if entry.file_type().is_file() {
            bytes += entry.metadata()?.len();
            count += 1;
        }
    }
    Ok((count, bytes))
}

#[cfg(not(unix))]
fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in walkdir::WalkDir::new(src) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(src).unwrap();
        let dest = dst.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dest)?;
        } else {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}
