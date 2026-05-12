use std::path::PathBuf;
use anyhow::Result;

#[derive(clap::Args, Debug)]
pub struct ToolsManifestArgs {
    #[arg(long)]
    pub pack: Option<PathBuf>,
    
    #[arg(long, default_value = "json")]
    pub format: String,
    
    #[arg(long, default_value_t = false)]
    pub include_examples: bool,
}

pub async fn run(args: ToolsManifestArgs) -> Result<()> {
    use crate::tools::tool_defs;
    
    let tools: Vec<_> = tool_defs()
        .into_iter()
        .map(|d| {
            let mut spec = serde_json::to_value(d.spec).unwrap();
            
            if !args.include_examples {
                if let Some(obj) = spec.as_object_mut() {
                    obj.remove("examples");
                }
            }
            
            spec
        })
        .collect();
    
    let manifest = serde_json::json!({
        "schema": "sinoragd-tools-manifest-v1",
        "generated_by": "sinoragd",
        "tools": tools
    });
    
    println!("{}", serde_json::to_string_pretty(&manifest)?);
    Ok(())
}
