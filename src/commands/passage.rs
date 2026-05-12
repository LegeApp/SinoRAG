use crate::datafusion_store::DataFusionStore;
use crate::jsonout::write_or_print;
use anyhow::Result;
use std::path::PathBuf;

pub async fn run(parquet_path: PathBuf, passage_id: String, out: Option<PathBuf>) -> Result<()> {
    let store = DataFusionStore::open(&parquet_path).await?;
    let passage = store.get_passage(&passage_id).await?;
    write_or_print(&passage, out)
}
