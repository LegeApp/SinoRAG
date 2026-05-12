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
    };
    crate::tools::batch::run(batch_args).await
}
