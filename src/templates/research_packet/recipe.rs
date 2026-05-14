//! Recipe loading: bundled named recipes (via `include_str!`) plus
//! `--recipe path/to/custom.json` for one-offs.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

pub const RECIPE_SCHEMA: &str = "sinorag-recipe-v1";

const BUILTIN_ACADEMIC_DEFAULT: &str = include_str!("recipes/academic-default.json");
const BUILTIN_PHRASE_FOCUSED: &str = include_str!("recipes/phrase-focused.json");
const BUILTIN_FULL_GENEALOGY: &str = include_str!("recipes/full-genealogy.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    pub schema: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_full_work_threshold")]
    pub full_work_threshold: usize,
    #[serde(default = "default_context_before")]
    pub context_before: usize,
    #[serde(default = "default_context_after")]
    pub context_after: usize,
    pub steps: Vec<RecipeStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeStep {
    pub tool: String,
    pub when: WhenFilter,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WhenFilter {
    Phrase,
    Passage,
    Person,
    Work,
    Canon,
    Period,
    AnyHit,
    AnyWork,
    AnySeed,
}

fn default_full_work_threshold() -> usize {
    5
}
fn default_context_before() -> usize {
    5
}
fn default_context_after() -> usize {
    5
}

impl Recipe {
    /// Resolve `name_or_path`: a bundled name like `academic-default`,
    /// or a path to a JSON file (must end with `.json`).
    pub fn load(name_or_path: &str) -> Result<Self> {
        let raw = if name_or_path.ends_with(".json") {
            let path = Path::new(name_or_path);
            std::fs::read_to_string(path)
                .with_context(|| format!("read recipe {}", path.display()))?
        } else {
            match name_or_path {
                "academic-default" => BUILTIN_ACADEMIC_DEFAULT.to_string(),
                "phrase-focused"   => BUILTIN_PHRASE_FOCUSED.to_string(),
                "full-genealogy"   => BUILTIN_FULL_GENEALOGY.to_string(),
                other => return Err(anyhow!(
                    "unknown recipe `{other}`. Built-ins: academic-default, phrase-focused, full-genealogy. Or pass a path ending in .json."
                )),
            }
        };
        let recipe: Recipe =
            serde_json::from_str(&raw).with_context(|| format!("parse recipe `{name_or_path}`"))?;
        if recipe.schema != RECIPE_SCHEMA {
            return Err(anyhow!(
                "recipe schema `{}` (expected `{}`)",
                recipe.schema,
                RECIPE_SCHEMA
            ));
        }
        Ok(recipe)
    }

    /// Names of all bundled recipes (useful for `--list-recipes` UX later).
    pub fn builtin_names() -> &'static [&'static str] {
        &["academic-default", "phrase-focused", "full-genealogy"]
    }
}

impl WhenFilter {
    /// Should this step run for a seed of the given `kind` (one of
    /// `phrase|passage|person|work|canon|period`)?
    pub fn matches_seed_kind(&self, kind: &str) -> bool {
        match self {
            WhenFilter::AnySeed => true,
            WhenFilter::AnyHit | WhenFilter::AnyWork => false, // fan-out, not seed-driven
            WhenFilter::Phrase => kind == "phrase",
            WhenFilter::Passage => kind == "passage",
            WhenFilter::Person => kind == "person",
            WhenFilter::Work => kind == "work",
            WhenFilter::Canon => kind == "canon",
            WhenFilter::Period => kind == "period",
        }
    }
}
