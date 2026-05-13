use std::path::PathBuf;
use anyhow::Result;

#[derive(clap::Args, Debug)]
pub struct ToolCallArgs {
    pub tool: String,

    #[arg(long)]
    pub json: Option<String>,

    #[arg(long)]
    pub json_file: Option<PathBuf>,

    #[arg(long)]
    pub pack: Option<PathBuf>,

    #[arg(long, default_value_t = false)]
    pub readonly: bool,

    #[arg(long, default_value_t = false)]
    pub allow_admin_tools: bool,

    #[arg(long)]
    pub passages_parquet: Option<PathBuf>,

    #[arg(long)]
    pub phrase_index: Option<PathBuf>,

    #[arg(long)]
    pub tfidf_index: Option<PathBuf>,

    #[arg(long)]
    pub catalog_index: Option<PathBuf>,

    #[arg(long)]
    pub doc_table: Option<PathBuf>,

    #[arg(long)]
    pub registry: Option<PathBuf>,

    #[arg(long)]
    pub output_root: Option<PathBuf>,
}

pub async fn run(args: ToolCallArgs) -> Result<()> {
    use crate::tools::{ToolEngine, EngineConfig, call_tool_enveloped};

    let json_text = match (&args.json, &args.json_file) {
        (Some(s), None) => s.clone(),
        (None, Some(path)) => std::fs::read_to_string(path)?,
        (Some(_), Some(_)) => anyhow::bail!("use either --json or --json-file, not both"),
        (None, None) => anyhow::bail!("missing --json or --json-file"),
    };

    let value: serde_json::Value = serde_json::from_str(&json_text)
        .map_err(|e| anyhow::anyhow!("invalid JSON args: {}", e))?;

    let config = EngineConfig {
        pack: args.pack,
        readonly: args.readonly,
        allow_admin_tools: args.allow_admin_tools,
        max_heavy_concurrency: 1,
        passages_parquet: args.passages_parquet,
        phrase_index: args.phrase_index,
        tfidf_index: args.tfidf_index,
        catalog_index: args.catalog_index,
        doc_table: args.doc_table,
        registry: args.registry,
        output_root: args.output_root,
    };

    let engine = ToolEngine::open(config).await?;

    let envelope = call_tool_enveloped(&engine, None, args.tool, value).await;

    println!("{}", serde_json::to_string_pretty(&envelope)?);

    Ok(())
}
