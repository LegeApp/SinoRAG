use anyhow::Result;
use serde::Deserialize;
use std::path::PathBuf;

use crate::tools;
use crate::tools::engine::ToolEngine;
use crate::tools::errors::ToolErrorBody;
use crate::tools::registry::call_tool_enveloped;

/// A batch job definition
#[derive(Debug, Clone, Deserialize)]
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

    #[arg(long)]
    pub passages_parquet: Option<PathBuf>,

    #[arg(long)]
    pub phrase_index: Option<PathBuf>,

    #[arg(long)]
    pub tfidf_index: Option<PathBuf>,

    #[arg(long)]
    pub vector_index: Option<PathBuf>,

    #[arg(long)]
    pub catalog_index: Option<PathBuf>,

    #[arg(long)]
    pub doc_table: Option<PathBuf>,

    #[arg(long)]
    pub registry: Option<PathBuf>,
}

/// Run a batch of tools from a JSONL file
pub async fn run(args: RunToolsArgs) -> Result<()> {
    use futures::{stream, StreamExt};
    use std::io::{BufRead, Write};
    use std::sync::Arc;

    let config = crate::tools::EngineConfig {
        pack: args.pack,
        readonly: args.readonly,
        allow_admin_tools: args.allow_admin_tools,
        max_heavy_concurrency: args.jobs.max(1),
        passages_parquet: args.passages_parquet,
        phrase_index: args.phrase_index,
        tfidf_index: args.tfidf_index,
        vector_index: args.vector_index,
        catalog_index: args.catalog_index,
        doc_table: args.doc_table,
        registry: args.registry,
        output_root: args.output_root,
    };

    let engine = Arc::new(ToolEngine::open(config).await?);

    let input = std::fs::File::open(&args.input)?;
    let reader = std::io::BufReader::new(input);

    if let Some(parent) = args.output.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let output = std::fs::File::create(&args.output)?;
    let mut writer = std::io::BufWriter::new(output);

    let mut jobs = Vec::new();
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
                    writer.flush()?;
                    return Ok(());
                }

                continue;
            }
        };

        jobs.push(job);
    }

    if jobs.iter().any(|job| !job.depends_on.is_empty()) {
        return run_jobs_with_dependencies(engine, jobs, &mut writer, args.continue_on_error).await;
    }

    if args.jobs.max(1) == 1 {
        for job in jobs {
            let env = run_one_job(engine.clone(), job.clone()).await;
            let ok = env.ok;
            writeln!(writer, "{}", serde_json::to_string(&env)?)?;
            writer.flush()?;

            let continue_job = job.continue_on_error.unwrap_or(args.continue_on_error);
            if !ok && !continue_job {
                break;
            }
        }
        return Ok(());
    }

    let continue_on_error = args.continue_on_error;
    let concurrency = args.jobs.max(1);
    let mut stream = stream::iter(jobs.into_iter().map(|job| {
        let engine = engine.clone();
        async move {
            let env = run_one_job(engine, job.clone()).await;
            (job, env)
        }
    }))
    .buffer_unordered(concurrency);

    while let Some((job, env)) = stream.next().await {
        let ok = env.ok;
        writeln!(writer, "{}", serde_json::to_string(&env)?)?;
        writer.flush()?;

        let continue_job = job.continue_on_error.unwrap_or(continue_on_error);
        if !ok && !continue_job {
            break;
        }
    }

    Ok(())
}

async fn run_jobs_with_dependencies<W: std::io::Write>(
    engine: std::sync::Arc<ToolEngine>,
    mut pending: Vec<BatchJob>,
    writer: &mut W,
    continue_on_error: bool,
) -> Result<()> {
    use std::collections::HashMap;
    use std::io::Write;

    let mut completed: HashMap<String, bool> = HashMap::new();

    while !pending.is_empty() {
        let mut made_progress = false;
        let mut next_pending = Vec::new();

        for job in pending {
            let failed_dep = job
                .depends_on
                .iter()
                .find(|dep| completed.get(*dep) == Some(&false))
                .cloned();
            if let Some(dep) = failed_dep {
                let env = skipped_dependency_envelope(&job, &dep);
                record_completion(&mut completed, &job, false);
                writeln!(writer, "{}", serde_json::to_string(&env)?)?;
                writer.flush()?;
                made_progress = true;
                if !job.continue_on_error.unwrap_or(continue_on_error) {
                    return Ok(());
                }
                continue;
            }

            let ready = job.depends_on.iter().all(|dep| completed.contains_key(dep));
            if !ready {
                next_pending.push(job);
                continue;
            }

            let env = run_one_job(engine.clone(), job.clone()).await;
            let ok = env.ok;
            record_completion(&mut completed, &job, ok);
            writeln!(writer, "{}", serde_json::to_string(&env)?)?;
            writer.flush()?;
            made_progress = true;

            if !ok && !job.continue_on_error.unwrap_or(continue_on_error) {
                return Ok(());
            }
        }

        if !made_progress {
            for job in next_pending {
                let missing: Vec<String> = job
                    .depends_on
                    .iter()
                    .filter(|dep| !completed.contains_key(*dep))
                    .cloned()
                    .collect();
                let env = unresolved_dependency_envelope(&job, missing);
                record_completion(&mut completed, &job, false);
                writeln!(writer, "{}", serde_json::to_string(&env)?)?;
                writer.flush()?;
                if !job.continue_on_error.unwrap_or(continue_on_error) {
                    return Ok(());
                }
            }
            return Ok(());
        }

        pending = next_pending;
    }

    Ok(())
}

fn record_completion(done: &mut std::collections::HashMap<String, bool>, job: &BatchJob, ok: bool) {
    if let Some(id) = &job.id {
        done.insert(id.clone(), ok);
    }
}

fn skipped_dependency_envelope(job: &BatchJob, dep: &str) -> tools::registry::ToolCallEnvelope {
    tools::registry::ToolCallEnvelope {
        id: job.id.clone(),
        ok: false,
        tool: job.tool.clone(),
        result: None,
        error: Some(ToolErrorBody {
            code: "dependency_failed".to_string(),
            message: format!("dependency `{dep}` failed; skipping job"),
            suggested_command: None,
            details: Some(serde_json::json!({ "dependency": dep })),
        }),
        meta: tools::registry::ToolCallMeta {
            elapsed_ms: 0,
            started_utc: None,
            finished_utc: None,
        },
    }
}

fn unresolved_dependency_envelope(
    job: &BatchJob,
    missing: Vec<String>,
) -> tools::registry::ToolCallEnvelope {
    tools::registry::ToolCallEnvelope {
        id: job.id.clone(),
        ok: false,
        tool: job.tool.clone(),
        result: None,
        error: Some(ToolErrorBody {
            code: "dependency_unresolved".to_string(),
            message: "job has dependencies that were not completed".to_string(),
            suggested_command: None,
            details: Some(serde_json::json!({ "missing": missing })),
        }),
        meta: tools::registry::ToolCallMeta {
            elapsed_ms: 0,
            started_utc: None,
            finished_utc: None,
        },
    }
}

async fn run_one_job(
    engine: std::sync::Arc<ToolEngine>,
    job: BatchJob,
) -> tools::registry::ToolCallEnvelope {
    let fut = call_tool_enveloped(
        engine.as_ref(),
        job.id.clone(),
        job.tool.clone(),
        job.args.clone(),
    );

    if let Some(timeout_ms) = job.timeout_ms {
        match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), fut).await {
            Ok(env) => env,
            Err(_) => tools::registry::ToolCallEnvelope {
                id: job.id.clone(),
                ok: false,
                tool: job.tool.clone(),
                result: None,
                error: Some(ToolErrorBody {
                    code: "timeout".to_string(),
                    message: format!("tool timed out after {timeout_ms} ms"),
                    suggested_command: None,
                    details: None,
                }),
                meta: tools::registry::ToolCallMeta {
                    elapsed_ms: timeout_ms as u128,
                    started_utc: None,
                    finished_utc: None,
                },
            },
        }
    } else {
        fut.await
    }
}
