//! `sinorag merge-cbeta` — one-time three-way merge of the CBETA corpus.
//!
//! Combines up to three CBETA distributions into a single `xml-merged/`
//! directory ready for `sinorag ingest cbeta`:
//!
//!   --github  : GitHub xml-p5 (one file per work, largest content per canon)
//!   --iso     : ISO xml-iso   (one file per fascicle, different coverage)
//!   --extra   : tei-extra/    (CC, LC, TX, YP canons absent from ISO)
//!
//! Strategy per work: whichever source has the most bytes wins. Works that
//! exist in only one source are always included. The --extra source only
//! contributes works not already covered by GitHub or ISO.
//!
//! Output layout (compatible with `sinorag ingest cbeta <out>`):
//!   <out>/xml-merged/T/T01/T01n0001.xml         ← GitHub-sourced
//!   <out>/xml-merged/T/T01/T01n0001_001.xml      ← ISO-sourced (fascicle)
//!   <out>/xml-merged/CC/CC01/CC01n0001.xml       ← extra-sourced
//!   <out>/merge-manifest.json                    ← provenance record

use crate::tei;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Files that belong to one work_id from one corpus.
struct WorkFiles {
    files: Vec<(PathBuf, String, u64)>, // (abs_path, rel_path, file_size)
    total_bytes: u64,
    source: &'static str,
}

pub fn run(
    github: PathBuf,
    iso: PathBuf,
    extra: Option<PathBuf>,
    out: PathBuf,
    dry_run: bool,
) -> Result<()> {
    eprintln!("Scanning GitHub corpus...");
    let github_scan = tei::scan_cbeta_corpus(&github)?;
    eprintln!(
        "  {} files ({})",
        github_scan.files.len(),
        github_scan.distribution.as_str()
    );

    eprintln!("Scanning ISO corpus...");
    let iso_scan = tei::scan_cbeta_corpus(&iso)?;
    eprintln!(
        "  {} files ({})",
        iso_scan.files.len(),
        iso_scan.distribution.as_str()
    );

    let extra_scan = if let Some(ref extra_path) = extra {
        eprintln!("Scanning extra corpus...");
        let scan = tei::scan_cbeta_corpus(extra_path)?;
        eprintln!(
            "  {} files ({})",
            scan.files.len(),
            scan.distribution.as_str()
        );
        Some(scan)
    } else {
        None
    };

    let github_works = build_work_map(&github_scan.files, "github")?;
    let iso_works = build_work_map(&iso_scan.files, "iso")?;
    let extra_works = match &extra_scan {
        Some(scan) => build_work_map(&scan.files, "extra")?,
        None => BTreeMap::new(),
    };

    let out_xml = out.join("xml-merged");
    if !dry_run {
        std::fs::create_dir_all(&out_xml).context("creating xml-merged output directory")?;
    }

    // Collect all unique work_ids in deterministic order.
    let all_work_ids: BTreeMap<&String, ()> = github_works
        .keys()
        .chain(iso_works.keys())
        .chain(extra_works.keys())
        .map(|k| (k, ()))
        .collect();

    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut contested = 0usize;
    let mut total_files_written = 0usize;

    for work_id in all_work_ids.keys() {
        // Gather candidates from all sources, pick the one with most bytes.
        let mut candidates: Vec<&WorkFiles> = Vec::new();
        if let Some(w) = github_works.get(*work_id) {
            candidates.push(w);
        }
        if let Some(w) = iso_works.get(*work_id) {
            candidates.push(w);
        }
        if let Some(w) = extra_works.get(*work_id) {
            candidates.push(w);
        }

        if candidates.len() > 1 {
            contested += 1;
        }

        let winner = candidates
            .iter()
            .max_by_key(|w| w.total_bytes)
            .expect("at least one candidate per work_id");

        *counts.entry(winner.source).or_insert(0) += 1;

        for (abs_path, rel_path, _) in &winner.files {
            let dest = out_xml.join(rel_path);
            if !dry_run {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("create dir for {}", dest.display()))?;
                }
                std::fs::copy(abs_path, &dest).with_context(|| {
                    format!("copy {} → {}", abs_path.display(), dest.display())
                })?;
            }
            total_files_written += 1;
        }
    }

    let total_works = all_work_ids.len();
    eprintln!("\nMerge summary:");
    eprintln!("  Total works:           {total_works}");
    for (source, count) in &counts {
        eprintln!("  Won by {source:<16} {count}");
    }
    eprintln!("  Contested (>1 source): {contested}");
    eprintln!("  Total XML files:       {total_files_written}");

    // Coverage check against the embedded work catalog.
    let catalog = crate::cbeta_sidecar::work_catalog();
    let merged_ids: std::collections::BTreeSet<&String> =
        all_work_ids.keys().copied().collect();
    let mut missing: Vec<&str> = Vec::new();
    for cat_id in catalog.work_ids() {
        if !merged_ids.contains(cat_id) {
            missing.push(cat_id);
        }
    }
    missing.sort();
    if missing.is_empty() {
        eprintln!("\nCoverage: all {} catalog works are present.", catalog.len());
    } else {
        eprintln!(
            "\nCoverage: {}/{} catalog works present ({} missing).",
            catalog.len() - missing.len(),
            catalog.len(),
            missing.len()
        );
        let preview = if missing.len() > 20 { 20 } else { missing.len() };
        for id in &missing[..preview] {
            let label = catalog
                .get(id)
                .map(|e| e.title.as_str())
                .unwrap_or("?");
            eprintln!("    missing: {id}  {label}");
        }
        if missing.len() > 20 {
            eprintln!("    ... and {} more", missing.len() - 20);
        }
    }

    if dry_run {
        eprintln!("\n(dry run — no files written)");
        return Ok(());
    }

    write_manifest(
        &out,
        &github,
        &iso,
        extra.as_deref(),
        &counts,
        contested,
        total_files_written,
    )?;

    eprintln!("\nOutput: {}", out_xml.display());
    eprintln!("Next step:");
    eprintln!("  sinorag ingest cbeta {}", out.display());
    Ok(())
}

/// Build a map from work_id → WorkFiles by grouping files that share the same
/// stem after fascicle-suffix stripping (e.g. T01n0001_001 → T01n0001).
fn build_work_map(
    files: &[(PathBuf, String)],
    source: &'static str,
) -> Result<BTreeMap<String, WorkFiles>> {
    let mut map: BTreeMap<String, WorkFiles> = BTreeMap::new();

    for (abs_path, rel_path) in files {
        let stem = Path::new(rel_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let work_id = tei::strip_fascicle_suffix(stem).to_string();

        let size = std::fs::metadata(abs_path)
            .map(|m| m.len())
            .unwrap_or(0);

        let entry = map.entry(work_id).or_insert_with(|| WorkFiles {
            files: Vec::new(),
            total_bytes: 0,
            source,
        });
        entry.files.push((abs_path.clone(), rel_path.clone(), size));
        entry.total_bytes += size;
    }

    Ok(map)
}

fn write_manifest(
    out: &Path,
    github: &Path,
    iso: &Path,
    extra: Option<&Path>,
    counts: &BTreeMap<&str, usize>,
    contested: usize,
    total_files: usize,
) -> Result<()> {
    let mut sources = serde_json::json!({
        "github": github.to_string_lossy(),
        "iso": iso.to_string_lossy(),
    });
    if let Some(extra_path) = extra {
        sources["extra"] = serde_json::Value::String(extra_path.to_string_lossy().into_owned());
    }

    let won_by: serde_json::Map<String, serde_json::Value> = counts
        .iter()
        .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
        .collect();

    let manifest = serde_json::json!({
        "schema": "sinorag-merge-manifest-v1",
        "created_at": chrono::Utc::now().to_rfc3339(),
        "sources": sources,
        "stats": {
            "won_by": won_by,
            "contested": contested,
            "total_xml_files": total_files,
        }
    });
    let path = out.join("merge-manifest.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest)?)
        .with_context(|| format!("write manifest to {}", path.display()))?;
    eprintln!("Wrote {}", path.display());
    Ok(())
}
