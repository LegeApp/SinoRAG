use anyhow::{Context, Result};
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

pub fn run(input: PathBuf, out_corpus: PathBuf, snapshot_id: Option<String>) -> Result<()> {
    let repos = discover_repos(&input)?;
    let xml_root = out_corpus.join("xml-p5").join("kanripo");
    let metadata_dir = out_corpus.join("CBETA_Sorting_Data");
    fs::create_dir_all(&xml_root)?;
    fs::create_dir_all(&metadata_dir)?;

    let mut metadata_items = Vec::new();
    let mut converted = 0usize;
    let mut passage_count = 0usize;

    for repo in repos {
        let work_id = work_id_for_repo(&repo)?;
        let readme = read_readme(&repo);
        let title = title_from_readme(readme.as_deref()).unwrap_or_else(|| work_id.clone());
        let (edition_siglum, edition_label) = edition_from_readme(readme.as_deref());
        let section_files = section_files(&repo, &work_id)?;
        if section_files.is_empty() {
            continue;
        }

        let mut body = String::new();
        for section in &section_files {
            let section_id = section
                .file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or(&work_id)
                .to_string();
            let raw = fs::read_to_string(section)
                .with_context(|| format!("read Kanripo text {}", section.display()))?;
            let paragraphs = segment_paragraphs(&raw);
            if paragraphs.is_empty() {
                continue;
            }

            body.push_str(&format!(
                "      <div type=\"juan\" xml:id=\"{}\" n=\"{}\">\n",
                escape_xml_attr(&section_id),
                escape_xml_attr(section_id.rsplit_once('_').map(|(_, n)| n).unwrap_or(""))
            ));
            body.push_str(&format!(
                "        <head>{}</head>\n",
                escape_xml_text(&format!("{title} {section_id}"))
            ));
            for (idx, paragraph) in paragraphs.iter().enumerate() {
                let xml_id = format!("{section_id}-p{:04}", idx + 1);
                body.push_str(&format!(
                    "        <p xml:id=\"{}\">{}</p>\n",
                    escape_xml_attr(&xml_id),
                    escape_xml_text(paragraph)
                ));
                passage_count += 1;
            }
            body.push_str("      </div>\n");
        }

        if body.is_empty() {
            continue;
        }

        let rel_path = format!("kanripo/{work_id}.xml");
        let xml_path = xml_root.join(format!("{work_id}.xml"));
        let source_url = format!("https://github.com/kanripo/{work_id}");
        let snapshot = snapshot_id.clone().unwrap_or_default();
        let xml = build_tei(
            &work_id,
            &title,
            &edition_siglum,
            &edition_label,
            &source_url,
            &snapshot,
            &body,
        );
        fs::write(&xml_path, xml)?;

        metadata_items.push(json!({
            "file": rel_path,
            "source_corpus": "kanripo",
            "source_work_id": work_id,
            "source_url": source_url,
            "edition_siglum": edition_siglum,
            "edition_label": edition_label,
            "rights_id": "CC-BY-SA-4.0",
            "rights_notes": "Derived from a local Kanripo repository snapshot. Preserve attribution and share-alike obligations for redistributable outputs.",
            "retrieval_method": "local-repository",
            "snapshot_id": snapshot,
            "quality_flags_json": serde_json::to_string(&json!({
                "synthetic_paragraph_segmentation": true,
                "kanripo_plain_text_source": true,
                "source_downloaded_by_graphdiscovery": false
            }))?,
            "canon": "KANRIPO",
            "canon_name": "Kanseki Repository",
            "traditions": ["Classical Chinese"],
            "period": "Unknown Period",
            "origin": "China",
            "author": "",
            "main_title": title,
        }));
        converted += 1;
    }

    let metadata = json!({ "detailed_analysis": metadata_items });
    fs::write(
        metadata_dir.join("buddhist_metadata_analysis.json"),
        serde_json::to_string_pretty(&metadata)?,
    )?;

    println!("converted_works {converted}");
    println!("passages {passage_count}");
    println!("wrote {}", out_corpus.display());
    Ok(())
}

pub fn manifest(input: PathBuf, out: PathBuf) -> Result<()> {
    let repos = discover_repos(&input)?;
    fs::create_dir_all(&out)?;

    let mut work_writer = line_writer(&out.join("work_manifest.jsonl"))?;
    let mut section_writer = line_writer(&out.join("section_manifest.jsonl"))?;
    let mut total_sections = 0usize;
    let mut total_bytes = 0u64;
    let mut total_cjk = 0usize;
    let mut total_estimated_passages = 0usize;

    for repo in repos {
        let work_id = work_id_for_repo(&repo)?;
        let readme = read_readme(&repo);
        let title = title_from_readme(readme.as_deref()).unwrap_or_else(|| work_id.clone());
        let (edition_siglum, edition_label) = edition_from_readme(readme.as_deref());
        let sections = section_files(&repo, &work_id)?;
        let snapshot_id = git_head(&repo).unwrap_or_default();
        let repo_rel = repo
            .strip_prefix(&input)
            .unwrap_or(&repo)
            .to_string_lossy()
            .replace('\\', "/");

        let mut work_raw_bytes = 0u64;
        let mut work_cjk_chars = 0usize;
        let mut work_estimated_passages = 0usize;

        for (idx, section) in sections.iter().enumerate() {
            let raw = fs::read_to_string(section)
                .with_context(|| format!("read Kanripo text {}", section.display()))?;
            let raw_bytes = raw.len() as u64;
            let line_count = raw.lines().count();
            let cjk_char_count = raw.chars().filter(|ch| is_cjk_char(*ch)).count();
            let paragraphs = segment_paragraphs(&raw);
            let section_id = section
                .file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or(&work_id)
                .to_string();
            let rel_path = section
                .strip_prefix(&input)
                .unwrap_or(section)
                .to_string_lossy()
                .replace('\\', "/");
            let heading_guess = paragraphs.first().cloned().unwrap_or_default();

            writeln!(
                section_writer,
                "{}",
                serde_json::to_string(&json!({
                    "source_corpus": "kanripo",
                    "source_work_id": work_id,
                    "source_section_id": section_id,
                    "rel_path": rel_path,
                    "raw_bytes": raw_bytes,
                    "line_count": line_count,
                    "cjk_char_count": cjk_char_count,
                    "heading_guess": heading_guess,
                    "section_order": idx + 1,
                    "estimated_passage_count": paragraphs.len(),
                    "quality_flags_json": serde_json::to_string(&json!({
                        "synthetic_paragraph_segmentation": true,
                        "paragraph_confidence": "low",
                        "source_format": "kanripo_txt",
                        "work_level_date_only": true
                    }))?
                }))?
            )?;

            work_raw_bytes += raw_bytes;
            work_cjk_chars += cjk_char_count;
            work_estimated_passages += paragraphs.len();
        }

        total_sections += sections.len();
        total_bytes += work_raw_bytes;
        total_cjk += work_cjk_chars;
        total_estimated_passages += work_estimated_passages;

        writeln!(
            work_writer,
            "{}",
            serde_json::to_string(&json!({
                "source_corpus": "kanripo",
                "source_work_id": work_id,
                "repo_path": repo.display().to_string(),
                "repo_rel_path": repo_rel,
                "title": title,
                "author": "",
                "period": "Unknown Period",
                "edition_siglum": edition_siglum,
                "edition_label": edition_label,
                "rights_id": "CC-BY-SA-4.0",
                "snapshot_id": snapshot_id,
                "file_count": sections.len(),
                "raw_bytes": work_raw_bytes,
                "cjk_char_count": work_cjk_chars,
                "estimated_passage_count": work_estimated_passages,
                "quality_flags_json": serde_json::to_string(&json!({
                    "synthetic_paragraph_segmentation": true,
                    "paragraph_confidence": "low",
                    "source_format": "kanripo_txt",
                    "work_level_date_only": true
                }))?
            }))?
        )?;
    }

    fs::write(
        out.join("manifest_summary.json"),
        serde_json::to_string_pretty(&json!({
            "source_corpus": "kanripo",
            "work_count": count_lines(&out.join("work_manifest.jsonl"))?,
            "section_count": total_sections,
            "raw_bytes": total_bytes,
            "cjk_char_count": total_cjk,
            "estimated_passage_count": total_estimated_passages,
            "note": "Manifest counts are derived from local Kanripo repositories without full TEI conversion."
        }))?,
    )?;

    println!("works {}", count_lines(&out.join("work_manifest.jsonl"))?);
    println!("sections {total_sections}");
    println!("estimated_passages {total_estimated_passages}");
    println!("wrote {}", out.display());
    Ok(())
}

fn discover_repos(input: &Path) -> Result<Vec<PathBuf>> {
    if let Ok(work_id) = work_id_for_repo(input) {
        if section_files(input, &work_id)
            .map(|files| !files.is_empty())
            .unwrap_or(false)
        {
            return Ok(vec![input.to_path_buf()]);
        }
    }

    let mut repos = Vec::new();
    for entry in WalkDir::new(input)
        .min_depth(1)
        .max_depth(3)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(work_id) = work_id_for_repo(path) else {
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
    if repos.is_empty() {
        anyhow::bail!(
            "No Kanripo work repositories found under {}",
            input.display()
        );
    }
    Ok(repos)
}

fn work_id_for_repo(repo: &Path) -> Result<String> {
    let name = repo
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| anyhow::anyhow!("Cannot infer Kanripo work id from {}", repo.display()))?;
    if name.starts_with("KR") {
        Ok(name.to_string())
    } else {
        let first = WalkDir::new(repo)
            .max_depth(1)
            .into_iter()
            .filter_map(Result::ok)
            .map(|entry| entry.path().to_path_buf())
            .find_map(|path| {
                path.file_stem()
                    .and_then(|v| v.to_str())
                    .and_then(|stem| stem.split_once('_').map(|(work, _)| work.to_string()))
                    .filter(|work| work.starts_with("KR"))
            });
        first.ok_or_else(|| anyhow::anyhow!("Cannot infer Kanripo work id from {}", repo.display()))
    }
}

fn section_files(repo: &Path, work_id: &str) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(repo).with_context(|| format!("read {}", repo.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) == Some("txt")
            && path
                .file_stem()
                .and_then(|v| v.to_str())
                .map(|stem| stem.starts_with(&format!("{work_id}_")))
                .unwrap_or(false)
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn segment_paragraphs(raw: &str) -> Vec<String> {
    raw.lines()
        .map(|line| line.trim().trim_start_matches('\u{feff}').trim())
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with('#'))
        .filter(|line| crate::normalize::contains_cjk(line))
        .map(|line| line.to_string())
        .collect()
}

fn line_writer(path: &Path) -> Result<fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::File::create(path).with_context(|| format!("create {}", path.display()))
}

fn count_lines(path: &Path) -> Result<usize> {
    Ok(fs::read_to_string(path)?.lines().count())
}

fn is_cjk_char(ch: char) -> bool {
    ('\u{3400}'..='\u{4dbf}').contains(&ch)
        || ('\u{4e00}'..='\u{9fff}').contains(&ch)
        || ('\u{f900}'..='\u{faff}').contains(&ch)
}

fn git_head(repo: &Path) -> Option<String> {
    let output = Command::new("git")
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

fn read_readme(repo: &Path) -> Option<String> {
    ["Readme.org", "README.org", "README.md", "Readme.md"]
        .iter()
        .map(|name| repo.join(name))
        .find(|path| path.is_file())
        .and_then(|path| fs::read_to_string(path).ok())
}

fn title_from_readme(readme: Option<&str>) -> Option<String> {
    let readme = readme?;
    for line in readme.lines() {
        let trimmed = line.trim();
        for prefix in ["#+TITLE:", "#+title:", "TITLE:", "Title:"] {
            if let Some(title) = trimmed.strip_prefix(prefix) {
                let title = title.trim();
                if !title.is_empty() {
                    return Some(title.to_string());
                }
            }
        }
    }
    None
}

fn edition_from_readme(readme: Option<&str>) -> (String, String) {
    let Some(readme) = readme else {
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

fn build_tei(
    work_id: &str,
    title: &str,
    edition_siglum: &str,
    edition_label: &str,
    source_url: &str,
    snapshot_id: &str,
    body: &str,
) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<TEI xmlns="http://www.tei-c.org/ns/1.0" xml:id="{work_id}">
  <teiHeader>
    <fileDesc>
      <titleStmt>
        <title>{title}</title>
      </titleStmt>
      <publicationStmt>
        <availability status="free">
          <licence target="https://creativecommons.org/licenses/by-sa/4.0/">CC BY-SA 4.0</licence>
        </availability>
      </publicationStmt>
      <sourceDesc>
        <bibl>
          <idno type="kanripo">{work_id}</idno>
          <idno type="url">{source_url}</idno>
          <edition>{edition_siglum}</edition>
          <note>{edition_label}</note>
          <note type="snapshot">{snapshot_id}</note>
        </bibl>
      </sourceDesc>
    </fileDesc>
  </teiHeader>
  <text>
    <body>
{body}    </body>
  </text>
</TEI>
"#,
        work_id = escape_xml_attr(work_id),
        title = escape_xml_text(title),
        source_url = escape_xml_text(source_url),
        edition_siglum = escape_xml_text(edition_siglum),
        edition_label = escape_xml_text(edition_label),
        snapshot_id = escape_xml_text(snapshot_id),
        body = body
    )
}

fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_xml_attr(value: &str) -> String {
    escape_xml_text(value).replace('"', "&quot;")
}
