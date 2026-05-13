//! Gather phase: for each (seed, applicable step) pair, invoke the
//! matching SinoRAG command in-process with its output redirected into
//! the packet's `tools/` directory. Returns a manifest of what ran.

use super::brief::{Brief, Seed};
use super::recipe::{Recipe, WhenFilter};
use crate::pack::Pack;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub step_index: usize,
    pub tool: String,
    pub seed_kind: String,
    pub seed_slug: String,
    pub seed_value: String,
    pub output_relpath: PathBuf,
    pub args: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn run(
    brief: &Brief,
    recipe: &Recipe,
    pack: &Pack,
    tools_dir: &Path,
    packet_root: &Path,
) -> Result<Vec<ToolInvocation>> {
    std::fs::create_dir_all(tools_dir)?;

    let parquet = pack.passages_path();
    let phrase_index = pack.phrase_path();
    let tfidf_index = pack.tfidf_path();
    let catalog = pack.catalog_path();

    let mut invocations = Vec::new();

    // Pass 1 — seed-driven steps.
    for (step_idx, step) in recipe.steps.iter().enumerate() {
        if matches!(step.when, WhenFilter::AnyHit | WhenFilter::AnyWork) {
            continue; // fan-out passes handled separately if/when implemented
        }
        for seed in &brief.seeds {
            if !step.when.matches_seed_kind(seed.kind()) {
                continue;
            }
            let out_name = format!("{:02}_{}--{}.json", step_idx + 1, step.tool, seed.slug(),);
            let out_path = tools_dir.join(&out_name);
            let rel_out = out_path
                .strip_prefix(packet_root)
                .unwrap_or(&out_path)
                .to_path_buf();

            let mut inv = ToolInvocation {
                step_index: step_idx + 1,
                tool: step.tool.clone(),
                seed_kind: seed.kind().to_string(),
                seed_slug: seed.slug(),
                seed_value: seed_value_for_log(seed),
                output_relpath: rel_out,
                args: step.args.clone(),
                error: None,
            };

            let result = dispatch(
                &step.tool,
                seed,
                &step.args,
                &parquet,
                phrase_index.as_deref(),
                tfidf_index.as_deref(),
                &catalog,
                &out_path,
            )
            .await;

            if let Err(e) = result {
                inv.error = Some(format!("{e:#}"));
                eprintln!(
                    "  ! step {} `{}` on seed `{}`: {}",
                    inv.step_index, inv.tool, inv.seed_slug, e
                );
            } else {
                eprintln!(
                    "  ok step {} `{}` on seed `{}` -> {}",
                    inv.step_index,
                    inv.tool,
                    inv.seed_slug,
                    inv.output_relpath.display()
                );
            }
            invocations.push(inv);
        }
    }

    Ok(invocations)
}

fn seed_value_for_log(seed: &Seed) -> String {
    match seed {
        Seed::Phrase { value }
        | Seed::Passage { value }
        | Seed::Work { value }
        | Seed::Canon { value }
        | Seed::Period { value } => value.clone(),
        Seed::Person { name, .. } => name.clone(),
    }
}

async fn dispatch(
    tool: &str,
    seed: &Seed,
    args: &Value,
    parquet: &Path,
    phrase_index: Option<&Path>,
    tfidf_index: Option<&Path>,
    _catalog: &Path,
    out: &Path,
) -> Result<()> {
    let pq = parquet.to_path_buf();
    let pi = phrase_index.map(|p| p.to_path_buf());
    let out_opt = Some(out.to_path_buf());

    match (tool, seed) {
        ("phrase-index-search", Seed::Phrase { value }) => {
            let idx = pi.clone().context("phrase index missing in pack")?;
            let limit = uarg(args, "limit", 200);
            crate::commands::phrase_index::search(pq, idx, value.clone(), limit, out_opt).await
        }
        ("phrase-history", Seed::Phrase { value }) => {
            let limit = uarg(args, "limit", 200);
            let include_variants = barg(args, "include_variants", true);
            // phrase-history pulls timeline buckets internally when timeline=true
            let timeline = barg(args, "timeline", false);
            crate::commands::phrase_history::run(
                value.clone(),
                pq,
                include_variants,
                timeline,
                pi,
                out_opt,
            )
            .await?;
            // phrase-history caps at the internal default; the explicit limit is informational.
            let _ = limit;
            Ok(())
        }
        ("first-attestation", Seed::Phrase { value }) => {
            let limit = uarg(args, "limit", 200);
            crate::commands::first_attestation::run(value.clone(), pq, limit, pi, out_opt).await
        }
        ("timeline", Seed::Phrase { value }) => {
            let limit = uarg(args, "limit", 200);
            let include_variants = barg(args, "include_variants", true);
            crate::commands::timeline::run(value.clone(), pq, include_variants, limit, pi, out_opt)
                .await
        }
        ("canonical-source", Seed::Phrase { value }) => {
            let limit = uarg(args, "limit", 100);
            let canon: Vec<String> = args
                .get("canon")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_else(|| vec!["T".to_string()]);
            crate::commands::canonical_source::run(value.clone(), pq, canon, limit, pi, out_opt)
                .await
        }
        ("person-history", Seed::Person { name, aliases }) => {
            let limit = uarg(args, "limit", 200);
            crate::commands::person_history::run(name.clone(), aliases.clone(), pq, limit, out_opt)
                .await
        }
        ("person-resolve", Seed::Person { name, aliases }) => {
            crate::commands::person_resolve::run(name.clone(), aliases.clone(), pq, out_opt).await
        }
        ("similar", Seed::Passage { value }) => {
            let idx = tfidf_index
                .map(|p| p.to_path_buf())
                .context("tfidf index missing in pack")?;
            let limit = uarg(args, "limit", 25);
            let shared_ngram_limit = uarg(args, "shared_ngram_limit", 12);
            let shared_phrase_limit = uarg(args, "shared_phrase_limit", 8);
            let min_shared_phrase_len = uarg(args, "min_shared_phrase_len", 4);
            crate::commands::tfidf::similar(
                pq,
                idx,
                value.clone(),
                limit,
                shared_ngram_limit,
                shared_phrase_limit,
                min_shared_phrase_len,
                out_opt,
            )
            .await
        }
        _ => {
            anyhow::bail!(
                "no dispatch for tool=`{}` seed-kind=`{}`",
                tool,
                seed.kind()
            )
        }
    }
}

fn uarg(args: &Value, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(default)
}
fn barg(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}
