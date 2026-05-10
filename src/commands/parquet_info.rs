use crate::phrase_index::parquet_files;
use anyhow::Result;
use serde_json::json;
use std::collections::HashSet;
use std::path::PathBuf;

pub fn parquet_info(parquet_path: PathBuf) -> Result<()> {
    eprintln!("Analyzing parquet: {}", parquet_path.display());

    let files = parquet_files(&parquet_path)?;
    eprintln!("Found {} parquet files", files.len());

    let mut total_rows = 0u64;
    let mut partitions: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut unique_corpus: HashSet<String> = HashSet::new();

    for file_path in &files {
        let file = std::fs::File::open(file_path)?;
        let builder = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file)?;
        let reader = builder.build()?;

        for batch_result in reader {
            if let Ok(batch) = batch_result {
                total_rows += batch.num_rows() as u64;
            }
        }

        // Get partition info from path
        let path_str = file_path.to_string_lossy().to_string();
        for part in path_str.split('/') {
            if part.starts_with("source_corpus=") {
                let corpus = part.trim_start_matches("source_corpus=").to_string();
                unique_corpus.insert(corpus.clone());
                *partitions.entry(corpus).or_insert(0) += 1;
            }
        }
    }

    let payload = json!({
        "schema": "readzen-parquet-info",
        "path": parquet_path.display().to_string(),
        "total_files": files.len(),
        "total_rows": total_rows,
        "unique_corpus": unique_corpus.iter().collect::<Vec<_>>(),
        "partitions": partitions,
    });

    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}