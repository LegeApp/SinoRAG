//! Bundled variant tables for `query-expand-terms` and `variant-form-search`.
//! Tables ship in the binary via `include_str!`.

use serde::Deserialize;
use std::collections::BTreeMap;

const TERMS_JSON: &str = include_str!("buddhist_terms.json");
const ORTHO_JSON: &str = include_str!("orthographic.json");

#[derive(Debug, Deserialize)]
struct TermsFile {
    #[allow(dead_code)] schema: String,
    entries: Vec<TermEntry>,
}
#[derive(Debug, Deserialize)]
struct TermEntry {
    key: String,
    variants: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OrthoFile {
    #[allow(dead_code)] schema: String,
    pairs: Vec<(String, String)>,
}

/// Singleton-style loaders. Cheap (parse once per process).
pub struct VariantTables {
    pub terms: BTreeMap<String, Vec<String>>,
    /// Trad ↔ simp per-char map; both directions populated.
    pub orthographic_chars: BTreeMap<char, char>,
}

impl VariantTables {
    pub fn load() -> Self {
        let mut terms: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if let Ok(f) = serde_json::from_str::<TermsFile>(TERMS_JSON) {
            for e in f.entries {
                terms.insert(e.key, e.variants);
            }
        }
        let mut orthographic_chars: BTreeMap<char, char> = BTreeMap::new();
        if let Ok(f) = serde_json::from_str::<OrthoFile>(ORTHO_JSON) {
            for (a, b) in f.pairs {
                if let (Some(ca), Some(cb)) = (a.chars().next(), b.chars().next()) {
                    orthographic_chars.entry(ca).or_insert(cb);
                    orthographic_chars.entry(cb).or_insert(ca);
                }
            }
        }
        VariantTables { terms, orthographic_chars }
    }

    /// Look up bundled term variants.
    pub fn term_variants(&self, key: &str) -> Vec<String> {
        self.terms.get(key).cloned().unwrap_or_default()
    }

    /// Generate orthographic flips of `s`: for each Han char that has a known
    /// counterpart, produce a single-substitution variant. Combinatorial
    /// expansion across all known chars is intentionally avoided (would
    /// explode). Returns up to `max` distinct results.
    pub fn orthographic_flips(&self, s: &str, max: usize) -> Vec<String> {
        let chars: Vec<char> = s.chars().collect();
        let mut out: Vec<String> = Vec::new();
        for (i, ch) in chars.iter().enumerate() {
            if let Some(&alt) = self.orthographic_chars.get(ch) {
                if alt == *ch { continue; }
                let mut new_chars = chars.clone();
                new_chars[i] = alt;
                let v: String = new_chars.iter().collect();
                if !out.contains(&v) {
                    out.push(v);
                    if out.len() >= max { break; }
                }
            }
        }
        out
    }
}
