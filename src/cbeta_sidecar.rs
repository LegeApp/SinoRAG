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
const EMBEDDED_BYTES: &[u8] =
    include_bytes!("../assets/cbeta/buddhist_metadata_analysis.json");

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
