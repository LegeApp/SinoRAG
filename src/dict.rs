//! Buddhist term dictionary for automatic tool-response annotation.
//!
//! Loads dictionary entries from `data/dict.parquet/` at first access,
//! caches them in a HashMap for O(1) lookup, and annotates every tool
//! response with `_term_context` for significant Buddhist terms.
//!
//! See THIRD_PARTY_NOTICES.md for dictionary source attributions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::sync::OnceCell;

#[derive(Debug, Clone)]
pub struct DictEntry {
    pub term: String,
    pub source: String,
    pub sanskrit: Option<String>,
    pub gloss: String,
}

pub struct DictStore {
    entries: HashMap<String, Vec<DictEntry>>,
    max_term_chars: usize,
}

impl DictStore {
    /// Load all dictionary entries from a parquet directory into a HashMap.
    pub async fn load(parquet_dir: &Path) -> anyhow::Result<Self> {
        use arrow::array::Array;
        use datafusion::prelude::*;

        let ctx = SessionContext::new();
        let source = parquet_dir
            .join("**/*.parquet")
            .to_string_lossy()
            .replace('\\', "/");

        ctx.register_parquet("dict", &source, ParquetReadOptions::default())
            .await?;

        let df = ctx.sql("SELECT term, source, sanskrit, gloss FROM dict").await?;
        let batches = df.collect().await?;

        let mut entries: HashMap<String, Vec<DictEntry>> = HashMap::new();
        let mut max_chars = 0usize;

        for batch in &batches {
            let term_col = batch
                .column_by_name("term")
                .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());
            let source_col = batch
                .column_by_name("source")
                .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());
            let sanskrit_col = batch
                .column_by_name("sanskrit")
                .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());
            let gloss_col = batch
                .column_by_name("gloss")
                .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());

            let (Some(terms), Some(sources), Some(sanskrits), Some(glosses)) =
                (term_col, source_col, sanskrit_col, gloss_col)
            else {
                continue;
            };

            for i in 0..batch.num_rows() {
                let term = terms.value(i).to_string();
                let char_len = term.chars().count();
                if char_len < 2 {
                    continue;
                }
                max_chars = max_chars.max(char_len);

                let entry = DictEntry {
                    term: term.clone(),
                    source: sources.value(i).to_string(),
                    sanskrit: if sanskrits.is_null(i) {
                        None
                    } else {
                        Some(sanskrits.value(i).to_string())
                    },
                    gloss: glosses.value(i).to_string(),
                };

                entries.entry(term).or_default().push(entry);
            }
        }

        eprintln!(
            "dict: loaded {} terms from {}",
            entries.len(),
            parquet_dir.display()
        );

        Ok(DictStore {
            entries,
            max_term_chars: max_chars,
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn get(&self, term: &str) -> Option<&[DictEntry]> {
        self.entries.get(term).map(|v| v.as_slice())
    }

    /// Extract Buddhist terms from `text`, ranked by specialization.
    ///
    /// Greedy longest-match scan. Scoring: term_length * 10 + (has_sanskrit ? 5 : 0).
    /// This naturally surfaces multi-character Buddhist jargon with Sanskrit
    /// equivalents — exactly the terms models misinterpret.
    pub fn extract_terms(&self, text: &str, limit: usize) -> Vec<&DictEntry> {
        let chars: Vec<char> = text.chars().collect();
        let mut matched: Vec<(&DictEntry, usize)> = Vec::new();
        let mut covered = vec![false; chars.len()];

        let max_len = self.max_term_chars.min(chars.len());
        for window in (2..=max_len).rev() {
            for start in 0..chars.len().saturating_sub(window - 1) {
                if covered[start] {
                    continue;
                }
                let candidate: String = chars[start..start + window].iter().collect();
                if let Some(entries) = self.entries.get(&candidate) {
                    for i in start..start + window {
                        covered[i] = true;
                    }
                    // Pick the entry with the best gloss (prefer Soothill for English,
                    // then any entry with Sanskrit).
                    let best = entries
                        .iter()
                        .min_by_key(|e| match e.source.as_str() {
                            "soothill" => 0,
                            _ if e.sanskrit.is_some() => 1,
                            _ => 2,
                        })
                        .unwrap();
                    matched.push((best, window));
                }
            }
        }

        matched.sort_by(|a, b| {
            let score_a = a.1 * 10 + if a.0.sanskrit.is_some() { 5 } else { 0 };
            let score_b = b.1 * 10 + if b.0.sanskrit.is_some() { 5 } else { 0 };
            score_b.cmp(&score_a)
        });

        matched.dedup_by(|a, b| a.0.term == b.0.term);
        matched.into_iter().take(limit).map(|(e, _)| e).collect()
    }
}

// ---------------------------------------------------------------------------
// Entity store — persons and places for auto-annotation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct EntityEntry {
    pub entity_type: &'static str, // "person" or "place"
    pub id: String,
    pub dynasty: Option<String>,  // persons only
    pub category: Option<String>, // places only
    pub summary: String,
}

pub struct EntityStore {
    /// name (primary + all alts) → Vec<EntityEntry> (may have both person + place)
    entries: HashMap<String, Vec<EntityEntry>>,
    max_name_chars: usize,
}

impl EntityStore {
    pub async fn load(persons_dir: Option<&std::path::Path>, places_dir: Option<&std::path::Path>) -> anyhow::Result<Self> {
        use arrow::array::Array;
        use datafusion::prelude::*;

        let ctx = SessionContext::new();
        let mut entries: HashMap<String, Vec<EntityEntry>> = HashMap::new();
        let mut max_chars = 0usize;

        // Load persons
        if let Some(dir) = persons_dir {
            if dir.is_dir() {
                let src = dir.join("**/*.parquet").to_string_lossy().replace('\\', "/");
                if ctx.register_parquet("persons", &src, ParquetReadOptions::default()).await.is_ok() {
                    let df = ctx.sql(
                        "SELECT person_id, primary_name, primary_name_lang, alt_names_json, dynasty, concise_bio FROM persons"
                    ).await?;
                    for batch in df.collect().await? {
                        let ids = batch.column_by_name("person_id")
                            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());
                        let names = batch.column_by_name("primary_name")
                            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());
                        let langs = batch.column_by_name("primary_name_lang")
                            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());
                        let alts = batch.column_by_name("alt_names_json")
                            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());
                        let dynasties = batch.column_by_name("dynasty")
                            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());
                        let bios = batch.column_by_name("concise_bio")
                            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());
                        let (Some(ids), Some(names), Some(langs), Some(alts), Some(dynasties), Some(bios)) =
                            (ids, names, langs, alts, dynasties, bios) else { continue };

                        for i in 0..batch.num_rows() {
                            let lang = langs.value(i);
                            let primary = names.value(i).to_string();
                            let id = ids.value(i).to_string();
                            let dynasty = if dynasties.is_null(i) { None } else { Some(dynasties.value(i).trim().to_string()) };
                            let summary = if bios.is_null(i) { String::new() } else { truncate_gloss(bios.value(i), 200) };

                            // Index CJK primary name if present
                            if (lang.starts_with("zho") || lang.starts_with("jpn")) && primary.chars().count() >= 2 {
                                let entry = EntityEntry {
                                    entity_type: "person",
                                    id: id.clone(),
                                    dynasty: dynasty.clone(),
                                    category: None,
                                    summary: summary.clone(),
                                };
                                max_chars = max_chars.max(primary.chars().count());
                                entries.entry(primary).or_default().push(entry);
                            }

                            // Always index CJK alt names (covers entries with non-CJK primary)
                            if let Ok(alts_vec) = serde_json::from_str::<Vec<String>>(alts.value(i)) {
                                for alt in alts_vec {
                                    let ch = alt.chars().count();
                                    if ch >= 2 && crate::normalize::contains_cjk(&alt) {
                                        max_chars = max_chars.max(ch);
                                        entries.entry(alt).or_default().push(EntityEntry {
                                            entity_type: "person",
                                            id: id.clone(),
                                            dynasty: dynasty.clone(),
                                            category: None,
                                            summary: summary.clone(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                    eprintln!("entity: loaded persons from {}", dir.display());
                }
            }
        }

        // Load places
        if let Some(dir) = places_dir {
            if dir.is_dir() {
                let src = dir.join("**/*.parquet").to_string_lossy().replace('\\', "/");
                if ctx.register_parquet("places", &src, ParquetReadOptions::default()).await.is_ok() {
                    let df = ctx.sql(
                        "SELECT place_id, primary_name, primary_name_lang, alt_names_json, category, description FROM places"
                    ).await?;
                    for batch in df.collect().await? {
                        let ids = batch.column_by_name("place_id")
                            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());
                        let names = batch.column_by_name("primary_name")
                            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());
                        let langs = batch.column_by_name("primary_name_lang")
                            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());
                        let alts = batch.column_by_name("alt_names_json")
                            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());
                        let cats = batch.column_by_name("category")
                            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());
                        let descs = batch.column_by_name("description")
                            .and_then(|c| c.as_any().downcast_ref::<arrow::array::StringArray>());
                        let (Some(ids), Some(names), Some(langs), Some(alts), Some(cats), Some(descs)) =
                            (ids, names, langs, alts, cats, descs) else { continue };

                        for i in 0..batch.num_rows() {
                            let lang = langs.value(i);
                            let primary = names.value(i).to_string();
                            let id = ids.value(i).to_string();
                            let category = if cats.is_null(i) { None } else { Some(cats.value(i).trim().to_string()) };
                            let summary = if descs.is_null(i) { String::new() } else { truncate_gloss(descs.value(i), 200) };

                            // Index CJK primary name if present
                            if (lang.starts_with("zho") || lang.starts_with("jpn")) && primary.chars().count() >= 2 {
                                let entry = EntityEntry {
                                    entity_type: "place",
                                    id: id.clone(),
                                    dynasty: None,
                                    category: category.clone(),
                                    summary: summary.clone(),
                                };
                                max_chars = max_chars.max(primary.chars().count());
                                entries.entry(primary).or_default().push(entry);
                            }

                            // Always index CJK alt names (covers entries with non-CJK primary)
                            if let Ok(alts_vec) = serde_json::from_str::<Vec<String>>(alts.value(i)) {
                                for alt in alts_vec {
                                    let ch = alt.chars().count();
                                    if ch >= 2 && crate::normalize::contains_cjk(&alt) {
                                        max_chars = max_chars.max(ch);
                                        entries.entry(alt).or_default().push(EntityEntry {
                                            entity_type: "place",
                                            id: id.clone(),
                                            dynasty: None,
                                            category: category.clone(),
                                            summary: summary.clone(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                    eprintln!("entity: loaded places from {}", dir.display());
                }
            }
        }

        eprintln!("entity: {} distinct name keys, max_chars={}", entries.len(), max_chars);
        Ok(EntityStore { entries, max_name_chars: max_chars })
    }

    /// Extract entity matches from `text`, up to `limit`. Returns all matching entities.
    /// Uses greedy longest-match on CJK names, gated to 2+ char names.
    pub fn extract_entities<'a>(&'a self, text: &str, limit: usize) -> Vec<&'a EntityEntry> {
        let chars: Vec<char> = text.chars().collect();
        let mut results: Vec<&EntityEntry> = Vec::new();
        let mut covered = vec![false; chars.len()];
        let max_len = self.max_name_chars.min(chars.len());

        'outer: for window in (2..=max_len).rev() {
            for start in 0..chars.len().saturating_sub(window - 1) {
                if covered[start] {
                    continue;
                }
                let candidate: String = chars[start..start + window].iter().collect();
                if let Some(entries) = self.entries.get(&candidate) {
                    for i in start..start + window {
                        covered[i] = true;
                    }
                    for e in entries {
                        results.push(e);
                        if results.len() >= limit {
                            break 'outer;
                        }
                    }
                }
            }
        }
        results
    }
}

// ---------------------------------------------------------------------------
// Global state — loaded once from parquet at first tool call
// ---------------------------------------------------------------------------

static DICT_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Set the dict parquet path before any tool calls. Called from ToolEngine::open.
pub fn set_dict_path(path: Option<PathBuf>) {
    let _ = DICT_PATH.set(path);
}

static PERSON_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
static PLACE_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

pub fn set_person_path(path: Option<PathBuf>) {
    let _ = PERSON_PATH.set(path);
}

pub fn set_place_path(path: Option<PathBuf>) {
    let _ = PLACE_PATH.set(path);
}

pub fn get_person_path() -> Option<PathBuf> {
    PERSON_PATH.get()?.clone()
}

pub fn get_place_path() -> Option<PathBuf> {
    PLACE_PATH.get()?.clone()
}

/// Lazy-loaded global EntityStore. Returns None if neither persons nor places parquet exist.
static ENTITY_STORE: OnceCell<Option<EntityStore>> = OnceCell::const_new();

async fn get_entity_store() -> &'static Option<EntityStore> {
    ENTITY_STORE
        .get_or_init(|| async {
            let persons = PERSON_PATH.get().and_then(|p| p.as_ref());
            let places = PLACE_PATH.get().and_then(|p| p.as_ref());
            if persons.is_none() && places.is_none() {
                return None;
            }
            match EntityStore::load(persons.map(|p| p.as_path()), places.map(|p| p.as_path())).await {
                Ok(store) => Some(store),
                Err(e) => {
                    eprintln!("warn: failed to load entity parquet: {e}");
                    None
                }
            }
        })
        .await
}

/// Lazy-loaded global DictStore. Returns None if dict.parquet doesn't exist.
static DICT_STORE: OnceCell<Option<DictStore>> = OnceCell::const_new();

async fn get_dict_store() -> &'static Option<DictStore> {
    DICT_STORE
        .get_or_init(|| async {
            let path = DICT_PATH.get().and_then(|p| p.as_ref());
            match path {
                Some(p) if p.is_dir() => match DictStore::load(p).await {
                    Ok(store) => Some(store),
                    Err(e) => {
                        eprintln!("warn: failed to load dict parquet: {e}");
                        None
                    }
                },
                _ => None,
            }
        })
        .await
}

// ---------------------------------------------------------------------------
// Tool response annotation
// ---------------------------------------------------------------------------

const MAX_ANNOTATIONS: usize = 8;
const MAX_GLOSS_CHARS: usize = 300;

/// Annotate a tool response JSON with `_term_context` and `_entity_context`.
///
/// No-op if the parquet stores are absent or the response isn't a JSON object.
pub async fn annotate_response(response: &mut serde_json::Value) {
    if !response.is_object() {
        return;
    }

    let text = collect_chinese_text(response);
    if text.is_empty() {
        return;
    }

    // Term annotations (Buddhist dictionary)
    if let Some(store) = get_dict_store().await {
        let terms = store.extract_terms(&text, MAX_ANNOTATIONS);
        if !terms.is_empty() {
            let annotations: Vec<serde_json::Value> = terms
                .iter()
                .map(|e| {
                    let mut obj = serde_json::json!({
                        "term": e.term,
                        "gloss": truncate_gloss(&e.gloss, MAX_GLOSS_CHARS),
                        "source": e.source,
                    });
                    if let Some(ref s) = e.sanskrit {
                        obj["sanskrit"] = serde_json::Value::String(s.clone());
                    }
                    obj
                })
                .collect();
            if let Some(obj) = response.as_object_mut() {
                obj.insert("_term_context".to_string(), serde_json::Value::Array(annotations));
            }
        }
    }

    // Entity annotations (persons + places)
    if let Some(store) = get_entity_store().await {
        let entities = store.extract_entities(&text, MAX_ANNOTATIONS);
        if !entities.is_empty() {
            // Deduplicate by (type, id)
            let mut seen = std::collections::HashSet::new();
            let annotations: Vec<serde_json::Value> = entities
                .into_iter()
                .filter(|e| seen.insert((e.entity_type, e.id.as_str())))
                .map(|e| {
                    let mut obj = serde_json::json!({
                        "type": e.entity_type,
                        "id": e.id,
                    });
                    if let Some(ref d) = e.dynasty {
                        obj["dynasty"] = serde_json::Value::String(d.clone());
                    }
                    if let Some(ref c) = e.category {
                        obj["category"] = serde_json::Value::String(c.clone());
                    }
                    if !e.summary.is_empty() {
                        obj["summary"] = serde_json::Value::String(e.summary.clone());
                    }
                    obj
                })
                .collect();
            if !annotations.is_empty() {
                if let Some(obj) = response.as_object_mut() {
                    obj.insert("_entity_context".to_string(), serde_json::Value::Array(annotations));
                }
            }
        }
    }
}

fn collect_chinese_text(value: &serde_json::Value) -> String {
    let mut buf = String::new();
    collect_text_recursive(value, &mut buf);
    buf
}

fn collect_text_recursive(value: &serde_json::Value, buf: &mut String) {
    match value {
        serde_json::Value::String(s) => {
            if crate::normalize::contains_cjk(s) {
                if !buf.is_empty() {
                    buf.push(' ');
                }
                buf.push_str(s);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                collect_text_recursive(v, buf);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, v) in map {
                if key.starts_with('_') {
                    continue;
                }
                collect_text_recursive(v, buf);
            }
        }
        _ => {}
    }
}

fn truncate_gloss(gloss: &str, max_chars: usize) -> String {
    if gloss.chars().count() <= max_chars {
        return gloss.to_string();
    }
    let truncated: String = gloss.chars().take(max_chars).collect();
    format!("{truncated}…")
}
