use serde::Deserialize;
use std::path::PathBuf;
use anyhow::Result;

use crate::tools::engine::ToolEngine;
use crate::tools::registry::call_tool_enveloped;
use crate::tools::errors::ToolErrorBody;
use crate::tools;

/// A batch job definition
#[derive(Debug, Deserialize)]
pub struct BatchJob {
    pub id: Option<String>,
    pub tool: String,
    
    #[serde(default)]
    pub args: serde_json::Value,
    
    #[serde(default)]
    pub depends_on: Vec<String>,
    
    #[serde(default)]
    pub continue_on_error: Option<bool>,
    
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Arguments for the run-tools command
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

/// Run a batch of tools from a JSONL file
pub async fn run(args: RunToolsArgs) -> Result<()> {
    use std::io::{BufRead, Write};
    
    let config = crate::tools::EngineConfig {
        pack: args.pack,
        readonly: args.readonly,
        allow_admin_tools: args.allow_admin_tools,
        max_heavy_concurrency: args.jobs.max(1),
        passages_parquet: None,
        phrase_index: None,
        tfidf_index: None,
        catalog_index: None,
        doc_table: None,
        registry: None,
        output_root: args.output_root,
    };
    
    let engine = ToolEngine::open(config).await?;
    
    let input = std::fs::File::open(&args.input)?;
    let reader = std::io::BufReader::new(input);
    
    if let Some(parent) = args.output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    let output = std::fs::File::create(&args.output)?;
    let mut writer = std::io::BufWriter::new(output);
    
    for (line_no, line) in reader.lines().enumerate() {
        let line = line?;
        
        if line.trim().is_empty() {
            continue;
        }
        
        let job: BatchJob = match serde_json::from_str(&line) {
            Ok(j) => j,
            Err(e) => {
                let env = tools::registry::ToolCallEnvelope {
                    id: Some(format!("line_{}", line_no + 1)),
                    ok: false,
                    tool: "<parse>".to_string(),
                    result: None,
                    error: Some(ToolErrorBody {
                        code: "invalid_json".to_string(),
                        message: format!("line {}: {}", line_no + 1, e),
                        suggested_command: None,
                        details: Some(serde_json::json!({ "line": line })),
                    }),
                    meta: tools::registry::ToolCallMeta {
                        elapsed_ms: 0,
                        started_utc: None,
                        finished_utc: None,
                    },
                };
                
                writeln!(writer, "{}", serde_json::to_string(&env)?)?;
                
                if !args.continue_on_error {
                    break;
                }
                
                continue;
            }
        };
        
        // Check for dependencies (not implemented yet, reject if present)
        if !job.depends_on.is_empty() {
            let env = tools::registry::ToolCallEnvelope {
                id: job.id.clone(),
                ok: false,
                tool: job.tool.clone(),
                result: None,
                error: Some(ToolErrorBody {
                    code: "dependencies_not_supported".to_string(),
                    message: "depends_on field is not yet supported".to_string(),
                    suggested_command: None,
                    details: None,
                }),
                meta: tools::registry::ToolCallMeta {
                    elapsed_ms: 0,
                    started_utc: None,
                    finished_utc: None,
                },
            };
            
            writeln!(writer, "{}", serde_json::to_string(&env)?)?;
            
            if !args.continue_on_error {
                break;
            }
            
            continue;
        }
        
        let env = call_tool_enveloped(
            &engine,
            job.id.clone(),
            job.tool.clone(),
            job.args.clone(),
        )
        .await;
        
        let ok = env.ok;
        writeln!(writer, "{}", serde_json::to_string(&env)?)?;
        writer.flush()?;
        
        let continue_job = job.continue_on_error.unwrap_or(args.continue_on_error);
        
        if !ok && !continue_job {
            break;
        }
    }
    
    Ok(())
}
