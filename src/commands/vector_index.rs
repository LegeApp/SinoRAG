use crate::vector_index::{self, HnswParams, VectorBuildMetadata};
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
    source_fingerprint: Option<String>,
    embedding_text_template: String,
    input_text_field_policy: String,
    truncation_policy: String,
    max_input_chars: Option<u32>,
    pooling: Option<String>,
    instruction: Option<String>,
    max_nb_connection: usize,
    ef_construction: usize,
    nb_layer: usize,
    resume: bool,
    hnsw_threads: Option<usize>,
) -> Result<()> {
    let hnsw = HnswParams {
        max_nb_connection,
        ef_construction,
        nb_layer,
    };

    // Resume: check what's already present.
    if resume && out.is_file() {
        let (dump_dir, dump_basename) = vector_index::hnsw_dump_location(&out)?;
        let graph_exists = dump_dir
            .join(format!(
                "{dump_basename}{}",
                vector_index::HNSW_GRAPH_SUFFIX
            ))
            .is_file();
        let data_exists = dump_dir
            .join(format!("{dump_basename}{}", vector_index::HNSW_DATA_SUFFIX))
            .is_file();

        if graph_exists && data_exists {
            eprintln!("resume: index and HNSW dump already complete — nothing to do");
            let header = vector_index::VectorIndex::header_info(&out)?;
            let payload = serde_json::json!({
                "schema": "sinorag-vector-build-v1",
                "out": out,
                "header": header,
                "resumed": true,
                "phase": "complete",
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
            return Ok(());
        }

        // .index exists but HNSW dump is missing — rebuild HNSW only from the index file.
        eprintln!("resume: index file found but HNSW dump missing — rebuilding HNSW from index");
        let header = vector_index::rebuild_hnsw_from_index(&out, hnsw_threads)?;
        let payload = serde_json::json!({
            "schema": "sinorag-vector-build-v1",
            "out": out,
            "header": header,
            "resumed": true,
            "phase": "hnsw_only",
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let header = vector_index::build_from_embeddings(
        &doc_table,
        &embeddings,
        &out,
        model_id,
        model_revision,
        VectorBuildMetadata {
            source_fingerprint,
            embedding_text_template,
            input_text_field_policy,
            truncation_policy,
            max_input_chars,
            pooling,
            instruction,
        },
        hnsw,
        hnsw_threads,
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
