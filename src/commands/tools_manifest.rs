use crate::pack;
use anyhow::Result;
use std::path::PathBuf;

#[derive(clap::Args, Debug)]
pub struct ToolsManifestArgs {
    #[arg(long)]
    pub pack: Option<PathBuf>,

    #[arg(long, default_value = "json")]
    pub format: String,

    #[arg(long, default_value_t = false)]
    pub include_examples: bool,

    #[arg(long, default_value_t = false)]
    pub include_schemas: bool,
}

pub async fn run(args: ToolsManifestArgs) -> Result<()> {
    use crate::tools::tool_defs;
    let tools: Vec<_> = tool_defs()
        .into_iter()
        .map(|d| {
            let requires = d.spec.requires.clone();
            let mut spec = serde_json::to_value(d.spec).unwrap();

            if !args.include_examples {
                if let Some(obj) = spec.as_object_mut() {
                    obj.remove("examples");
                }
            }

            if !args.include_schemas {
                if let Some(obj) = spec.as_object_mut() {
                    obj.remove("input_schema");
                    obj.remove("output_schema");
                }
            }

            if let Some(obj) = spec.as_object_mut() {
                let missing = missing_requirements(args.pack.as_deref(), &requires);
                obj.insert(
                    "available".to_string(),
                    serde_json::json!(missing.is_empty()),
                );
                obj.insert("missing".to_string(), serde_json::json!(missing));
                if let Some(docs) = crate::tools::docs::doc_for_tool(
                    obj.get("name").and_then(|v| v.as_str()).unwrap_or_default(),
                ) {
                    obj.insert("docs".to_string(), docs);
                }
            }

            spec
        })
        .collect();

    let manifest = serde_json::json!({
        "schema": "sinorag-tools-manifest-v1",
        "generated_by": "sinorag",
        "pack": args.pack.as_ref().map(|p| p.display().to_string()),
        "workflows": workflow_profiles(args.pack.as_deref()),
        "tools": tools
    });

    println!("{}", serde_json::to_string_pretty(&manifest)?);
    Ok(())
}

fn workflow_profiles(pack_root: Option<&std::path::Path>) -> serde_json::Value {
    let profile =
        |name: &str, primary_tool: &str, required: &[&'static str], optional: &[&'static str]| {
            let missing_required = missing_requirements(pack_root, required);
            let missing_optional = missing_requirements(pack_root, optional);
            let available = missing_required.is_empty();
            let quality = if !available {
                "unavailable"
            } else if missing_optional.is_empty() {
                "full"
            } else if required.len() == 1 && required[0] == "passages.parquet" {
                "minimal"
            } else {
                "partial"
            };
            serde_json::json!({
                "name": name,
                "primary_tool": primary_tool,
                "available": available,
                "quality": quality,
                "missing_required": missing_required,
                "missing_optional": missing_optional,
            })
        };

    serde_json::json!([
        profile(
            "exact_evidence",
            "evidence-search",
            &["passages.parquet"],
            &["phrase.index", "catalog.index", "doc_table.bin"],
        ),
        profile(
            "semantic_discovery",
            "hybrid-discover",
            &["passages.parquet", "doc_table.bin"],
            &["vector.index", "tfidf.index", "catalog.index"],
        ),
        profile(
            "source_investigation",
            "source-investigate",
            &["passages.parquet", "doc_table.bin"],
            &["catalog.index", "tfidf.index", "vector.index"],
        ),
        profile(
            "source_reading",
            "source-read",
            &["passages.parquet"],
            &["catalog.index"],
        ),
        profile(
            "scope_comparison",
            "scope-profile",
            &["passages.parquet", "catalog.index", "doc_table.bin"],
            &[],
        ),
        profile("report_from_evidence", "report-from-evidence", &[], &[],)
    ])
}

fn missing_requirements(
    pack_root: Option<&std::path::Path>,
    requires: &[&'static str],
) -> Vec<String> {
    requires
        .iter()
        .filter(|name| !resource_exists(pack_root, name))
        .map(|name| (*name).to_string())
        .collect()
}

fn resource_exists(pack_root: Option<&std::path::Path>, name: &str) -> bool {
    let rel = match name {
        "passages.parquet" => pack::DEFAULT_PASSAGES,
        "doc_table.bin" => pack::DEFAULT_DOC_TABLE,
        "catalog.index" => pack::DEFAULT_CATALOG,
        "phrase.index" => pack::DEFAULT_PHRASE,
        "tfidf.index" => pack::DEFAULT_TFIDF,
        "vector.index" => pack::DEFAULT_VECTOR,
        "registry.sqlite" => pack::DEFAULT_REGISTRY,
        other => other,
    };

    if let Some(root) = pack_root {
        return root.join(rel).exists();
    }

    match name {
        "passages.parquet" => std::path::Path::new("data/passages.parquet").exists(),
        "doc_table.bin" => std::path::Path::new("data/derived/doc_table.bin").exists(),
        "catalog.index" => std::path::Path::new("data/derived/catalog.index").exists(),
        "phrase.index" => std::path::Path::new("data/derived/phrase.index").exists(),
        "tfidf.index" => std::path::Path::new("data/derived/tfidf.index").exists(),
        "vector.index" => std::path::Path::new("data/derived/vector.index").exists(),
        "registry.sqlite" => std::path::Path::new("data/derived/registry.sqlite").exists(),
        _ => false,
    }
}
