use crate::cbeta_sidecar::SidecarIndex;
use crate::models::PassageRecord;
use crate::normalize::{collapse_whitespace, contains_cjk, normalize_zh};
use anyhow::{Context, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Which CBETA distribution a given xml root came from. The two CBETA
/// releases are not interchangeable — github xml-p5 is ~20% of the full
/// ISO, with different volume coverage and one-file-per-work vs ISO's
/// one-file-per-fascicle granularity. We tag every passage with this so
/// downstream catalogs / dedup don't conflate the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CbetaDistribution {
    /// github.com/cbeta-org/xml-p5 snapshot (one file per work).
    GitHubP5,
    /// CBETA ISO `xml-iso/` tree (one file per fascicle).
    IsoP5,
    /// Translation overlay (`xml-p5t/`) — same TEI, with translated nodes added.
    P5Translated,
    /// Bare directory of XML — we can't tell which release.
    Unknown,
    /// Output of `sinorag merge-cbeta` — may contain both single-work files
    /// (GitHub) and fascicle files (ISO). Distribution is inferred per-file
    /// from the presence of the `_NNN` suffix on the filename stem.
    Merged,
}

impl CbetaDistribution {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GitHubP5 => "xml-p5-github",
            Self::IsoP5 => "xml-iso",
            Self::P5Translated => "xml-p5t",
            Self::Unknown => "unknown",
            Self::Merged => "xml-merged",
        }
    }
}

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

#[derive(Debug, Clone)]
pub struct BuddhistMeta {
    pub source_corpus: String,
    pub source_work_id: String,
    pub source_section_id: String,
    pub source_locator: String,
    pub source_url: String,
    pub edition_siglum: String,
    pub edition_label: String,
    pub rights_id: String,
    pub rights_notes: String,
    pub retrieval_method: String,
    pub snapshot_id: String,
    pub quality_flags_json: String,
    pub canon: String,
    pub canon_name: String,
    pub traditions: Vec<String>,
    pub period: String,
    pub origin: String,
    pub author: String,
    pub main_title: String,
    pub period_rank: i32,
}

impl Default for BuddhistMeta {
    fn default() -> Self {
        Self {
            source_corpus: default_source_corpus(),
            source_work_id: String::new(),
            source_section_id: String::new(),
            source_locator: String::new(),
            source_url: String::new(),
            edition_siglum: String::new(),
            edition_label: String::new(),
            rights_id: String::new(),
            rights_notes: String::new(),
            retrieval_method: String::new(),
            snapshot_id: String::new(),
            quality_flags_json: String::new(),
            canon: String::new(),
            canon_name: String::new(),
            traditions: Vec::new(),
            period: String::new(),
            origin: String::new(),
            author: String::new(),
            main_title: String::new(),
            period_rank: default_period_rank(),
        }
    }
}

/// Extract Buddhist metadata directly from TEI XML file during parsing.
///
/// Classification (tradition / period / origin) comes from the
/// `buddhist_metadata_analysis.json` sidecar when available — that file
/// is the ground-truth catalog produced by `CBETA_Sorting_Data/` and
/// covers the full 4990-work corpus. When a rel_path is not in the
/// sidecar (e.g. a corpus subset or post-snapshot additions), we fall
/// back to `classify_*_fallback`, which is intentionally narrower than
/// the legacy heuristic.
pub fn extract_metadata_from_xml(
    xml_path: &Path,
    rel_path: &str,
    sidecar: Option<&SidecarIndex>,
    distribution: CbetaDistribution,
    catalog: Option<&crate::cbeta_sidecar::WorkCatalog>,
) -> BuddhistMeta {
    let file = File::open(xml_path);
    let mut meta = BuddhistMeta::default();

    // Derive source_work_id from filename.
    // For ISO split-fascicle files (e.g. T01n0001_001.xml) strip the _NNN suffix
    // so all fascicles of the same work share a single source_work_id.
    if let Some(filename) = xml_path.file_stem().and_then(|s| s.to_str()) {
        let work_id = strip_fascicle_suffix(filename);
        meta.source_work_id = work_id.to_string();
    }

    // Extract canon from file path (first directory component)
    let path_parts: Vec<&str> = rel_path.split('/').collect();
    if path_parts.len() >= 1 {
        meta.canon = path_parts[0].to_string();
    }

    // Parse TEI header for additional metadata
    if let Ok(file) = file {
        let reader = Reader::from_reader(BufReader::new(file));
        let mut buf = Vec::new();
        let mut in_header = false;
        let mut in_title = false;
        let mut in_author = false;
        let mut in_source_desc = false;
        let mut in_availability = false;
        let mut current_text = String::new();

        let mut reader = reader;
        reader.config_mut().trim_text(true);

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let name_bytes = e.name().as_ref().to_vec();
                    let name = local_name(&name_bytes);
                    match name {
                        "teiHeader" => in_header = true,
                        "title" if in_header => in_title = true,
                        "author" if in_header => in_author = true,
                        "sourceDesc" if in_header => in_source_desc = true,
                        "availability" if in_header => in_availability = true,
                        _ => {}
                    }
                    buf.clear();
                }
                Ok(Event::Text(e)) => {
                    let text = String::from_utf8_lossy(e.as_ref()).to_string();
                    if in_title && meta.main_title.is_empty() {
                        meta.main_title = text.trim().to_string();
                    } else if in_author && meta.author.is_empty() {
                        meta.author = text.trim().to_string();
                    } else if in_source_desc {
                        // Look for URLs in source description
                        if text.contains("http") {
                            let url_start = text.find("http").unwrap_or(0);
                            let url_end = text[url_start..]
                                .find(|c: char| !c.is_alphanumeric() && !"/:.?=&_-".contains(c))
                                .map(|i| url_start + i)
                                .unwrap_or(text.len());
                            let url = &text[url_start..url_end];
                            if url.starts_with("http") {
                                meta.source_url = url.to_string();
                            }
                        }
                    } else if in_availability {
                        current_text.push_str(&text);
                    }
                    buf.clear();
                }
                Ok(Event::End(e)) => {
                    let name_bytes = e.name().as_ref().to_vec();
                    let name = local_name(&name_bytes);
                    match name {
                        "teiHeader" => in_header = false,
                        "title" => in_title = false,
                        "author" => in_author = false,
                        "sourceDesc" => in_source_desc = false,
                        "availability" => {
                            in_availability = false;
                            if !current_text.is_empty() {
                                meta.rights_notes = collapse_whitespace(&current_text);
                                current_text.clear();
                            }
                        }
                        _ => {}
                    }
                    buf.clear();
                }
                Ok(Event::Eof) => break,
                Ok(_) => buf.clear(),
                Err(_) => break,
            }
        }
    }

    // For merged corpora, infer the per-file distribution from the filename:
    // a fascicle suffix (_NNN) means the file came from ISO, otherwise GitHub.
    let effective_dist = if distribution == CbetaDistribution::Merged {
        if let Some(stem) = xml_path.file_stem().and_then(|s| s.to_str()) {
            if strip_fascicle_suffix(stem) != stem {
                CbetaDistribution::IsoP5
            } else {
                CbetaDistribution::GitHubP5
            }
        } else {
            distribution
        }
    } else {
        distribution
    };

    // Classification: prefer the sidecar, fall back to heuristic.
    // For ISO fascicle files the sidecar is keyed on the GitHub-style
    // single-file path (e.g. T/T01/T01n0001.xml), so normalise the key.
    let sidecar_key = sidecar_key_for(rel_path);
    let classification_source = if let Some(entry) = sidecar.and_then(|s| s.lookup(&sidecar_key)) {
        meta.traditions = entry.traditions.clone();
        meta.period = entry.period.clone();
        meta.origin = entry.origin.clone();
        if let Some(name) = &entry.canon_name {
            if meta.canon_name.is_empty() {
                meta.canon_name = name.clone();
            }
        }
        if meta.main_title.is_empty() {
            if let Some(t) = &entry.main_title {
                meta.main_title = t.clone();
            }
        }
        if meta.author.is_empty() {
            if let Some(a) = &entry.author {
                meta.author = a.clone();
            }
        }
        "sidecar"
    } else {
        let text_content = format!("{} {} {}", meta.main_title, meta.author, meta.rights_notes);
        meta.traditions = classify_tradition_fallback(&text_content, &meta.canon, rel_path);
        meta.period = classify_period_fallback(&text_content);
        meta.origin = classify_origin_fallback(&text_content);
        "fallback"
    };
    meta.period_rank = period_rank(&meta.period);

    // Fill in any still-blank title/author/period from the work catalog.
    if let Some(cat) = catalog {
        if let Some(entry) = cat.get(&meta.source_work_id) {
            if meta.main_title.is_empty() {
                meta.main_title = entry.title.clone();
            }
            if meta.author.is_empty() || meta.period.is_empty() {
                let (cat_author, cat_period, cat_rank) =
                    crate::cbeta_sidecar::parse_catalog_translator(&entry.translator_field);
                if meta.author.is_empty() {
                    if let Some(a) = cat_author {
                        meta.author = a;
                    }
                }
                if meta.period.is_empty() {
                    if let Some(p) = cat_period {
                        meta.period = p;
                        meta.period_rank = cat_rank;
                    }
                }
            }
        }
    }

    // Stamp distribution + classification source on retrieval_method + quality_flags_json
    // so downstream catalog / dedup can distinguish github-p5 from ISO and audit
    // whether classification was authoritative or heuristic.
    if meta.retrieval_method.is_empty() {
        meta.retrieval_method = format!("cbeta-{}", effective_dist.as_str());
    }
    let flags = serde_json::json!({
        "distribution": effective_dist.as_str(),
        "classification_source": classification_source,
    });
    meta.quality_flags_json = flags.to_string();

    meta
}

/// Fallback tradition classifier — only used for works not present in
/// the `buddhist_metadata_analysis.json` sidecar. Two design changes vs
/// the previous version, both motivated by false-positive rates measured
/// against the sidecar ground truth:
///
/// 1. **論/史/傳/律 are demoted to *secondary* labels.** They match an
///    enormous fraction of CBETA titles (every `論` = "Commentarial",
///    every `傳` = "Historical") and were drowning out real school
///    membership in mixed-tradition works. We only emit Commentarial /
///    Historical / Vinaya when no primary school matched.
///
/// 2. **Canon + volume range prior for Chan/Zen.** The Taishō, X, and J
///    canons group Chan literature into well-known volume ranges. A work
///    landing in those ranges is Chan/Zen even when its title is
///    Sanskrit-transliterated and contains no 禪. This recovers most of
///    the 496 sidecar-tagged Chan works when the sidecar is absent.
fn classify_tradition_fallback(text: &str, canon: &str, rel_path: &str) -> Vec<String> {
    let text_lower = text.to_lowercase();
    let mut primary: Vec<String> = Vec::new();

    if text_lower.contains("禪")
        || text_lower.contains("禅")
        || text_lower.contains("chan")
        || text_lower.contains("zen")
        || is_chan_canon_range(canon, rel_path)
    {
        primary.push("Chan/Zen".to_string());
    }
    if text_lower.contains("淨土") || text_lower.contains("净土") || text_lower.contains("阿彌陀")
    {
        primary.push("Pure Land".to_string());
    }
    if text_lower.contains("天台") || text_lower.contains("法華") {
        primary.push("Tiantai".to_string());
    }
    if text_lower.contains("華嚴") || text_lower.contains("华严") {
        primary.push("Huayan".to_string());
    }
    if text_lower.contains("中觀") || text_lower.contains("中論") {
        primary.push("Madhyamaka".to_string());
    }
    if text_lower.contains("瑜伽") || text_lower.contains("唯識") {
        primary.push("Yogacara".to_string());
    }
    if text_lower.contains("密") || text_lower.contains("密教") {
        primary.push("Esoteric".to_string());
    }

    if !primary.is_empty() {
        return primary;
    }

    // No primary school detected — fall through to secondary labels.
    let mut secondary: Vec<String> = Vec::new();
    if text_lower.contains("律") || text_lower.contains("戒律") || text_lower.contains("毗奈耶")
    {
        secondary.push("Vinaya".to_string());
    }
    if text_lower.contains("註") || text_lower.contains("疏") || text_lower.contains("論") {
        secondary.push("Commentarial".to_string());
    }
    if text_lower.contains("史") || text_lower.contains("傳") {
        secondary.push("Historical".to_string());
    }
    if secondary.is_empty() {
        secondary.push("General/Unspecified".to_string());
    }
    secondary
}

/// Canon + volume ranges that the scholarly literature treats as
/// predominantly Chan/Zen. These match the bulk of the sidecar's
/// Chan/Zen tags (T 47–48 = 禪宗部, X 63–87 = 禪宗類續編, the entire
/// J canon = 嘉興藏, mostly Chan/Pure Land late-imperial collections).
fn is_chan_canon_range(canon: &str, rel_path: &str) -> bool {
    if canon == "J" {
        return true;
    }
    let vol = parse_volume_from_rel_path(rel_path);
    match canon {
        "T" => matches!(vol, Some(47..=48)),
        "X" => matches!(vol, Some(63..=87)),
        _ => false,
    }
}

/// `T/T47/T47n1985.xml` -> Some(47); `T/T47t/T47n1985.xml` -> Some(47).
fn parse_volume_from_rel_path(rel_path: &str) -> Option<u32> {
    let parts: Vec<&str> = rel_path.split('/').collect();
    if parts.len() < 2 {
        return None;
    }
    let canon = parts[0];
    let vol_dir = parts[1].trim_start_matches(canon);
    // Trim trailing non-digit suffix (e.g. ISO's "T47t").
    let digits: String = vol_dir.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn classify_period_fallback(text: &str) -> String {
    if text.contains("唐") {
        return "Tang".to_string();
    } else if text.contains("宋") {
        return "Song".to_string();
    } else if text.contains("元") {
        return "Yuan".to_string();
    } else if text.contains("明") {
        return "Ming".to_string();
    } else if text.contains("清") {
        return "Qing".to_string();
    } else if text.contains("漢") || text.contains("魏") || text.contains("晉") {
        return "Pre-Tang".to_string();
    } else if text.contains("隋") {
        return "Sui".to_string();
    } else if text.contains("民國") || text.contains("現代") {
        return "Modern".to_string();
    }
    "Unknown Period".to_string()
}

fn classify_origin_fallback(text: &str) -> String {
    if text.contains("印度") || text.contains("天竺") {
        return "India".to_string();
    } else if text.contains("西域") || text.contains("中亞") {
        return "Central Asia".to_string();
    } else if text.contains("中國") || text.contains("漢地") || text.contains("中土") {
        return "China".to_string();
    } else if text.contains("高麗") || text.contains("朝鮮") {
        return "Korea".to_string();
    } else if text.contains("日本") {
        return "Japan".to_string();
    }
    "Unknown Origin".to_string()
}

pub fn iter_xml_paths(corpus_root: &Path) -> Result<Vec<(PathBuf, String)>> {
    Ok(scan_cbeta_corpus(corpus_root)?.files)
}

/// Richer scan result: the resolved xml root, which distribution it
/// belongs to, and the (abs, rel) file list. Callers that need to know
/// "github xml-p5 vs ISO" or load a sidecar from a path relative to the
/// corpus should use this directly.
pub struct CbetaScan {
    pub xml_root: PathBuf,
    pub distribution: CbetaDistribution,
    pub files: Vec<(PathBuf, String)>,
}

pub fn scan_cbeta_corpus(corpus_root: &Path) -> Result<CbetaScan> {
    let (xml_root, distribution) = resolve_xml_root(corpus_root)?;

    let mut paths = Vec::new();
    for entry in WalkDir::new(&xml_root).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("xml") {
            continue;
        }
        let rel = path
            .strip_prefix(&xml_root)?
            .to_string_lossy()
            .replace('\\', "/");
        paths.push((path.to_path_buf(), rel));
    }

    paths.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(CbetaScan {
        xml_root,
        distribution,
        files: paths,
    })
}

/// Resolve the directory that contains CBETA XML content.
///
/// Accepts:
/// - A CBETA root containing `xml-p5/` (GitHub TEI, one file per work)
/// - A CBETA root containing `xml-iso/` (ISO TEI, one file per fascicle)
/// - Either of those subdirectories directly
/// - Any directory that already contains `.xml` files
pub fn resolve_xml_root(corpus_root: &Path) -> Result<(PathBuf, CbetaDistribution)> {
    for (sub, dist) in &[
        ("xml-p5", CbetaDistribution::GitHubP5),
        ("xml-iso", CbetaDistribution::IsoP5),
        ("xml-p5t", CbetaDistribution::P5Translated),
        ("xml-merged", CbetaDistribution::Merged),
    ] {
        let nested = corpus_root.join(sub);
        if nested.is_dir() {
            return Ok((nested, *dist));
        }
    }
    // Passed a leaf directory directly? Infer distribution from its name.
    if let Some(name) = corpus_root.file_name().and_then(|s| s.to_str()) {
        let dist = match name {
            "xml-p5" => CbetaDistribution::GitHubP5,
            "xml-iso" => CbetaDistribution::IsoP5,
            "xml-p5t" => CbetaDistribution::P5Translated,
            "xml-merged" => CbetaDistribution::Merged,
            _ => CbetaDistribution::Unknown,
        };
        if corpus_root.is_dir() && contains_xml_anywhere(corpus_root) {
            return Ok((corpus_root.to_path_buf(), dist));
        }
    }
    anyhow::bail!(
        "No CBETA XML content found under {}.\n  \
         Expected a directory containing `xml-p5/`, `xml-iso/`, `xml-p5t/`, or \
         `xml-merged/` (CBETA root) or one of those directories itself.",
        corpus_root.display()
    );
}

/// Compute the sidecar lookup key for `rel_path`.
///
/// The sidecar is indexed on GitHub-style single-file paths
/// (`T/T01/T01n0001.xml`). ISO fascicle paths (`T01n0001_001.xml`) won't
/// match directly, so strip the fascicle suffix from the stem before lookup.
fn sidecar_key_for(rel_path: &str) -> String {
    let p = Path::new(rel_path);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let stripped = strip_fascicle_suffix(stem);
    if stripped == stem {
        return rel_path.to_string();
    }
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("xml");
    match p.parent().and_then(|p| p.to_str()) {
        Some("") | None => format!("{stripped}.{ext}"),
        Some(parent) => format!("{parent}/{stripped}.{ext}"),
    }
}

/// Strip trailing `_NNN` fascicle suffix from CBETA ISO filenames.
/// `T01n0001_001` → `T01n0001`, `T01n0001` → `T01n0001` (unchanged).
/// Exported so `merge_cbeta` can use the same logic for work_id grouping.
pub(crate) fn strip_fascicle_suffix(stem: &str) -> &str {
    // Suffix is `_` followed by 3+ digits at the end of the stem.
    if let Some(idx) = stem.rfind('_') {
        let suffix = &stem[idx + 1..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            return &stem[..idx];
        }
    }
    stem
}

fn contains_xml_anywhere(dir: &Path) -> bool {
    WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .any(|e| e.path().extension().and_then(|s| s.to_str()) == Some("xml"))
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
                        if let Some(mut record) =
                            active_p.into_record(rel_path, meta, &mut previous_lb)
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
            from_lb,
            to_lb,
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
