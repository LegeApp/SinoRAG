//! Research brief: what the receiving agent will work from. Parsed either
//! from a `--brief <file>.json` or composed from individual CLI flags.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

pub const BRIEF_SCHEMA: &str = "sinoragd-research-brief-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Brief {
    pub schema: String,
    pub topic: String,
    #[serde(default)]
    pub notes: String,
    pub seeds: Vec<Seed>,
    #[serde(default)]
    pub filters: BriefFilters,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Seed {
    Phrase { value: String },
    Passage { value: String },
    Person { name: String, #[serde(default)] aliases: Vec<String> },
    Work { value: String },
    Canon { value: String },
    Period { value: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BriefFilters {
    #[serde(default)]
    pub include_canons: Vec<String>,
    #[serde(default)]
    pub exclude_periods: Vec<String>,
    #[serde(default)]
    pub max_per_seed: Option<usize>,
}

impl Seed {
    pub fn kind(&self) -> &'static str {
        match self {
            Seed::Phrase { .. } => "phrase",
            Seed::Passage { .. } => "passage",
            Seed::Person { .. } => "person",
            Seed::Work { .. } => "work",
            Seed::Canon { .. } => "canon",
            Seed::Period { .. } => "period",
        }
    }
    pub fn slug(&self) -> String {
        let raw = match self {
            Seed::Phrase { value } => value.clone(),
            Seed::Passage { value } => value.clone(),
            Seed::Person { name, .. } => name.clone(),
            Seed::Work { value } => value.clone(),
            Seed::Canon { value } => value.clone(),
            Seed::Period { value } => value.clone(),
        };
        slugify(&raw)
    }
}

impl Brief {
    /// Load and validate a brief from a JSON file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let value: Value = serde_json::from_slice(&bytes)?;
        let mut brief: Brief = serde_json::from_value(value)
            .with_context(|| format!("parse brief {}", path.display()))?;
        if brief.schema != BRIEF_SCHEMA {
            return Err(anyhow!(
                "brief schema `{}` (expected `{}`)",
                brief.schema, BRIEF_SCHEMA
            ));
        }
        if brief.seeds.is_empty() {
            return Err(anyhow!("brief has no seeds"));
        }
        if brief.topic.trim().is_empty() {
            // Synthesize a topic from the first seed if user omitted one.
            brief.topic = brief.seeds[0].slug();
        }
        Ok(brief)
    }

    /// Compose a brief from individual CLI flags (each may contribute one seed).
    pub fn from_flags(args: BriefArgs) -> Result<Self> {
        let mut seeds = Vec::new();
        if let Some(v) = args.phrase { seeds.push(Seed::Phrase { value: v }); }
        if let Some(v) = args.seed_passage { seeds.push(Seed::Passage { value: v }); }
        if let Some(v) = args.work { seeds.push(Seed::Work { value: v }); }
        if let Some(v) = args.canon { seeds.push(Seed::Canon { value: v }); }
        if let Some(v) = args.period { seeds.push(Seed::Period { value: v }); }
        if let Some(name) = args.person {
            seeds.push(Seed::Person { name, aliases: args.person_alias });
        }
        if seeds.is_empty() {
            return Err(anyhow!(
                "no seeds supplied. Pass at least one of --phrase / --seed-passage / --work / --canon / --period / --person, or use --brief <file>."
            ));
        }
        let topic = args.topic.unwrap_or_else(|| seeds[0].slug());
        Ok(Brief {
            schema: BRIEF_SCHEMA.to_string(),
            topic,
            notes: args.notes.unwrap_or_default(),
            seeds,
            filters: BriefFilters::default(),
        })
    }

    pub fn render_markdown(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("# Brief: {}\n\n", self.topic));
        if !self.notes.is_empty() {
            s.push_str(&format!("> {}\n\n", self.notes));
        }
        s.push_str("## Seeds\n\n");
        for seed in &self.seeds {
            match seed {
                Seed::Phrase { value }  => s.push_str(&format!("- **phrase**: `{value}`\n")),
                Seed::Passage { value } => s.push_str(&format!("- **passage**: `{value}`\n")),
                Seed::Person { name, aliases } => {
                    s.push_str(&format!("- **person**: {name}"));
                    if !aliases.is_empty() {
                        s.push_str(&format!(" (aliases: {})", aliases.join(", ")));
                    }
                    s.push('\n');
                }
                Seed::Work { value }   => s.push_str(&format!("- **work**: `{value}`\n")),
                Seed::Canon { value }  => s.push_str(&format!("- **canon**: `{value}`\n")),
                Seed::Period { value } => s.push_str(&format!("- **period**: `{value}`\n")),
            }
        }
        s
    }
}

/// Flags accepted on the CLI when not using `--brief`.
#[derive(Debug, Default, Clone)]
pub struct BriefArgs {
    pub topic: Option<String>,
    pub notes: Option<String>,
    pub phrase: Option<String>,
    pub seed_passage: Option<String>,
    pub person: Option<String>,
    pub person_alias: Vec<String>,
    pub work: Option<String>,
    pub canon: Option<String>,
    pub period: Option<String>,
}

fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch.is_alphanumeric() {
            // keep CJK and other non-ASCII alnum as-is
            out.push(ch);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() { "seed".to_string() } else { trimmed }
}
