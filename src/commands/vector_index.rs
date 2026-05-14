use crate::vector_index::{self, HnswParams};
use anyhow::Result;
use std::path::PathBuf;

pub async fn export(
    parquet: PathBuf,
    doc_table: PathBuf,
    out: PathBuf,
    limit: Option<usize>,
) -> Result<()> {
    let count = vector_index::export_jsonl(parquet, doc_table, out.clone(), limit).await?;
    let payload = serde_json::json!({
        "schema": "sinorag-vector-export-v1",
        "out": out,
        "records": count,
    });
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn build(
    doc_table: PathBuf,
    embeddings: PathBuf,
    out: PathBuf,
    model_id: String,
    model_revision: String,
    max_nb_connection: usize,
    ef_construction: usize,
    nb_layer: usize,
) -> Result<()> {
    let header = vector_index::build_from_embeddings(
        &doc_table,
        &embeddings,
        &out,
        model_id,
        model_revision,
        HnswParams {
            max_nb_connection,
            ef_construction,
            nb_layer,
        },
    )?;
    let payload = serde_json::json!({
        "schema": "sinorag-vector-build-v1",
        "out": out,
        "header": header,
    });
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

pub fn info(index: PathBuf) -> Result<()> {
    let payload = vector_index::VectorIndex::header_info(&index)?;
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}
