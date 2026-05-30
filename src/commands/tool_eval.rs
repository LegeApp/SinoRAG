use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(clap::Args, Debug)]
pub struct ToolEvalArgs {
    /// JSON fixture file containing eval cases.
    #[arg(long)]
    pub cases: PathBuf,

    /// Optional path for the JSON eval report.
    #[arg(long)]
    pub out: Option<PathBuf>,

    #[arg(long)]
    pub pack: Option<PathBuf>,

    #[arg(long, default_value_t = false)]
    pub readonly: bool,

    #[arg(long, default_value_t = false)]
    pub allow_admin_tools: bool,

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

#[derive(Debug, Deserialize)]
struct EvalFile {
    #[serde(default = "default_eval_schema")]
    schema: String,
    cases: Vec<EvalCase>,
}

fn default_eval_schema() -> String {
    "sinorag-tool-eval-v1".to_string()
}

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    tool: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    expect: EvalExpect,
}

#[derive(Debug, Default, Deserialize)]
struct EvalExpect {
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    json_pointer_exists: Vec<String>,
    #[serde(default)]
    json_pointer_absent: Vec<String>,
    #[serde(default)]
    min_counts: Vec<MinCountExpectation>,
    #[serde(default)]
    equals: Vec<EqualsExpectation>,
}

#[derive(Debug, Deserialize)]
struct MinCountExpectation {
    pointer: String,
    min: usize,
}

#[derive(Debug, Deserialize)]
struct EqualsExpectation {
    pointer: String,
    value: Value,
}

#[derive(Debug, Serialize)]
struct EvalReport {
    schema: &'static str,
    fixture_schema: String,
    cases: Vec<EvalCaseReport>,
    passed: usize,
    failed: usize,
}

#[derive(Debug, Serialize)]
struct EvalCaseReport {
    id: String,
    tool: String,
    ok: bool,
    elapsed_ms: u128,
    passed: bool,
    failures: Vec<String>,
}

pub async fn run(args: ToolEvalArgs) -> Result<()> {
    use crate::tools::{call_tool_enveloped, EngineConfig, ToolEngine};

    let text = std::fs::read_to_string(&args.cases)
        .with_context(|| format!("reading eval fixture {}", args.cases.display()))?;
    let eval: EvalFile = serde_json::from_str(&text)
        .with_context(|| format!("parsing eval fixture {}", args.cases.display()))?;

    let config = EngineConfig {
        pack: args.pack,
        readonly: args.readonly,
        allow_admin_tools: args.allow_admin_tools,
        max_heavy_concurrency: 1,
        passages_parquet: args.passages_parquet,
        phrase_index: args.phrase_index,
        tfidf_index: args.tfidf_index,
        vector_index: args.vector_index,
        catalog_index: args.catalog_index,
        doc_table: args.doc_table,
        registry: args.registry,
        output_root: args.output_root,
    };
    let engine = ToolEngine::open(config).await?;

    let mut reports = Vec::with_capacity(eval.cases.len());
    for case in eval.cases {
        let envelope =
            call_tool_enveloped(&engine, Some(case.id.clone()), case.tool.clone(), case.args).await;
        let failures = evaluate_expectations(&envelope, &case.expect);
        reports.push(EvalCaseReport {
            id: case.id,
            tool: case.tool,
            ok: envelope.ok,
            elapsed_ms: envelope.meta.elapsed_ms,
            passed: failures.is_empty(),
            failures,
        });
    }

    let passed = reports.iter().filter(|case| case.passed).count();
    let failed = reports.len().saturating_sub(passed);
    let report = EvalReport {
        schema: "sinorag-tool-eval-report-v1",
        fixture_schema: eval.schema,
        cases: reports,
        passed,
        failed,
    };
    let json = serde_json::to_string_pretty(&report)?;
    if let Some(path) = args.out {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, json)?;
    } else {
        println!("{json}");
    }
    Ok(())
}

fn evaluate_expectations(
    envelope: &crate::tools::registry::ToolCallEnvelope,
    expect: &EvalExpect,
) -> Vec<String> {
    let mut failures = Vec::new();
    let envelope_value = serde_json::to_value(envelope).unwrap_or_else(|_| serde_json::json!({}));

    if let Some(expected_ok) = expect.ok {
        if envelope.ok != expected_ok {
            failures.push(format!("expected ok={expected_ok}, got {}", envelope.ok));
        }
    }
    for pointer in &expect.json_pointer_exists {
        if envelope_value.pointer(pointer).is_none() {
            failures.push(format!("missing pointer {pointer}"));
        }
    }
    for pointer in &expect.json_pointer_absent {
        if envelope_value.pointer(pointer).is_some() {
            failures.push(format!("expected pointer absent {pointer}"));
        }
    }
    for expectation in &expect.min_counts {
        match envelope_value.pointer(&expectation.pointer) {
            Some(Value::Array(items)) if items.len() >= expectation.min => {}
            Some(Value::Array(items)) => failures.push(format!(
                "pointer {} has {} items, expected at least {}",
                expectation.pointer,
                items.len(),
                expectation.min
            )),
            Some(Value::Number(number)) => {
                let value = number.as_u64().unwrap_or(0) as usize;
                if value < expectation.min {
                    failures.push(format!(
                        "pointer {} is {}, expected at least {}",
                        expectation.pointer, value, expectation.min
                    ));
                }
            }
            Some(_) => failures.push(format!(
                "pointer {} is not an array or number",
                expectation.pointer
            )),
            None => failures.push(format!("missing pointer {}", expectation.pointer)),
        }
    }
    for expectation in &expect.equals {
        match envelope_value.pointer(&expectation.pointer) {
            Some(value) if value == &expectation.value => {}
            Some(value) => failures.push(format!(
                "pointer {} expected {}, got {}",
                expectation.pointer, expectation.value, value
            )),
            None => failures.push(format!("missing pointer {}", expectation.pointer)),
        }
    }

    failures
}
