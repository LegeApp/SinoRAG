//! CLI dispatcher for `research-packet build`. Thin: parses the brief
//! (from flags or `--brief file.json`), then hands off to the template.

use crate::templates::research_packet::{self, Brief, BriefArgs, BuildOptions};
use anyhow::Result;
use chrono::Utc;
use std::path::PathBuf;

#[allow(clippy::too_many_arguments)]
pub async fn build(
    pack: PathBuf,
    out: Option<PathBuf>,
    recipe: String,
    brief_file: Option<PathBuf>,
    keep_temp: bool,
    // brief-from-flags
    topic: Option<String>,
    notes: Option<String>,
    phrase: Option<String>,
    seed_passage: Option<String>,
    person: Option<String>,
    person_alias: Vec<String>,
    work: Option<String>,
    canon: Option<String>,
    period: Option<String>,
) -> Result<()> {
    let brief = if let Some(path) = brief_file {
        Brief::from_file(&path)?
    } else {
        Brief::from_flags(BriefArgs {
            topic, notes, phrase, seed_passage, person, person_alias,
            work, canon, period,
        })?
    };

    let out_zip = out.unwrap_or_else(|| default_out(&brief.topic));

    research_packet::build(brief, BuildOptions {
        pack_root: pack,
        out_zip,
        recipe_name_or_path: recipe,
        keep_temp,
    }).await?;
    Ok(())
}

fn default_out(topic: &str) -> PathBuf {
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let safe: String = topic
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let safe = safe.trim_matches('-').to_string();
    PathBuf::from(format!("data/research_packets/{safe}-{stamp}.researchpacket.zip"))
}
