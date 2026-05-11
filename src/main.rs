mod cli;
mod commands;
mod context_expand;
mod cef;
mod datafusion_store;
mod document_table;
mod ingest;
mod jsonout;
mod mcp;
mod memory;
mod models;
mod normalize;
mod catalog_index;
mod pack;
mod parquet_metadata;
mod phrase_index;
mod registry;
mod research;
mod search_packet;
mod storage;
mod tei;
mod templates;
mod tfidf;

use anyhow::Result;
use clap::Parser;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();

    let cli = cli::Cli::parse();
    commands::run(cli).await
}
