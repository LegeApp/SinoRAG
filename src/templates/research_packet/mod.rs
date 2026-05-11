//! Research packet: SinoRAG curates source material — it does not write the
//! report. A packet is a zipped directory of tool outputs, primary-source
//! passages, contexts, full work texts, and graph drafts. A downstream
//! agent or human researcher composes the actual report from this packet.
//!
//! Layout (relative to the unzipped packet root):
//! ```text
//!   manifest.json       provenance + every tool invocation
//!   README.md           orientation for the receiving agent
//!   brief/
//!     brief.json
//!     brief.md
//!   tools/              raw JSON outputs, one per (step, seed)
//!   passages/           one .md per cited passage
//!   contexts/           ±N-passage windows around each cited passage
//!   documents/          full text of works contributing >= threshold cited passages
//!   pre_diagrams/       evidence / timeline / lineage graph drafts (JSON)
//!   index.jsonl         path + bytes of every artifact
//! ```

pub mod assemble;
pub mod brief;
pub mod gather;
pub mod recipe;

use crate::pack::Pack;
use anyhow::Result;
use std::path::PathBuf;

pub use brief::{Brief, BriefArgs, Seed};
pub use recipe::Recipe;

pub struct BuildOptions {
    pub pack_root: PathBuf,
    pub out_zip: PathBuf,
    pub recipe_name_or_path: String,
    pub keep_temp: bool,
}

pub async fn build(brief: Brief, options: BuildOptions) -> Result<PathBuf> {
    let pack = Pack::open(&options.pack_root)?;
    let recipe = Recipe::load(&options.recipe_name_or_path)?;

    // Stage in <out_zip>.work/ so the zip itself is atomic at the end.
    let work_dir = options.out_zip.with_extension("work");
    if work_dir.exists() {
        std::fs::remove_dir_all(&work_dir)?;
    }
    std::fs::create_dir_all(&work_dir)?;
    let tools_dir     = work_dir.join("tools");
    let passages_dir  = work_dir.join("passages");
    let contexts_dir  = work_dir.join("contexts");
    let documents_dir = work_dir.join("documents");
    let diagrams_dir  = work_dir.join("pre_diagrams");

    eprintln!("=== research-packet build ===");
    eprintln!("Brief topic : {}", brief.topic);
    eprintln!("Recipe      : {} ({} steps, full_work_threshold={})",
        recipe.name, recipe.steps.len(), recipe.full_work_threshold);
    eprintln!("Pack        : {}", options.pack_root.display());
    eprintln!("Staging dir : {}", work_dir.display());
    eprintln!("Output zip  : {}", options.out_zip.display());
    eprintln!();

    eprintln!("[1/6 gather] running tool steps...");
    let invocations = gather::run(&brief, &recipe, &pack, &tools_dir, &work_dir).await?;
    let errors = invocations.iter().filter(|i| i.error.is_some()).count();
    eprintln!("       {} invocations, {} errors", invocations.len(), errors);

    eprintln!("[2/6 passages] extracting cited passages...");
    let stats = assemble::collect_and_write_passages(&work_dir, &passages_dir, &invocations)?;
    eprintln!("       {} passages written ({} distinct works)",
        stats.passages_written, stats.works_seen.len());

    eprintln!("[3/6 contexts] expanding contexts (±{}/±{})...",
        recipe.context_before, recipe.context_after);
    let contexts_written = assemble::write_contexts(
        &contexts_dir, &pack, &stats.cited,
        recipe.context_before, recipe.context_after,
    ).await?;
    eprintln!("       {} contexts written", contexts_written);

    eprintln!("[4/6 documents] full-text for works with >= {} cited passages...",
        recipe.full_work_threshold);
    let documents_written = assemble::write_documents(
        &documents_dir, &pack, &stats.works_seen, recipe.full_work_threshold,
    ).await?;
    eprintln!("       {} works exported in full", documents_written.len());

    eprintln!("[5/6 pre_diagrams] rendering graph drafts...");
    assemble::write_pre_diagrams(&diagrams_dir, &brief, &stats.cited)?;
    eprintln!("       evidence, timeline, lineage written");

    eprintln!("[6/6 assemble] manifest + README + index + zip...");
    assemble::write_brief_files(&work_dir, &brief)?;
    assemble::write_readme(&work_dir, &brief, &recipe)?;
    assemble::write_manifest(&work_dir, &brief, &recipe, &pack,
        &invocations, &stats, contexts_written, &documents_written)?;
    assemble::write_index_jsonl(&work_dir)?;
    assemble::seal_zip(&work_dir, &options.out_zip)?;
    eprintln!("\nwrote {}", options.out_zip.display());

    if !options.keep_temp {
        let _ = std::fs::remove_dir_all(&work_dir);
    } else {
        eprintln!("(kept staging dir: {})", work_dir.display());
    }
    Ok(options.out_zip)
}
