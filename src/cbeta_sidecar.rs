//! Loader for `buddhist_metadata_analysis.json` — the authoritative
//! per-work classification of the CBETA corpus produced by the
//! `CBETA_Sorting_Data` analysis scripts.
//!
//! Schema (the only field this module reads):
//! ```json
//! { "detailed_analysis": [ {
//!     "file": "/abs/path/.../xml-p5/T/T01/T01n0001.xml",
//!     "canon": "T",
//!     "canon_name": "Taishō Tripiṭaka",
//!     "traditions": ["Chan/Zen", ...],
//!     "period": "Tang",
//!     "origin": "China",
//!     "author": "...",
//!     "main_title": "..."
//! }, ... ] }
//! ```
//!
//! Each entry's `file` is normalized to a rel-path key of the form
//! `T/T01/T01n0001.xml` — identical to what `tei::iter_xml_paths`
//! returns as its second tuple element, so lookups are O(1) drop-in.

use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The authoritative classification table, embedded into the binary at
/// compile time. Sourced from `CBETA-Translator/CBETA_Sorting_Data/` and
/// copied to `assets/cbeta/` in this repo so a built `sinorag` runs with
/// zero external setup. Refresh by overwriting that file and rebuilding.
const EMBEDDED_BYTES: &[u8] = include_bytes!("../assets/cbeta/buddhist_metadata_analysis.json");

static EMBEDDED: OnceLock<SidecarIndex> = OnceLock::new();

/// Get the embedded sidecar (parsed once, cached for the process lifetime).
/// Panics only if the bundled JSON is corrupt — that would be a build-time
/// bug we want to surface loudly, not paper over.
pub fn embedded() -> &'static SidecarIndex {
    EMBEDDED.get_or_init(|| {
        load_from_bytes(EMBEDDED_BYTES)
            .expect("embedded buddhist_metadata_analysis.json failed to parse")
    })
}

#[derive(Debug, Clone)]
pub struct SidecarEntry {
    pub traditions: Vec<String>,
    pub period: String,
    pub origin: String,
    pub canon_name: Option<String>,
    pub author: Option<String>,
    pub main_title: Option<String>,
}

#[derive(Debug, Default)]
pub struct SidecarIndex {
    map: HashMap<String, SidecarEntry>,
    pub source_path: Option<PathBuf>,
}

impl SidecarIndex {
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Look up a rel_path like `T/T01/T01n0001.xml`. For ISO per-fascicle
    /// files like `T/T01/T01n0001_001.xml`, also try the work-level key
    /// by stripping the trailing `_NNN` suffix from the file stem.
    pub fn lookup(&self, rel_path: &str) -> Option<&SidecarEntry> {
        if let Some(e) = self.map.get(rel_path) {
            return Some(e);
        }
        // Fascicle fallback: T/T01/T01n0001_001.xml -> T/T01/T01n0001.xml
        let (dir, file) = rel_path.rsplit_once('/')?;
        let (stem, ext) = file.rsplit_once('.')?;
        let work_stem = strip_fascicle_suffix(stem);
        if work_stem == stem {
            return None;
        }
        let key = format!("{dir}/{work_stem}.{ext}");
        self.map.get(&key)
    }
}

fn strip_fascicle_suffix(stem: &str) -> &str {
    if let Some(idx) = stem.rfind('_') {
        let suffix = &stem[idx + 1..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            return &stem[..idx];
        }
    }
    stem
}

/// Search for `CBETA_Sorting_Data/buddhist_metadata_analysis.json` near
/// the corpus root and load it. Searches, in order:
///   <corpus_root>/CBETA_Sorting_Data/
///   <corpus_root>/../CBETA_Sorting_Data/
///   <corpus_root>/../../CBETA_Sorting_Data/
///   <corpus_root>/sorting/                (in-archive convention)
///   <corpus_root>/buddhist_metadata_analysis.json   (drop-in)
///
/// Returns `None` (silently) if no sidecar is found; ingest then falls
/// back to the heuristic classifier in `tei.rs`.
pub fn discover_and_load(corpus_root: &Path) -> Option<SidecarIndex> {
    let candidates = candidate_paths(corpus_root);
    for path in &candidates {
        if path.is_file() {
            match load_from_file(path) {
                Ok(idx) => return Some(idx),
                Err(e) => {
                    eprintln!(
                        "warn: failed to parse CBETA sidecar at {}: {}",
                        path.display(),
                        e
                    );
                }
            }
        }
    }
    None
}

fn candidate_paths(corpus_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let filename = "buddhist_metadata_analysis.json";
    let mut push_dir = |dir: PathBuf| {
        out.push(dir.join("CBETA_Sorting_Data").join(filename));
        out.push(dir.join("sorting").join(filename));
        out.push(dir.join(filename));
    };
    push_dir(corpus_root.to_path_buf());
    if let Some(p) = corpus_root.parent() {
        push_dir(p.to_path_buf());
        if let Some(pp) = p.parent() {
            push_dir(pp.to_path_buf());
        }
    }
    out
}

#[derive(Debug, Deserialize)]
struct RawFile {
    detailed_analysis: Vec<RawEntry>,
}

#[derive(Debug, Deserialize)]
struct RawEntry {
    file: String,
    #[serde(default)]
    canon_name: Option<String>,
    #[serde(default)]
    traditions: Vec<String>,
    #[serde(default)]
    period: String,
    #[serde(default)]
    origin: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    main_title: Option<String>,
}

pub fn load_from_file(path: &Path) -> anyhow::Result<SidecarIndex> {
    let bytes = fs::read(path)?;
    let mut idx = load_from_bytes(&bytes)?;
    idx.source_path = Some(path.to_path_buf());
    Ok(idx)
}

pub fn load_from_bytes(bytes: &[u8]) -> anyhow::Result<SidecarIndex> {
    let raw: RawFile = serde_json::from_slice(bytes)?;
    let mut map = HashMap::with_capacity(raw.detailed_analysis.len());
    for e in raw.detailed_analysis {
        let key = normalize_file_key(&e.file);
        if key.is_empty() {
            continue;
        }
        map.insert(
            key,
            SidecarEntry {
                traditions: e.traditions,
                period: e.period,
                origin: e.origin,
                canon_name: e.canon_name,
                author: e.author,
                main_title: e.main_title,
            },
        );
    }
    Ok(SidecarIndex {
        map,
        source_path: None,
    })
}

/// `/abs/path/.../xml-p5/T/T01/T01n0001.xml` -> `T/T01/T01n0001.xml`
/// Also accepts already-relative or windows-style paths.
fn normalize_file_key(file: &str) -> String {
    let s = file.replace('\\', "/");
    // Anchor on the canonical CBETA XML subdir name; the sorting data was
    // generated from an xml-p5 tree but the rel-path part is identical for
    // xml-iso and xml-p5t.
    for anchor in ["/xml-p5/", "/xml-iso/", "/xml-p5t/"] {
        if let Some(idx) = s.find(anchor) {
            return s[idx + anchor.len()..].to_string();
        }
    }
    // Already a rel-path? Take last 3 components (CANON/VOL/FILE.xml).
    let parts: Vec<&str> = s.trim_start_matches('/').split('/').collect();
    if parts.len() >= 3 {
        let tail = &parts[parts.len() - 3..];
        return tail.join("/");
    }
    s
}

// ---------------------------------------------------------------------------
// Work catalog: sutra_sch.lst — authoritative list of every CBETA work.
// Source: https://github.com/zhaowenping/cbeta (idx/sutra_sch.lst)
// ---------------------------------------------------------------------------

const CATALOG_BYTES: &[u8] = include_bytes!("../assets/cbeta/sutra_sch.lst");

static CATALOG: OnceLock<WorkCatalog> = OnceLock::new();

pub fn work_catalog() -> &'static WorkCatalog {
    CATALOG.get_or_init(|| parse_work_catalog(CATALOG_BYTES))
}

#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub work_id: String,
    pub title: String,
    pub juan_count: Option<u32>,
    pub translator_field: String,
}

#[derive(Debug, Default)]
pub struct WorkCatalog {
    entries: HashMap<String, CatalogEntry>,
}

impl WorkCatalog {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn get(&self, work_id: &str) -> Option<&CatalogEntry> {
        self.entries.get(work_id)
    }

    pub fn work_ids(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }
}

/// Parse `sutra_sch.lst` — one work per line.
///
/// Format: `{work_id} {title} ({juan}卷)【{dynasty} {translator}】`
///
/// Lines with fascicle suffixes (`_NNN`) or anchors (`#pNNNNaNNN`) are
/// folded into their parent work_id so the catalog has one entry per work.
fn parse_work_catalog(bytes: &[u8]) -> WorkCatalog {
    let text = String::from_utf8_lossy(bytes);
    let mut entries: HashMap<String, CatalogEntry> = HashMap::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((raw_id, rest)) = line.split_once(' ') else {
            continue;
        };

        // Strip anchor (#pNNNN...) then fascicle suffix (_NNN).
        let id_no_anchor = raw_id.split('#').next().unwrap_or(raw_id);
        let work_id = crate::tei::strip_fascicle_suffix(id_no_anchor);

        // Only keep the first (most complete) entry per work_id.
        if entries.contains_key(work_id) {
            continue;
        }

        let (title, juan, translator) = parse_catalog_fields(rest);

        entries.insert(
            work_id.to_string(),
            CatalogEntry {
                work_id: work_id.to_string(),
                title,
                juan_count: juan,
                translator_field: translator,
            },
        );
    }

    WorkCatalog { entries }
}

/// Extract title, juan count, and translator from the rest of a catalog line.
///
/// Example input: `長阿含經 (22卷)【後秦 佛陀耶舍共竺佛念譯】`
fn parse_catalog_fields(rest: &str) -> (String, Option<u32>, String) {
    let mut title = String::new();
    let mut juan: Option<u32> = None;
    let mut translator = String::new();

    // Find `(N卷)` and `【...】` by scanning for the delimiters.
    if let Some(paren_start) = rest.find('(') {
        title = rest[..paren_start].trim().to_string();
        if let Some(juan_end) = rest[paren_start..].find('卷') {
            let juan_str = &rest[paren_start + '('.len_utf8()..paren_start + juan_end];
            juan = juan_str.trim().parse().ok();
        }
    }

    if let Some(bracket_start) = rest.find('【') {
        if let Some(bracket_end) = rest.find('】') {
            translator = rest[bracket_start + '【'.len_utf8()..bracket_end]
                .trim()
                .to_string();
        }
    }

    if title.is_empty() {
        // No `(N卷)` found — title is the entire rest minus any 【】 block.
        let end = rest.find('【').unwrap_or(rest.len());
        title = rest[..end].trim().to_string();
    }

    (title, juan, translator)
}

// ---------------------------------------------------------------------------
// Catalog translator parsing: shared by tei.rs and patch_metadata.rs
// ---------------------------------------------------------------------------

/// Parse `translator_field` from sutra_sch.lst into (author, period, period_rank).
///
/// Format examples:
///   "後秦 佛陀耶舍共竺佛念譯"  →  dynasty="後秦", author="佛陀耶舍共竺佛念"
///   "唐 不空譯"               →  dynasty="唐", author="不空"
///   "失譯"                    →  no dynasty, author="失譯"
///   "黃謹良譯"                →  no dynasty (modern), author="黃謹良"
///   ""                        →  nothing
pub fn parse_catalog_translator(field: &str) -> (Option<String>, Option<String>, i32) {
    let field = field.trim();
    if field.is_empty() {
        return (None, None, 99);
    }

    // Conventional "no known translator" strings — preserve as-is.
    const ANONYMOUS: &[&str] = &["失譯", "佚名", "失名", "不詳"];
    if ANONYMOUS.contains(&field) {
        return (Some(field.to_string()), None, 99);
    }

    let (dynasty_raw, translator_raw) = if let Some(sp) = field.find(' ') {
        (&field[..sp], field[sp + 1..].trim())
    } else {
        ("", field)
    };

    let (period, rank) = dynasty_to_english(dynasty_raw);
    let dynasty_known = period.is_some();

    let translator_str = if dynasty_known { translator_raw } else { field };

    let author = strip_translation_verb(translator_str);
    let author = if author.is_empty() {
        None
    } else {
        Some(author.to_string())
    };

    (author, period, rank)
}

/// Map a Chinese dynasty name to an English period string + sort rank.
pub fn dynasty_to_english(d: &str) -> (Option<String>, i32) {
    let (p, r) = match d {
        "前漢" | "後漢" | "東漢" | "漢" => ("Han", 1),
        "吳" | "曹魏" | "蜀" | "魏" => ("Three Kingdoms", 2),
        "晉" | "晉世" | "西晉" | "東晉" => ("Jin", 3),
        "劉宋" | "南齊" | "蕭齊" | "梁" | "陳" | "南北朝" | "北涼" | "北魏" | "元魏" | "後魏"
        | "東魏" | "西魏" | "北齊" | "高齊" | "北周" | "宇文周" | "後秦" | "姚秦" | "前秦"
        | "符秦" | "乞伏秦" | "西秦" | "前涼" | "後趙" | "後燕" | "前燕" | "南涼" | "胡" => {
            ("Northern and Southern", 4)
        }
        "隋" => ("Sui", 5),
        "唐" | "南唐" => ("Tang", 6),
        "五代" | "後唐" | "後梁" | "後晉" | "後周" | "南漢" | "吳越" => {
            ("Five Dynasties", 7)
        }
        "宋" | "北宋" | "南宋" | "唐宋" | "遼" | "金" => ("Song", 8),
        "元" => ("Yuan", 9),
        "明" => ("Ming", 10),
        "清" => ("Qing", 11),
        "民國" | "近代" | "現代" => ("Modern", 99),
        _ => return (None, 99),
    };
    (Some(p.to_string()), r)
}

/// Strip trailing translation/authorship verb characters.
pub fn strip_translation_verb(s: &str) -> &str {
    const VERBS: &[char] = &[
        '譯', '撰', '著', '編', '纂', '記', '錄', '疏', '注', '解', '造', '集', '製', '述', '說',
        '釋',
    ];
    let s = s.trim_end_matches(|c| VERBS.contains(&c));
    let s = if s.ends_with('等') {
        &s[..s.len() - '等'.len_utf8()]
    } else {
        s
    };
    s.trim()
}

// ---------------------------------------------------------------------------
// Cross-canon parallel works: cmp.lst
// Source: https://github.com/zhaowenping/cbeta (idx/cmp.lst)
// ---------------------------------------------------------------------------

const CMP_BYTES: &[u8] = include_bytes!("../assets/cbeta/cmp.lst");

static PARALLEL_WORKS: OnceLock<ParallelWorksIndex> = OnceLock::new();

pub fn parallel_works() -> &'static ParallelWorksIndex {
    PARALLEL_WORKS.get_or_init(|| parse_parallel_works(CMP_BYTES))
}

#[derive(Debug, Default)]
pub struct ParallelWorksIndex {
    map: HashMap<String, Vec<String>>,
}

impl ParallelWorksIndex {
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Return the work_ids of parallel editions for `work_id`.
    /// Does NOT include `work_id` itself.
    pub fn parallels(&self, work_id: &str) -> Option<&[String]> {
        self.map.get(work_id).map(|v| v.as_slice())
    }
}

/// Parse `cmp.lst`. Each line starts with `{work_id} {title} ({juan}卷)...`,
/// optionally followed by tab-separated or space-separated cross-references
/// after the 【】 block.
///
/// A cross-reference is anything that looks like a CBETA work_id
/// (`{CANON}{TOME}n{SUTRA}` possibly with `_NNN#...` suffixes).
/// We strip suffixes and anchors to get the base work_id.
fn parse_parallel_works(bytes: &[u8]) -> ParallelWorksIndex {
    let text = String::from_utf8_lossy(bytes);
    let mut map: HashMap<String, Vec<String>> = HashMap::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Some((raw_id, rest)) = line.split_once(' ') else {
            continue;
        };
        let work_id = normalize_cmp_id(raw_id);
        if work_id.is_empty() {
            continue;
        }

        // The cross-references follow the 【translator】 block or, if absent,
        // after the title/(卷) section. Look for them as tokens that match
        // the CBETA id pattern.
        let refs_start = rest
            .find('】')
            .map(|i| i + '】'.len_utf8())
            .or_else(|| rest.find(')').map(|i| i + 1))
            .unwrap_or(rest.len());
        let refs_part = &rest[refs_start..];

        let mut refs: Vec<String> = Vec::new();
        for token in refs_part.split([',', ' ', '\t']) {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            let ref_id = normalize_cmp_id(token);
            if !ref_id.is_empty() && ref_id != work_id {
                if !refs.contains(&ref_id) {
                    refs.push(ref_id);
                }
            }
        }

        if !refs.is_empty() {
            map.insert(work_id, refs);
        }
    }

    ParallelWorksIndex { map }
}

fn normalize_cmp_id(raw: &str) -> String {
    let no_anchor = raw.split('#').next().unwrap_or(raw);
    crate::tei::strip_fascicle_suffix(no_anchor).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_distribution_prefix() {
        assert_eq!(
            normalize_file_key("/mnt/d/foo/xml-p5/T/T01/T01n0001.xml"),
            "T/T01/T01n0001.xml"
        );
        assert_eq!(
            normalize_file_key("/x/xml-iso/J/J01/J01nA001.xml"),
            "J/J01/J01nA001.xml"
        );
    }

    #[test]
    fn normalize_handles_already_relative() {
        assert_eq!(
            normalize_file_key("T/T01/T01n0001.xml"),
            "T/T01/T01n0001.xml"
        );
    }

    #[test]
    fn catalog_parses_standard_entry() {
        let (title, juan, translator) =
            parse_catalog_fields("長阿含經 (22卷)【後秦 佛陀耶舍共竺佛念譯】");
        assert_eq!(title, "長阿含經");
        assert_eq!(juan, Some(22));
        assert_eq!(translator, "後秦 佛陀耶舍共竺佛念譯");
    }

    #[test]
    fn catalog_parses_lost_translator() {
        let (title, juan, translator) = parse_catalog_fields("般泥洹經 (2卷)【失譯】");
        assert_eq!(title, "般泥洹經");
        assert_eq!(juan, Some(2));
        assert_eq!(translator, "失譯");
    }

    #[test]
    fn embedded_catalog_loads() {
        let cat = work_catalog();
        assert!(cat.len() > 4000, "expected >4000 works, got {}", cat.len());
        let entry = cat.get("T01n0001").expect("T01n0001 should be in catalog");
        assert_eq!(entry.title, "長阿含經");
        assert_eq!(entry.juan_count, Some(22));
    }

    #[test]
    fn parallel_works_loads() {
        let pw = parallel_works();
        assert!(
            pw.len() > 50,
            "expected >50 entries with parallels, got {}",
            pw.len()
        );
        // T01n0005 (佛般泥洹經) has parallels T01n0006, T01n0007, etc.
        let refs = pw
            .parallels("T01n0005")
            .expect("T01n0005 should have parallels");
        assert!(refs.contains(&"T01n0006".to_string()));
        assert!(refs.contains(&"T01n0007".to_string()));
    }

    #[test]
    fn fascicle_fallback() {
        let mut map = HashMap::new();
        map.insert(
            "T/T01/T01n0001.xml".to_string(),
            SidecarEntry {
                traditions: vec!["Chan/Zen".to_string()],
                period: "Tang".to_string(),
                origin: "China".to_string(),
                canon_name: None,
                author: None,
                main_title: None,
            },
        );
        let idx = SidecarIndex {
            map,
            source_path: None,
        };
        assert!(idx.lookup("T/T01/T01n0001_005.xml").is_some());
        assert!(idx.lookup("T/T01/T01n9999.xml").is_none());
    }
}
