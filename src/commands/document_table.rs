use crate::document_table::{DocTableLineage, DocumentTable};
use anyhow::{Context, Result};
use std::path::PathBuf;

pub fn build(parquet: PathBuf, out: PathBuf, append_to: Option<PathBuf>) -> Result<()> {
    if let Some(base_path) = append_to {
        eprintln!("Appending to existing DocumentTable at {}", base_path.display());
        let base = DocumentTable::load(&base_path)
            .with_context(|| format!("load base {}", base_path.display()))?;
        let base_fp = base.source_fingerprint.clone();
        let base_count = base.passage_ids.len() as u32;
        eprintln!("Base: {} passages, fingerprint {}...{}",
            base_count,
            &base_fp[..8.min(base_fp.len())],
            &base_fp[base_fp.len().saturating_sub(8)..]);

        let merged = DocumentTable::append_from_parquet(&base, &parquet)?;
        if let Some(parent) = out.parent() { std::fs::create_dir_all(parent)?; }
        merged.save_atomic(&out)?;
        DocTableLineage::new(base_fp, base_count).write(&out)?;
        eprintln!("DocumentTable saved to {} (+{} passages, total {})",
            out.display(),
            merged.passage_ids.len() - base_count as usize,
            merged.passage_ids.len());
    } else {
        eprintln!("Building DocumentTable from {}", parquet.display());
        let dt = DocumentTable::from_parquet(&parquet)?;
        if let Some(parent) = out.parent() { std::fs::create_dir_all(parent)?; }
        dt.save_atomic(&out)?;
        DocTableLineage::delete_if_present(&out)?;
        eprintln!("DocumentTable saved to {}", out.display());
    }
    Ok(())
}
