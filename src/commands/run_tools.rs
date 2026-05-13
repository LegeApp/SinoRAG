use std::path::PathBuf;
use anyhow::Result;

#[derive(clap::Args, Debug)]
pub struct RunToolsArgs {
    #[arg(long)]
    pub input: PathBuf,

    #[arg(long)]
    pub output: PathBuf,

    #[arg(long)]
    pub pack: Option<PathBuf>,

    #[arg(long, default_value_t = false)]
    pub readonly: bool,

    #[arg(long, default_value_t = false)]
    pub allow_admin_tools: bool,

    #[arg(long, default_value_t = true)]
    pub continue_on_error: bool,

    #[arg(long, default_value_t = 1)]
    pub jobs: usize,

    #[arg(long)]
    pub output_root: Option<PathBuf>,

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
}

pub async fn run(args: RunToolsArgs) -> Result<()> {
    let batch_args = crate::tools::batch::RunToolsArgs {
        input: args.input,
        output: args.output,
        pack: args.pack,
        readonly: args.readonly,
        allow_admin_tools: args.allow_admin_tools,
        continue_on_error: args.continue_on_error,
        jobs: args.jobs,
        output_root: args.output_root,
        passages_parquet: args.passages_parquet,
        phrase_index: args.phrase_index,
        tfidf_index: args.tfidf_index,
        catalog_index: args.catalog_index,
        doc_table: args.doc_table,
        registry: args.registry,
    };
    crate::tools::batch::run(batch_args).await
}
