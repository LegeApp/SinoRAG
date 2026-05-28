//! Ingest terebess.hu Zen biography pages (SingleFile-saved HTML) into the
//! corpus parquet. Filters 403/404 placeholders, strips site chrome, extracts
//! body text and the largest non-icon embedded image (written to disk).

use crate::models::PassageRecord;
use crate::storage::{write_parquet_part_partitioned, ParquetCompression, PassageBatch, PARQUET_BATCH_SIZE};
use anyhow::{anyhow, Context, Result};
use base64::Engine;
use rayon::prelude::*;
use regex::Regex;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub fn run(
    input: PathBuf,
    out_parquet: PathBuf,
    images_dir: PathBuf,
    min_body_chars: usize,
) -> Result<()> {
    if !input.is_dir() {
        return Err(anyhow!("input must be a directory: {}", input.display()));
    }
    fs::create_dir_all(&images_dir)?;

    let mut html_files: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(&input)
        .max_depth(1)
        .into_iter()
        .filter_map(Result::ok)
    {
        let p = entry.path();
        if p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("html") {
            html_files.push(p.to_path_buf());
        }
    }
    html_files.sort();
    eprintln!("=== ingest-terebess ===");
    eprintln!("Input dir : {}", input.display());
    eprintln!("Output    : {}", out_parquet.display());
    eprintln!("Images    : {}", images_dir.display());
    eprintln!("HTML files: {}", html_files.len());

    let records: Vec<PassageRecord> = {
        let images_dir = images_dir.clone();
        let extracted: Mutex<Vec<(PassageRecord, Option<String>)>> = Mutex::new(Vec::new());
        let skipped = std::sync::atomic::AtomicUsize::new(0);
        let processed = std::sync::atomic::AtomicUsize::new(0);

        html_files.par_iter().for_each(|p| {
            match extract_page(p, &images_dir, min_body_chars) {
                Ok(Some(parts)) => {
                    extracted.lock().unwrap().push(parts);
                }
                Ok(None) => {
                    skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Err(e) => {
                    eprintln!(
                        "  ! {}: {}",
                        p.file_name().and_then(|s| s.to_str()).unwrap_or(""),
                        e
                    );
                    skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
            let n = processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if n % 50 == 0 {
                eprintln!("  {}/{}", n, html_files.len());
            }
        });

        let final_extracted = extracted.into_inner().unwrap();
        eprintln!(
            "Extracted {} pages, skipped {}",
            final_extracted.len(),
            skipped.load(std::sync::atomic::Ordering::Relaxed)
        );

        // Carry the metadata_json alongside each record for the partitioned write.
        // We'll repack with metadata below.
        let mut out_records: Vec<PassageRecord> = Vec::with_capacity(final_extracted.len());
        let mut metas: Vec<Option<String>> = Vec::with_capacity(final_extracted.len());
        for (rec, meta) in final_extracted {
            out_records.push(rec);
            metas.push(meta);
        }
        // Sort by passage_id for stable doc_id assignment downstream.
        let mut paired: Vec<(PassageRecord, Option<String>)> =
            out_records.into_iter().zip(metas).collect();
        paired.sort_by(|a, b| a.0.passage_id.cmp(&b.0.passage_id));
        // Write in chunks.
        write_chunks(&paired, &out_parquet)?;
        // Return just the records for caller stats.
        paired.into_iter().map(|(r, _)| r).collect()
    };
    eprintln!(
        "Wrote {} passages into {}",
        records.len(),
        out_parquet.display()
    );
    Ok(())
}

fn write_chunks(paired: &[(PassageRecord, Option<String>)], out_parquet: &Path) -> Result<()> {
    let mut part_index = next_part_index(out_parquet, "terebess");
    let mut batch = PassageBatch::default();
    for (rec, meta) in paired {
        batch.push_with_metadata(rec, meta.clone())?;
        if batch.len() >= PARQUET_BATCH_SIZE {
            write_parquet_part_partitioned(&batch, out_parquet, "terebess", part_index, ParquetCompression::Zstd)?;
            part_index += 1;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        write_parquet_part_partitioned(&batch, out_parquet, "terebess", part_index, ParquetCompression::Zstd)?;
    }
    Ok(())
}

fn next_part_index(out_parquet: &Path, corpus: &str) -> usize {
    let dir = out_parquet.join(format!("source_corpus={corpus}"));
    if !dir.exists() {
        return 0;
    }
    let mut max_seen: i64 = -1;
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.filter_map(Result::ok) {
            let name = e.file_name();
            let s = name.to_string_lossy();
            if let Some(rest) = s.strip_prefix("part-") {
                if let Some(num_str) = rest.split('.').next() {
                    if let Ok(n) = num_str.parse::<i64>() {
                        if n > max_seen {
                            max_seen = n;
                        }
                    }
                }
            }
        }
    }
    (max_seen + 1) as usize
}

// ---------------------------------------------------------------------------
// Per-page extraction
// ---------------------------------------------------------------------------

fn extract_page(
    path: &Path,
    images_dir: &Path,
    min_body_chars: usize,
) -> Result<Option<(PassageRecord, Option<String>)>> {
    let raw = fs::read_to_string(path).with_context(|| path.display().to_string())?;

    // Filter 403/404 placeholders by title heuristic.
    let title = capture_title(&raw).unwrap_or_default();
    if is_placeholder_title(&title) {
        return Ok(None);
    }

    // SingleFile-preserved original URL (from the HTML comment).
    let url = capture_url(&raw).unwrap_or_default();

    // Strip toolbar, then scripts/styles, then collapse tags to plain text.
    let body = capture_body(&raw).unwrap_or_else(|| raw.clone());
    let body = strip_toolbar(&body);
    let body = strip_scripts_and_styles(&body);
    let body_text = tags_to_text(&body);
    if body_text.chars().count() < min_body_chars {
        return Ok(None);
    }

    // Extract main image (largest non-icon data URI).
    let image_path = extract_main_image(&body, images_dir, path)?;

    // Stable id: hash of preserved URL (falls back to filename if URL missing).
    let key = if url.is_empty() {
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        url.clone()
    };
    let id_hash = short_hash(&key);
    let passage_id = format!("terebess/{id_hash}");
    let work_id = slug_from_url(&url).unwrap_or_else(|| slug_from_filename(path));
    let clean_title = clean_title(&title);

    let metadata = json!({
        "source_url": url,
        "main_image_path": image_path.as_ref()
            .map(|p| p.display().to_string()).unwrap_or_default(),
        "original_filename": path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
    });
    let metadata_json = serde_json::to_string(&metadata).ok();

    let mut rec = PassageRecord::default();
    rec.source_corpus = "terebess".to_string();
    rec.source_work_id = work_id;
    rec.source_section_id = String::new();
    rec.source_locator = String::new();
    rec.source_url = url;
    rec.edition_siglum = String::new();
    rec.edition_label = String::new();
    rec.rights_id = "terebess.hu".to_string();
    rec.rights_notes =
        "scraped via SingleFile; verify usage rights before redistribution".to_string();
    rec.retrieval_method = "singlefile-html".to_string();
    rec.snapshot_id = String::new();
    rec.quality_flags_json = "{}".to_string();
    rec.passage_id = passage_id;
    rec.source_rel_path = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    rec.xml_id = String::new();
    rec.div_path = String::new();
    rec.heading = clean_title.clone();
    rec.heading_path = clean_title.clone();
    rec.from_lb = None;
    rec.to_lb = None;
    rec.zh_text_raw = body_text.clone();
    rec.zh_text_normalized = body_text.clone();
    rec.text_type = "biography".to_string();
    rec.contains_person = true;
    rec.contains_term = false;
    rec.contains_foreign = true; // multilingual content
    rec.canon = String::new();
    rec.canon_name = String::new();
    rec.traditions = vec!["Zen".to_string()];
    rec.period = String::new();
    rec.origin = String::new();
    rec.author = clean_title.clone();
    rec.main_title = clean_title;
    rec.period_rank = 9999;
    rec.zh = body_text.clone();
    rec.normalized_zh = body_text;

    Ok(Some((rec, metadata_json)))
}

// ---------------------------------------------------------------------------
// HTML helpers (regex-based; the source HTML is flat enough not to need DOM)
// ---------------------------------------------------------------------------

fn capture_title(html: &str) -> Option<String> {
    static_re(r"(?is)<title[^>]*>(.*?)</title>")
        .captures(html)
        .map(|c| {
            c.get(1)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default()
        })
}

fn capture_url(html: &str) -> Option<String> {
    static_re(r"(?i)url:\s*(https?://[^\s<]+)")
        .captures(html)
        .map(|c| c.get(1).map(|m| m.as_str().to_string()).unwrap_or_default())
}

fn capture_body(html: &str) -> Option<String> {
    static_re(r"(?is)<body[^>]*>(.*?)</body>")
        .captures(html)
        .map(|c| c.get(1).map(|m| m.as_str().to_string()).unwrap_or_default())
}

fn strip_toolbar(html: &str) -> String {
    static_re(r#"(?is)<div\s+id\s*=\s*["']?toolbar["']?[^>]*>.*?</div>"#)
        .replace_all(html, "")
        .to_string()
}

fn strip_scripts_and_styles(html: &str) -> String {
    let s = static_re(r"(?is)<script[^>]*>.*?</script>")
        .replace_all(html, " ")
        .to_string();
    static_re(r"(?is)<style[^>]*>.*?</style>")
        .replace_all(&s, " ")
        .to_string()
}

fn tags_to_text(html: &str) -> String {
    let no_tags = static_re(r"<[^>]+>").replace_all(html, " ").to_string();
    let decoded = decode_entities(&no_tags);
    collapse_whitespace(&decoded)
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

fn collapse_whitespace(s: &str) -> String {
    static_re(r"\s+").replace_all(s, " ").trim().to_string()
}

fn is_placeholder_title(title: &str) -> bool {
    let t = title.trim();
    t.starts_with("404 ")
        || t.starts_with("403 ")
        || t.contains("nem található")
        || t.contains("Tilos")
        || t.eq_ignore_ascii_case("Page not found")
}

fn clean_title(raw: &str) -> String {
    // Strip trailing SingleFile-saved timestamp like " (5_8_2026 10：51：35 AM)".
    static_re(r"(?i)\s*\(\d+[_/]\d+[_/]\d+\s+\d+[：:]\d+[：:]\d+(?:\s*[AP]M)?\)\s*$")
        .replace_all(raw, "")
        .to_string()
        .trim()
        .to_string()
}

fn slug_from_url(url: &str) -> Option<String> {
    if url.is_empty() {
        return None;
    }
    let tail = url.rsplit('/').next().unwrap_or("");
    let stem = tail.split('.').next().unwrap_or("");
    if stem.is_empty() {
        None
    } else {
        Some(format!("terebess-{stem}"))
    }
}

fn slug_from_filename(path: &Path) -> String {
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("page");
    let trimmed = clean_title(name);
    let safe: String = trimmed
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let safe = safe.trim_matches('-').to_string();
    if safe.is_empty() {
        "terebess-page".to_string()
    } else {
        format!("terebess-{safe}")
    }
}

fn short_hash(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())[..16].to_string()
}

// ---------------------------------------------------------------------------
// Image extraction
// ---------------------------------------------------------------------------

fn extract_main_image(
    body_html: &str,
    images_dir: &Path,
    source_path: &Path,
) -> Result<Option<PathBuf>> {
    let img_re = static_re(
        r#"(?is)<img[^>]+src\s*=\s*["']?(data:image/([a-z0-9+\-.]+);base64,([A-Za-z0-9+/=\r\n]+))["']?[^>]*>"#,
    );
    let mut best: Option<(usize, String, Vec<u8>)> = None; // (size, ext, bytes)
    for cap in img_re.captures_iter(body_html) {
        let mime = cap
            .get(2)
            .map(|m| m.as_str().to_ascii_lowercase())
            .unwrap_or_default();
        if mime == "svg+xml" || mime == "gif" {
            continue; // toolbar icons
        }
        let b64 = cap.get(3).map(|m| m.as_str()).unwrap_or("");
        // base64 strings can have embedded whitespace from the HTML formatter.
        let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
        let bytes = match base64::engine::general_purpose::STANDARD.decode(cleaned.as_bytes()) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if bytes.len() < 5 * 1024 {
            continue; // tiny inline icons we missed
        }
        let ext = match mime.as_str() {
            "jpeg" | "jpg" => "jpg",
            "png" => "png",
            "webp" => "webp",
            other => other,
        }
        .to_string();
        let take = match &best {
            Some((sz, _, _)) => bytes.len() > *sz,
            None => true,
        };
        if take {
            best = Some((bytes.len(), ext, bytes));
        }
    }
    if let Some((_, ext, bytes)) = best {
        let stem = source_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("img");
        let h = short_hash(stem);
        let name = format!("{h}.{ext}");
        let path = images_dir.join(&name);
        fs::write(&path, &bytes)?;
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Static regex cache helper
// ---------------------------------------------------------------------------

fn static_re(pat: &str) -> Regex {
    Regex::new(pat).expect("regex compile")
}
