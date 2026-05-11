//! `sinoragd status` — report what's built under the data root.
//!
//! Intentionally cheap: no parquet row scans, just filesystem inspection.
//! Designed so first-time users (or agents) can answer "what do I have,
//! what's missing, and what should I run next?" in a single command.

use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub fn run(data: PathBuf) -> Result<()> {
    let parquet_root  = data.join("passages.parquet");
    let derived       = data.join("derived");
    let doc_table     = derived.join("doc_table.bin");
    let catalog_index = derived.join("catalog.index");
    let phrase_index  = derived.join("phrase_v2.index");
    let tfidf_index   = derived.join("tfidf.index");
    let registry      = derived.join("registry.sqlite");

    println!("SinoRAGD status — data root: {}", data.display());
    println!();

    // --- Corpora --------------------------------------------------------
    println!("Corpus:");
    let corpora = list_partitions(&parquet_root);
    if corpora.is_empty() {
        println!("  (none ingested — run `sinoragd ingest <source> <path>`)");
    } else {
        for (corpus, files) in &corpora {
            println!("  • {corpus:<10} {files} partition file(s)");
        }
    }
    println!();

    // --- Indexes --------------------------------------------------------
    println!("Indexes:");
    report("doc_table",     &doc_table,     "base");
    report("catalog.index", &catalog_index, "base");
    report("phrase_v2.index", &phrase_index, "optional — exact phrase search");
    report("tfidf.index",   &tfidf_index,   "optional — similarity / frontier");
    println!();

    // --- Registry -------------------------------------------------------
    println!("Registry:");
    if registry.exists() {
        let size = fs::metadata(&registry).map(|m| m.len()).unwrap_or(0);
        println!("  • registry.sqlite present ({})", human_bytes(size));
        println!("    (small/empty is normal until research runs accumulate)");
    } else {
        println!("  • registry.sqlite not yet created — created on first research run");
    }
    println!();

    // --- Next steps -----------------------------------------------------
    println!("Suggested next:");
    if corpora.is_empty() {
        println!("  1. Ingest a corpus: `sinoragd ingest cbeta <PATH>`");
        return Ok(());
    }
    let parquet_bytes = super::estimate::dir_size(&parquet_root);
    let mut shown_any = false;
    if !phrase_index.exists() {
        println!(
            "  • Build phrase index (optional): `sinoragd index phrase`\n    estimate: {}",
            super::estimate::phrase_index_estimate(parquet_bytes)
        );
        shown_any = true;
    }
    if !tfidf_index.exists() {
        println!(
            "  • Build tf-idf index (optional):  `sinoragd index tfidf`\n    estimate: {}",
            super::estimate::tfidf_estimate(parquet_bytes)
        );
        shown_any = true;
    }
    println!("  • Start MCP server: `sinoragd mcp`");
    if !shown_any {
        println!("  (all heavy indexes already built — you're ready.)");
    }

    Ok(())
}

fn list_partitions(parquet_root: &Path) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    let Ok(read) = fs::read_dir(parquet_root) else { return out; };
    for entry in read.flatten() {
        let name = entry.file_name();
        let s = name.to_string_lossy();
        if let Some(corpus) = s.strip_prefix("source_corpus=") {
            let count = fs::read_dir(entry.path())
                .map(|r| r.flatten()
                          .filter(|e| e.path().extension()
                                          .and_then(|x| x.to_str()) == Some("parquet"))
                          .count())
                .unwrap_or(0);
            out.push((corpus.to_string(), count));
        }
    }
    out.sort();
    out
}

fn report(label: &str, path: &Path, note: &str) {
    if path.exists() {
        let size = path_size(path);
        println!("  ✓ {label:<16} {:>10}   ({note})", human_bytes(size));
    } else {
        println!("  ✗ {label:<16} {:>10}   ({note})", "missing");
    }
}

fn path_size(path: &Path) -> u64 {
    let Ok(meta) = fs::metadata(path) else { return 0; };
    if meta.is_file() {
        return meta.len();
    }
    let mut total = 0u64;
    if let Ok(read) = fs::read_dir(path) {
        for entry in read.flatten() {
            total += path_size(&entry.path());
        }
    }
    total
}

fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}
