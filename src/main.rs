mod cli;
mod commands;
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
mod research_tools;
mod search_packet;
mod storage;
mod tei;
mod templates;
mod text_analyzer;
mod tfidf;

use anyhow::Result;
use clap::Parser;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();

    // Resolve all relative paths (e.g. "data/...") against the exe's directory
    // so the binary can be invoked from any working directory.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let _ = std::env::set_current_dir(exe_dir);
        }
    }

    let cli = cli::Cli::parse();
    commands::run(cli).await
}
