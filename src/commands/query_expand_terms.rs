//! `query-expand-terms`: produce variants/orthographic flips/aliases for
//! a seed phrase. Pure lookup against bundled tables — no LLM, no I/O.

use crate::jsonout::write_or_print;
use crate::templates::variants::VariantTables;
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
pub enum ExpandMode {
    Variants,
    Orthographic,
    Persons,
    All,
}

impl ExpandMode {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "variants" => ExpandMode::Variants,
            "orthographic" => ExpandMode::Orthographic,
            "persons" => ExpandMode::Persons,
            "all" => ExpandMode::All,
            other => anyhow::bail!(
                "unknown --mode `{other}`; expected variants|orthographic|persons|all"
            ),
        })
    }
}

pub fn run(
    phrase: String,
    mode: String,
    person_aliases: Vec<String>,
    max: usize,
    out: Option<PathBuf>,
) -> Result<()> {
    let m = ExpandMode::parse(&mode)?;
    let tables = VariantTables::load();

    let mut variants_bucket = BTreeSet::<String>::new();
    let mut orthographic_bucket = BTreeSet::<String>::new();
    let mut persons_bucket = BTreeSet::<String>::new();

    if matches!(m, ExpandMode::Variants | ExpandMode::All) {
        for v in tables.term_variants(&phrase) {
            if v != phrase {
                variants_bucket.insert(v);
            }
        }
    }
    if matches!(m, ExpandMode::Orthographic | ExpandMode::All) {
        for v in tables.orthographic_flips(&phrase, max * 2) {
            orthographic_bucket.insert(v);
        }
        // Also flip every term in the variants bucket so we cover cross-Han variants.
        let cur: Vec<String> = variants_bucket.iter().cloned().collect();
        for v in cur {
            for f in tables.orthographic_flips(&v, max) {
                if !variants_bucket.contains(&f) {
                    orthographic_bucket.insert(f);
                }
            }
        }
    }
    if matches!(m, ExpandMode::Persons | ExpandMode::All) {
        for a in &person_aliases {
            if !a.is_empty() && a != &phrase {
                persons_bucket.insert(a.clone());
            }
        }
    }

    // Combined view (deduped, capped).
    let mut combined: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    seen.insert(phrase.clone());
    for v in variants_bucket
        .iter()
        .chain(orthographic_bucket.iter())
        .chain(persons_bucket.iter())
    {
        if seen.insert(v.clone()) {
            combined.push(v.clone());
            if combined.len() >= max {
                break;
            }
        }
    }

    let payload = json!({
        "schema": "sinoragd-query-expand-terms-v1",
        "input": phrase,
        "expanded": combined,
        "by_source": {
            "variants": variants_bucket.into_iter().collect::<Vec<_>>(),
            "orthographic": orthographic_bucket.into_iter().collect::<Vec<_>>(),
            "persons": persons_bucket.into_iter().collect::<Vec<_>>(),
        },
        "search_strategy": {
            "mode": mode,
            "max": max,
            "input_lang_guess": detect_lang(&phrase),
        }
    });
    write_or_print(&payload, out)
}

fn detect_lang(s: &str) -> &'static str {
    let mut has_han = false;
    let mut has_latin = false;
    for ch in s.chars() {
        if (0x4E00..=0x9FFF).contains(&(ch as u32))
            || (0x3400..=0x4DBF).contains(&(ch as u32))
            || (0xF900..=0xFAFF).contains(&(ch as u32))
        {
            has_han = true;
        }
        if ch.is_ascii_alphabetic() {
            has_latin = true;
        }
    }
    match (has_han, has_latin) {
        (true, false) => "zh",
        (false, true) => "en",
        (true, true) => "mixed",
        _ => "unknown",
    }
}

// Allow Value to be silently used; keeps the import live when expanded paths grow.
#[allow(dead_code)]
fn _unused(_: Value) {}
