//! Direct ingest of Kanripo plain-text repositories into `PassageRecord`s.
//!
//! Kanripo texts are mandoku-format `.txt` files (not TEI XML). To load them
//! alongside CBETA TEI without a separate `kanripo-to-tei` conversion step,
//! this module yields `PassageRecord` directly from the source layout
//!
//! ```text
//! <kanripo_root>/texts/KR<n>/KR<work_id>/KR<work_id>_<NNN>.txt
//! ```
//!
//! Lines are coalesced into paragraph-sized passages: the Mandoku
//! plaintext is column-wrapped at ~20 CJK chars/line, so each raw line
//! is a typographic fragment, not a semantic unit. We accumulate lines
//! and break on any of (blank line after stripping `¶`/tags, `<pb:`
//! page marker, line beginning with ≥2 full-width spaces), but only
//! once the accumulator has reached `MIN_PASSAGE_CJK_CHARS`; short
//! headings/stanzas therefore merge into the surrounding passage.
//! A hard `MAX_PASSAGE_CJK_CHARS` cap prevents runaway when no break
//! signal arrives. This yields CBETA-`<p>`-comparable granularity and
//! shrinks the Kanripo passage count by roughly an order of magnitude.

use crate::models::PassageRecord;
use crate::normalize::{contains_cjk, normalize_zh};
use anyhow::{Context, Result};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const MIN_PASSAGE_CJK_CHARS: usize = 120;
const MAX_PASSAGE_CJK_CHARS: usize = 600;

/// Walk a kanripo clone and yield passage records.
///
/// `kanripo_root` should be the repository root (the directory that
/// contains `texts/`). If a `texts/` subdirectory is present it is used,
/// otherwise the root itself is treated as the work tree.
pub fn extract_passages(kanripo_root: &Path) -> Result<Vec<PassageRecord>> {
    let scan_root = if kanripo_root.join("texts").is_dir() {
        kanripo_root.join("texts")
    } else {
        kanripo_root.to_path_buf()
    };

    let repos = discover_work_repos(&scan_root)?;
    let mut out = Vec::new();
    for repo in repos {
        let work_id = match work_id_for_repo(&repo) {
            Some(v) => v,
            None => continue,
        };
        let title = read_title(&repo).unwrap_or_else(|| work_id.clone());
        let (edition_siglum, edition_label) = read_edition(&repo);
        let snapshot = git_head(&repo).unwrap_or_default();
        let rel_repo = repo
            .strip_prefix(&scan_root)
            .unwrap_or(&repo)
            .to_string_lossy()
            .replace('\\', "/");
        let sections = section_files(&repo, &work_id)?;
        for section in sections {
            extract_section_passages(
                &section,
                &work_id,
                &title,
                &edition_siglum,
                &edition_label,
                &snapshot,
                &rel_repo,
                &mut out,
            )?;
        }
    }
    Ok(out)
}

/// Discover all work repositories in the scan root.
pub fn discover_work_repos(scan_root: &Path) -> Result<Vec<PathBuf>> {
    let mut repos = Vec::new();
    for entry in WalkDir::new(scan_root)
        .min_depth(1)
        .max_depth(3)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(work_id) = work_id_for_repo(path) else {
            continue;
        };
        if section_files(path, &work_id)
            .map(|files| !files.is_empty())
            .unwrap_or(false)
        {
            repos.push(path.to_path_buf());
        }
    }
    repos.sort();
    Ok(repos)
}

/// Extract passages from a single section file.
pub fn extract_section_passages(
    section_file: &Path,
    work_id: &str,
    title: &str,
    edition_siglum: &str,
    edition_label: &str,
    snapshot: &str,
    rel_repo: &str,
    out: &mut Vec<PassageRecord>,
) -> Result<()> {
    let raw = fs::read_to_string(section_file)
        .with_context(|| format!("read kanripo text {}", section_file.display()))?;
    let section_id = section_file
        .file_stem()
        .and_then(|s: &std::ffi::OsStr| s.to_str())
        .unwrap_or(work_id)
        .to_string();
    let rel_path = format!("kanripo/{work_id}/{section_id}.txt");
    let source_url = format!("https://github.com/kanripo/{work_id}");
    let quality_flags = serde_json::to_string(&json!({
        "synthetic_paragraph_segmentation": true,
        "kanripo_plain_text_source": true,
        "source_format": "kanripo_txt",
        "paragraph_confidence": "medium",
        "coalesce_strategy": "blank|pb|heading break gated by min/max chars"
    }))
    .unwrap_or_default();

    struct Acc {
        text: String,
        cjk_count: usize,
        first_line: u32,
        last_line: u32,
    }

    let mut acc: Option<Acc> = None;
    let mut next_ord: u32 = 0;

    let mut flush =
        |acc: &mut Option<Acc>, out: &mut Vec<PassageRecord>, next_ord: &mut u32| {
            let Some(a) = acc.take() else { return };
            if a.cjk_count == 0 {
                return;
            }
            let ord = *next_ord;
            *next_ord += 1;
            let xml_id = format!("{section_id}-p{:04}", ord + 1);
            let passage_id = format!("{rel_path}#{xml_id}");
            let raw_text = a.text;
            let normalized = normalize_zh(&raw_text);
            out.push(
                PassageRecord {
                    source_corpus: "kanripo".to_string(),
                    source_work_id: work_id.to_string(),
                    source_section_id: section_id.clone(),
                    source_locator: section_id.clone(),
                    source_url: source_url.clone(),
                    edition_siglum: edition_siglum.to_string(),
                    edition_label: edition_label.to_string(),
                    rights_id: "CC-BY-SA-4.0".to_string(),
                    rights_notes:
                        "Derived from a local Kanripo repository snapshot. Preserve attribution and share-alike obligations for redistributable outputs."
                            .to_string(),
                    retrieval_method: "local-repository".to_string(),
                    snapshot_id: snapshot.to_string(),
                    quality_flags_json: quality_flags.clone(),
                    passage_id,
                    source_rel_path: rel_path.clone(),
                    xml_id,
                    div_path: rel_repo.to_string(),
                    heading: title.to_string(),
                    heading_path: format!("{title} / {section_id}"),
                    from_lb: Some(format!("L{}", a.first_line)),
                    to_lb: Some(format!("L{}", a.last_line)),
                    passage_ord_in_file: ord,
                    zh_text_raw: raw_text,
                    zh_text_normalized: normalized,
                    text_type: "paragraph".to_string(),
                    contains_person: false,
                    contains_term: false,
                    contains_foreign: false,
                    canon: "KANRIPO".to_string(),
                    canon_name: "Kanseki Repository".to_string(),
                    traditions: vec!["Classical Chinese".to_string()],
                    period: "Unknown Period".to_string(),
                    origin: "China".to_string(),
                    author: String::new(),
                    main_title: title.to_string(),
                    period_rank: 99,
                    zh: String::new(),
                    normalized_zh: String::new(),
                }
                .finalize_aliases(),
            );
        };

    for (idx, line) in raw.lines().enumerate() {
        let line_no = (idx + 1) as u32;
        let s = line.trim_start_matches('\u{feff}');

        let is_pb = s.trim_start().starts_with("<pb:");
        let leading_fw = s.chars().take_while(|&c| c == '　').count();
        let is_heading = leading_fw >= 2;

        let stripped = strip_mandoku_markup(s);
        let stripped_trim = stripped.trim();
        let is_blank = stripped_trim.is_empty() || stripped_trim.starts_with('#');

        let break_signal = is_blank || is_pb || is_heading;

        if break_signal
            && acc
                .as_ref()
                .map(|a| a.cjk_count >= MIN_PASSAGE_CJK_CHARS)
                .unwrap_or(false)
        {
            flush(&mut acc, out, &mut next_ord);
        }

        if is_blank || is_pb {
            continue;
        }

        if !contains_cjk(stripped_trim) {
            continue;
        }

        let n_cjk = stripped_trim
            .chars()
            .filter(|c| {
                let cp = *c as u32;
                (0x3400..=0x4dbf).contains(&cp)
                    || (0x4e00..=0x9fff).contains(&cp)
                    || (0x20000..=0x2ffff).contains(&cp)
                    || (0xf900..=0xfaff).contains(&cp)
            })
            .count();

        match acc.as_mut() {
            Some(a) => {
                a.text.push('\n');
                a.text.push_str(stripped_trim);
                a.cjk_count += n_cjk;
                a.last_line = line_no;
            }
            None => {
                acc = Some(Acc {
                    text: stripped_trim.to_string(),
                    cjk_count: n_cjk,
                    first_line: line_no,
                    last_line: line_no,
                });
            }
        }

        if acc
            .as_ref()
            .map(|a| a.cjk_count >= MAX_PASSAGE_CJK_CHARS)
            .unwrap_or(false)
        {
            flush(&mut acc, out, &mut next_ord);
        }
    }

    flush(&mut acc, out, &mut next_ord);

    Ok(())
}

fn strip_mandoku_markup(line: &str) -> String {
    // Remove <pb:...>, <lb:...>, <mulu:...> tags and the mandoku ¶ paragraph
    // separator. Mandoku also uses <...> pseudo-XML in many places.
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '<' {
            for c in chars.by_ref() {
                if c == '>' {
                    break;
                }
            }
            continue;
        }
        if ch == '¶' {
            continue;
        }
        out.push(ch);
    }
    out
}

pub fn work_id_for_repo(repo: &Path) -> Option<String> {
    let name = repo.file_name().and_then(|v| v.to_str())?;
    if name.starts_with("KR") && name.len() >= 4 {
        // Heuristic: actual work directories are like `KR1a0001`.
        // Top-level group dirs `KR1`..`KR6` are too short.
        if name.chars().filter(|c| c.is_ascii_digit()).count() >= 3 {
            return Some(name.to_string());
        }
    }
    None
}

pub fn section_files(repo: &Path, work_id: &str) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let prefix = format!("{work_id}_");
    for entry in fs::read_dir(repo).with_context(|| format!("read {}", repo.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("txt") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if stem.starts_with(&prefix) {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

pub fn read_title(repo: &Path) -> Option<String> {
    let readme = read_readme(repo)?;
    for line in readme.lines() {
        let trimmed = line.trim();
        for prefix in ["#+TITLE:", "#+title:", "TITLE:", "Title:"] {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                let value = rest.trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

pub fn read_edition(repo: &Path) -> (String, String) {
    let Some(readme) = read_readme(repo) else {
        return (String::new(), String::new());
    };
    for line in readme.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix("#+PROPERTY: EDITION")
            .or_else(|| trimmed.strip_prefix("#+PROPERTY: edition"))
            .or_else(|| trimmed.strip_prefix("edition:"))
            .or_else(|| trimmed.strip_prefix("Edition:"))
        {
            let value = rest.trim().trim_start_matches(':').trim();
            if !value.is_empty() {
                let siglum = value.split_whitespace().next().unwrap_or("").to_string();
                return (siglum, value.to_string());
            }
        }
    }
    (String::new(), String::new())
}

fn read_readme(repo: &Path) -> Option<String> {
    ["Readme.org", "README.org", "README.md", "Readme.md"]
        .iter()
        .map(|name| repo.join(name))
        .find(|path| path.is_file())
        .and_then(|path| fs::read_to_string(path).ok())
}

pub fn git_head(repo: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()?;
    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}
