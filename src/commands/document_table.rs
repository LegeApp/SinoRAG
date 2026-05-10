use crate::document_table::DocumentTable;
use crate::jsonout;
use crate::memory::check_memory_available;
use anyhow::Result;
use std::path::PathBuf;

pub fn build(parquet: PathBuf, out: PathBuf) -> Result<()> {
    eprintln!("Building DocumentTable from {}", parquet.display());
    let doc_table = DocumentTable::from_parquet(&parquet)?;
    
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    doc_table.save_atomic(&out)?;
    eprintln!("DocumentTable saved to {}", out.display());
    
    Ok(())
}
