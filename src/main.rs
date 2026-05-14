mod cef;
mod cli;
mod commands;
mod datafusion_store;
mod document_table;
mod embedding;
mod ingest;
mod jsonout;
// mod mcp;  // Commented out - requires rmcp dependency
mod catalog_index;
mod memory;
mod models;
mod normalize;
mod pack;
mod parquet_metadata;
mod phrase_index;
mod registry;
mod research;
mod research_tools;
mod search_packet;
mod storage;
mod taxonomy_legend;
mod tei;
mod templates;
mod text_analyzer;
mod tfidf;
mod tools;
mod vector_index;

use anyhow::Result;
use clap::Parser;
use tracing::Level;

// Note: the Windows linker stack reserve is bumped to 32 MiB via
// `.cargo/config.toml`, because debug builds generate very large stack
// frames (proc-macro-generated MCP tool futures, clap parser) that
// overflow the default 1 MiB Windows main-thread stack at startup.
#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

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
