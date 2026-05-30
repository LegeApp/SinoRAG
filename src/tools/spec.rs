use serde::Serialize;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Safety level for a tool, used to enforce permissions
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSafety {
    /// Read-only operations, safe in readonly mode
    ReadOnly,
    /// Writes output files but doesn't mutate core data
    WritesOutput,
    /// Mutates the registry or metadata
    MutatesRegistry,
    /// Admin operations that can modify corpus data
    Admin,
}

/// Intended audience for a tool in agent-facing manifests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAudience {
    /// Preferred task-level tools for ordinary agent workflows.
    DefaultAgent,
    /// Public lower-level tools for focused or specialist calls.
    Specialist,
    /// Debug or forced-path tools hidden from normal manifests.
    InternalDebug,
}

/// Example usage of a tool for documentation
#[derive(Debug, Clone, Serialize)]
pub struct ToolExample {
    pub title: &'static str,
    pub args: Value,
}

/// Specification for a tool
#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: &'static str,
    pub audience: ToolAudience,
    pub description: &'static str,
    pub input_schema: Value,
    pub output_schema: Value,
    pub requires: Vec<&'static str>,
    pub safety: ToolSafety,
    pub examples: Vec<ToolExample>,
}

/// Type alias for tool handler future
pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, anyhow::Error>> + Send + 'a>>;

/// Definition of a tool including its spec and handler
pub struct ToolDef {
    pub spec: ToolSpec,
    pub call: for<'a> fn(&'a ToolEngine, Value) -> ToolFuture<'a>,
}

// Forward declaration for ToolEngine
use crate::tools::engine::ToolEngine;

/// Helper to generate JSON schema from a type using schemars
pub fn schema_for<T: schemars::JsonSchema>() -> Value {
    let schema = schemars::schema_for!(T);
    serde_json::to_value(schema).unwrap_or_else(|_| serde_json::json!({}))
}
