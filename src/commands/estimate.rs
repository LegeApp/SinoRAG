//! Cost estimates for the optional heavy indexes.
//!
//! Output is intentionally a range, not a precise figure — actual cost
//! varies with hardware (disk IO and core count dominate). The ratios
//! below are rules of thumb from observed runs on CJK n-gram (gram_len=4)
//! and TF-IDF (5–8 grams) on consumer SSDs. Calibrate by editing the
//! `EstimateRatios` constants if your hardware deviates significantly.

use std::fs;
use std::path::Path;

/// Per-index scaling factors. All ratios are relative to the on-disk
/// parquet bytes of the corpus.
struct EstimateRatios {
    /// Output index size, as a fraction of parquet bytes (low/high).
    disk_lo: f64,
    disk_hi: f64,
    /// Build-time minutes per GB of parquet (low/high).
    minutes_per_gb_lo: f64,
    minutes_per_gb_hi: f64,
}

const PHRASE: EstimateRatios = EstimateRatios {
    disk_lo: 0.5,
    disk_hi: 1.0,
    minutes_per_gb_lo: 2.0,
    minutes_per_gb_hi: 8.0,
};

const TFIDF: EstimateRatios = EstimateRatios {
    disk_lo: 0.15,
    disk_hi: 0.35,
    minutes_per_gb_lo: 1.0,
    minutes_per_gb_hi: 4.0,
};

/// One human-readable estimate string ("~1.2–2.4 GB disk, ~12–48 min build").
pub fn phrase_index_estimate(parquet_bytes: u64) -> String {
    render(parquet_bytes, &PHRASE)
}

pub fn tfidf_estimate(parquet_bytes: u64) -> String {
    render(parquet_bytes, &TFIDF)
}

fn render(parquet_bytes: u64, r: &EstimateRatios) -> String {
    if parquet_bytes == 0 {
        return "size unknown — ingest first".into();
    }
    let pb = parquet_bytes as f64;
    let gb = pb / (1024.0 * 1024.0 * 1024.0);

    let disk_lo = (pb * r.disk_lo) as u64;
    let disk_hi = (pb * r.disk_hi) as u64;
    let min_lo  = (gb * r.minutes_per_gb_lo).max(1.0);
    let min_hi  = (gb * r.minutes_per_gb_hi).max(1.0);

    format!(
        "~{}–{} disk, ~{} build",
        human_bytes(disk_lo),
        human_bytes(disk_hi),
        human_minutes(min_lo, min_hi),
    )
}

/// Recursively measure on-disk size of a parquet root (sum of all files).
pub fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = fs::metadata(path) else { return 0; };
    if meta.is_file() {
        return meta.len();
    }
    let mut total = 0u64;
    if let Ok(read) = fs::read_dir(path) {
        for entry in read.flatten() {
            total += dir_size(&entry.path());
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
    } else if v >= 10.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn human_minutes(lo: f64, hi: f64) -> String {
    // Promote to hours once the upper bound exceeds 90 min.
    if hi >= 90.0 {
        let lo_h = lo / 60.0;
        let hi_h = hi / 60.0;
        if hi_h >= 10.0 {
            format!("{lo_h:.0}–{hi_h:.0} h")
        } else {
            format!("{lo_h:.1}–{hi_h:.1} h")
        }
    } else {
        format!("{lo:.0}–{hi:.0} min")
    }
}
