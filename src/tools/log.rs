use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::tools::errors::ToolErrorBody;

const DEFAULT_LOG_PATH: &str = ".sinorag/tool_calls.jsonl";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallLogRecord {
    pub schema: String,
    pub timestamp_utc: String,
    pub tool: String,
    pub ok: bool,
    pub elapsed_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    pub args_summary: Value,
    pub result_summary: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolLogSummary {
    pub schema: &'static str,
    pub path: String,
    pub total_calls: usize,
    pub tools: Vec<ToolLogToolSummary>,
    pub recent: Vec<ToolCallLogRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolLogToolSummary {
    pub tool: String,
    pub calls: usize,
    pub successes: usize,
    pub failures: usize,
    pub avg_elapsed_ms: f64,
    pub p95_elapsed_ms: u128,
    pub last_error_code: Option<String>,
}

pub fn default_log_path() -> PathBuf {
    std::env::var_os("SINORAG_TOOL_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_LOG_PATH))
}

pub fn append_call(
    tool: &str,
    args: &Value,
    result: Option<&Value>,
    error: Option<&ToolErrorBody>,
    elapsed_ms: u128,
) {
    let path = default_log_path();
    if let Err(err) = append_call_at(&path, tool, args, result, error, elapsed_ms) {
        eprintln!(
            "[sinorag] WARNING: failed to append tool log {}: {err}",
            path.display()
        );
    }
}

pub fn append_call_at(
    path: &Path,
    tool: &str,
    args: &Value,
    result: Option<&Value>,
    error: Option<&ToolErrorBody>,
    elapsed_ms: u128,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let record = ToolCallLogRecord {
        schema: "sinorag-tool-call-log-v1".to_string(),
        timestamp_utc: Utc::now().to_rfc3339(),
        tool: tool.to_string(),
        ok: error.is_none(),
        elapsed_ms,
        error_code: error.map(|err| err.code.clone()),
        args_summary: summarize_value(args),
        result_summary: result
            .map(summarize_tool_result)
            .unwrap_or_else(|| serde_json::json!({})),
    };
    let mut line = serde_json::to_vec(&record)?;
    line.push(b'\n');
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(&line)?;
    Ok(())
}

pub fn summarize(path: &Path, limit_recent: usize) -> Result<ToolLogSummary> {
    let mut records = Vec::new();
    if path.exists() {
        let text = std::fs::read_to_string(path)?;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            if let Ok(record) = serde_json::from_str::<ToolCallLogRecord>(line) {
                records.push(record);
            }
        }
    }

    let mut by_tool: BTreeMap<String, Vec<&ToolCallLogRecord>> = BTreeMap::new();
    for record in &records {
        by_tool.entry(record.tool.clone()).or_default().push(record);
    }

    let mut tools = Vec::with_capacity(by_tool.len());
    for (tool, calls) in by_tool {
        let successes = calls.iter().filter(|record| record.ok).count();
        let failures = calls.len().saturating_sub(successes);
        let total_elapsed: u128 = calls.iter().map(|record| record.elapsed_ms).sum();
        let avg_elapsed_ms = if calls.is_empty() {
            0.0
        } else {
            total_elapsed as f64 / calls.len() as f64
        };
        let mut elapsed = calls
            .iter()
            .map(|record| record.elapsed_ms)
            .collect::<Vec<_>>();
        elapsed.sort_unstable();
        let p95_elapsed_ms = percentile(&elapsed, 0.95);
        let last_error_code = calls
            .iter()
            .rev()
            .find_map(|record| record.error_code.clone());
        tools.push(ToolLogToolSummary {
            tool,
            calls: calls.len(),
            successes,
            failures,
            avg_elapsed_ms,
            p95_elapsed_ms,
            last_error_code,
        });
    }
    tools.sort_by(|a, b| b.calls.cmp(&a.calls).then(a.tool.cmp(&b.tool)));

    let mut recent = records
        .iter()
        .rev()
        .take(limit_recent)
        .cloned()
        .collect::<Vec<_>>();
    recent.reverse();

    Ok(ToolLogSummary {
        schema: "sinorag-tool-log-summary-v1",
        path: path.display().to_string(),
        total_calls: records.len(),
        tools,
        recent,
    })
}

fn percentile(sorted: &[u128], pct: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() - 1) as f64 * pct).ceil() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn summarize_tool_result(result: &Value) -> Value {
    let mut summary = summarize_value(result);
    if let Some(obj) = result.as_object() {
        let mut counts = serde_json::Map::new();
        for key in [
            "hits",
            "merged_hits",
            "groups",
            "clusters",
            "sample_hits",
            "suggested_next_tools",
            "warnings",
            "stages",
            "components",
        ] {
            if let Some(array) = obj.get(key).and_then(|value| value.as_array()) {
                counts.insert(key.to_string(), serde_json::json!(array.len()));
            }
        }
        if !counts.is_empty() {
            summary["top_level_counts"] = Value::Object(counts);
        }
        for key in [
            "schema",
            "workflow",
            "mode",
            "mode_reason",
            "returned_count",
            "hit_count",
            "pair_hit_count",
            "total_pair_hits",
        ] {
            if let Some(value) = obj.get(key) {
                summary[key] = value.clone();
            }
        }
    }
    summary
}

fn summarize_value(value: &Value) -> Value {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
        Value::String(s) => {
            if s.chars().count() > 80 {
                let truncated = s.chars().take(80).collect::<String>();
                serde_json::json!({"type": "string", "chars": s.chars().count(), "prefix": truncated})
            } else {
                value.clone()
            }
        }
        Value::Array(items) => serde_json::json!({
            "type": "array",
            "len": items.len(),
        }),
        Value::Object(obj) => {
            let mut out = serde_json::Map::new();
            for (key, value) in obj {
                out.insert(key.clone(), summarize_value(value));
            }
            Value::Object(out)
        }
    }
}
