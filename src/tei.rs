use crate::models::PassageRecord;
use crate::normalize::{collapse_whitespace, contains_cjk, normalize_zh};
use anyhow::{Context, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use rayon::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const PASSAGE_TAGS: &[&str] = &["p", "lg"];
const SKIP_TEXT_TAGS: &[&str] = &[
    "note",
    "anchor",
    "lb",
    "pb",
    "milestone",
    "graphic",
    "figure",
    "space",
];

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BuddhistMeta {
    #[serde(default = "default_source_corpus")]
    pub source_corpus: String,
    #[serde(default)]
    pub source_work_id: String,
    #[serde(default)]
    pub source_section_id: String,
    #[serde(default)]
    pub source_locator: String,
    #[serde(default)]
    pub source_url: String,
    #[serde(default)]
    pub edition_siglum: String,
    #[serde(default)]
    pub edition_label: String,
    #[serde(default)]
    pub rights_id: String,
    #[serde(default)]
    pub rights_notes: String,
    #[serde(default)]
    pub retrieval_method: String,
    #[serde(default)]
    pub snapshot_id: String,
    #[serde(default)]
    pub quality_flags_json: String,
    #[serde(default)]
    pub canon: String,
    #[serde(default)]
    pub canon_name: String,
    #[serde(default)]
    pub traditions: Vec<String>,
    #[serde(default)]
    pub period: String,
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub main_title: String,
    #[serde(default = "default_period_rank")]
    pub period_rank: i32,
}

#[derive(Debug, Deserialize)]
struct MetadataPayload {
    #[serde(default)]
    detailed_analysis: Vec<MetadataItem>,
}

#[derive(Debug, Deserialize)]
struct MetadataItem {
    #[serde(default)]
    file: String,
    #[serde(default)]
    source_corpus: String,
    #[serde(default)]
    source_work_id: String,
    #[serde(default)]
    source_section_id: String,
    #[serde(default)]
    source_locator: String,
    #[serde(default)]
    source_url: String,
    #[serde(default)]
    edition_siglum: String,
    #[serde(default)]
    edition_label: String,
    #[serde(default)]
    rights_id: String,
    #[serde(default)]
    rights_notes: String,
    #[serde(default)]
    retrieval_method: String,
    #[serde(default)]
    snapshot_id: String,
    #[serde(default)]
    quality_flags_json: String,
    #[serde(default)]
    canon: String,
    #[serde(default)]
    canon_name: String,
    #[serde(default)]
    traditions: Vec<String>,
    period: Option<String>,
    #[serde(default)]
    origin: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    main_title: String,
}

pub fn load_buddhist_metadata(
    corpus_root: &Path,
    sorting_data_dir: Option<&Path>,
) -> Result<HashMap<String, BuddhistMeta>> {
    let mut candidates = Vec::new();
    if let Some(dir) = sorting_data_dir {
        candidates.push(dir.join("buddhist_metadata_analysis.json"));
    }
    candidates.push(
        corpus_root
            .join("CBETA_Sorting_Data")
            .join("buddhist_metadata_analysis.json"),
    );
    if let Some(parent) = corpus_root.parent() {
        candidates.push(
            parent
                .join("CBETA_Sorting_Data")
                .join("buddhist_metadata_analysis.json"),
        );
    }

    let Some(path) = candidates.into_iter().find(|p| p.is_file()) else {
        return Ok(HashMap::new());
    };

    let payload: MetadataPayload = serde_json::from_slice(&std::fs::read(&path)?)?;
    let mut out = HashMap::new();
    for item in payload.detailed_analysis {
        let Some(rel_path) = normalize_metadata_path(&item.file) else {
            continue;
        };
        let period = item.period.unwrap_or_else(|| "Unknown Period".to_string());
        out.insert(
            rel_path,
            BuddhistMeta {
                source_corpus: if item.source_corpus.is_empty() {
                    default_source_corpus()
                } else {
                    item.source_corpus
                },
                source_work_id: item.source_work_id,
                source_section_id: item.source_section_id,
                source_locator: item.source_locator,
                source_url: item.source_url,
                edition_siglum: item.edition_siglum,
                edition_label: item.edition_label,
                rights_id: item.rights_id,
                rights_notes: item.rights_notes,
                retrieval_method: item.retrieval_method,
                snapshot_id: item.snapshot_id,
                quality_flags_json: item.quality_flags_json,
                canon: item.canon,
                canon_name: item.canon_name,
                traditions: item.traditions,
                period_rank: period_rank(&period),
                period,
                origin: item.origin,
                author: item.author,
                main_title: item.main_title,
            },
        );
    }
    Ok(out)
}

pub fn iter_xml_paths(corpus_root: &Path) -> Result<Vec<(PathBuf, String)>> {
    let original_root = corpus_root.join("xml-p5");
    if !original_root.is_dir() {
        anyhow::bail!(
            "Original corpus directory not found: {}",
            original_root.display()
        );
    }

    let mut paths = Vec::new();
    for entry in WalkDir::new(&original_root)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("xml") {
            continue;
        }
        let rel = path
            .strip_prefix(&original_root)?
            .to_string_lossy()
            .replace('\\', "/");
        paths.push((path.to_path_buf(), rel));
    }

    paths.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(paths)
}

pub fn extract_passages_from_file(
    xml_path: &Path,
    rel_path: &str,
    meta: &BuddhistMeta,
) -> Result<Vec<PassageRecord>> {
    let file = File::open(xml_path).with_context(|| format!("open XML {}", xml_path.display()))?;
    let mut reader = Reader::from_reader(BufReader::new(file));
    reader.config_mut().trim_text(false);

    let mut buf = Vec::new();
    let mut results = Vec::new();
    let mut in_body = false;
    let mut skip_depth = 0usize;
    let mut div_stack: Vec<String> = Vec::new();
    let mut previous_lb: Option<String> = None;
    let mut current_heading = String::new();
    let mut current_heading_path = String::new();
    let mut heading_text: Option<String> = None;
    let mut active: Option<ActivePassage> = None;
    let mut passage_ord_in_file: u32 = 0;

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                let name = local_name(e.name().as_ref()).to_string();
                if name == "body" {
                    in_body = true;
                }
                if !in_body {
                    buf.clear();
                    continue;
                }
                if SKIP_TEXT_TAGS.contains(&name.as_str()) {
                    skip_depth += 1;
                    buf.clear();
                    continue;
                }
                if name == "div" {
                    let label = attr_value(&e, b"type")
                        .or_else(|| attr_value(&e, b"n"))
                        .or_else(|| attr_value(&e, b"id"))
                        .unwrap_or_else(|| "div".to_string());
                    div_stack.push(label);
                }
                if name == "head" || name == "mulu" {
                    heading_text = Some(String::new());
                }
                if let Some(active) = active.as_mut() {
                    match name.as_str() {
                        "persName" => active.contains_person = true,
                        "term" => active.contains_term = true,
                        "foreign" => active.contains_foreign = true,
                        _ => {}
                    }
                }
                if PASSAGE_TAGS.contains(&name.as_str()) {
                    if let Some(xml_id) = attr_value(&e, b"id") {
                        active = Some(ActivePassage {
                            tag: name,
                            xml_id,
                            text: String::new(),
                            lbs: Vec::new(),
                            div_path: div_stack.join(" / "),
                            heading: current_heading.clone(),
                            heading_path: if current_heading_path.is_empty() {
                                current_heading.clone()
                            } else {
                                current_heading_path.clone()
                            },
                            from_previous_lb: previous_lb.clone(),
                            contains_person: false,
                            contains_term: false,
                            contains_foreign: false,
                        });
                    }
                }
            }
            Event::Empty(e) => {
                let name = local_name(e.name().as_ref()).to_string();
                if name == "lb" {
                    if let Some(n) = attr_value(&e, b"n") {
                        previous_lb = Some(n.clone());
                        if let Some(active) = active.as_mut() {
                            active.lbs.push(n);
                        }
                    }
                }
            }
            Event::Text(e) => {
                if in_body && skip_depth == 0 {
                    let text = String::from_utf8_lossy(e.as_ref()).to_string();
                    if let Some(heading) = heading_text.as_mut() {
                        heading.push_str(&text);
                    }
                    if let Some(active) = active.as_mut() {
                        active.text.push_str(&text);
                    }
                }
            }
            Event::End(e) => {
                let name = local_name(e.name().as_ref()).to_string();
                if skip_depth > 0 && SKIP_TEXT_TAGS.contains(&name.as_str()) {
                    skip_depth -= 1;
                    buf.clear();
                    continue;
                }
                if name == "head" || name == "mulu" {
                    if let Some(text) = heading_text.take() {
                        let text = collapse_whitespace(&text);
                        if !text.is_empty() {
                            current_heading = text.clone();
                            current_heading_path = text;
                        }
                    }
                }
                if let Some(active_p) = active.take() {
                    if name == active_p.tag {
                        if let Some(mut record) = active_p.into_record(rel_path, meta, &mut previous_lb)
                        {
                            record.passage_ord_in_file = passage_ord_in_file;
                            passage_ord_in_file += 1;
                            results.push(record.finalize_aliases());
                        }
                    } else {
                        active = Some(active_p);
                    }
                }
                if name == "div" {
                    div_stack.pop();
                }
                if name == "body" {
                    in_body = false;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(results)
}

struct ActivePassage {
    tag: String,
    xml_id: String,
    text: String,
    lbs: Vec<String>,
    div_path: String,
    heading: String,
    heading_path: String,
    from_previous_lb: Option<String>,
    contains_person: bool,
    contains_term: bool,
    contains_foreign: bool,
}

impl ActivePassage {
    fn into_record(
        self,
        rel_path: &str,
        meta: &BuddhistMeta,
        previous_lb: &mut Option<String>,
    ) -> Option<PassageRecord> {
        let raw = collapse_whitespace(&self.text);
        if raw.is_empty() || !contains_cjk(&raw) {
            return None;
        }

        let from_lb = self.lbs.first().cloned().or(self.from_previous_lb);
        let to_lb = self.lbs.last().cloned().or(from_lb.clone());
        if let Some(last) = self.lbs.last() {
            *previous_lb = Some(last.clone());
        }

        let text_type = if self.tag == "lg" {
            "verse"
        } else if raw.contains('問')
            && (raw.contains('答') || raw.contains('云') || raw.contains('曰'))
        {
            "dialogue"
        } else {
            "prose"
        };
        let normalized = normalize_zh(&raw);

        Some(PassageRecord {
            source_corpus: meta.source_corpus.clone(),
            source_work_id: meta.source_work_id.clone(),
            source_section_id: meta.source_section_id.clone(),
            source_locator: meta.source_locator.clone(),
            source_url: meta.source_url.clone(),
            edition_siglum: meta.edition_siglum.clone(),
            edition_label: meta.edition_label.clone(),
            rights_id: meta.rights_id.clone(),
            rights_notes: meta.rights_notes.clone(),
            retrieval_method: meta.retrieval_method.clone(),
            snapshot_id: meta.snapshot_id.clone(),
            quality_flags_json: meta.quality_flags_json.clone(),
            passage_id: format!("{rel_path}#{}", self.xml_id),
            source_rel_path: rel_path.to_string(),
            xml_id: self.xml_id,
            div_path: self.div_path,
            heading: self.heading,
            heading_path: self.heading_path,
            from_lb: None,
            to_lb: None,
            passage_ord_in_file: 0,
            zh_text_raw: raw.clone(),
            zh_text_normalized: normalized.clone(),
            text_type: text_type.to_string(),
            contains_person: self.contains_person,
            contains_term: self.contains_term,
            contains_foreign: self.contains_foreign,
            canon: meta.canon.clone(),
            canon_name: meta.canon_name.clone(),
            traditions: meta.traditions.clone(),
            period: meta.period.clone(),
            origin: meta.origin.clone(),
            author: meta.author.clone(),
            main_title: meta.main_title.clone(),
            period_rank: meta.period_rank,
            zh: raw,
            normalized_zh: normalized,
        })
    }
}

fn attr_value(e: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    for attr in e.attributes().flatten() {
        let k = attr.key.as_ref();
        if k == key || k.ends_with(key) {
            return Some(String::from_utf8_lossy(attr.value.as_ref()).to_string());
        }
    }
    None
}

fn local_name(name: &[u8]) -> &str {
    let raw = std::str::from_utf8(name).unwrap_or("");
    raw.rsplit_once(':').map(|(_, local)| local).unwrap_or(raw)
}

fn normalize_metadata_path(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let normalized = value.replace('\\', "/");
    if let Some((_, tail)) = normalized.split_once("xml-p5/") {
        return Some(tail.to_string());
    }
    Some(normalized.trim_start_matches('/').to_string())
}

fn period_rank(period: &str) -> i32 {
    match period {
        "Han" => 1,
        "Three Kingdoms" => 2,
        "Jin" => 3,
        "Northern and Southern" => 4,
        "Sui" => 5,
        "Tang" => 6,
        "Five Dynasties" => 7,
        "Song" => 8,
        "Yuan" => 9,
        "Ming" => 10,
        "Qing" => 11,
        _ => 99,
    }
}

fn default_period_rank() -> i32 {
    99
}

fn default_source_corpus() -> String {
    "cbeta".to_string()
}
