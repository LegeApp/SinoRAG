use std::path::PathBuf;
use anyhow::Result;
use crate::pack;

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
            let requires = d.spec.requires.clone();
            let mut spec = serde_json::to_value(d.spec).unwrap();

            if !args.include_examples {
                if let Some(obj) = spec.as_object_mut() {
                    obj.remove("examples");
                }
            }

            if let Some(obj) = spec.as_object_mut() {
                let missing = missing_requirements(args.pack.as_deref(), &requires);
                obj.insert("available".to_string(), serde_json::json!(missing.is_empty()));
                obj.insert("missing".to_string(), serde_json::json!(missing));
            }

            spec
        })
        .collect();

    let manifest = serde_json::json!({
        "schema": "sinoragd-tools-manifest-v1",
        "generated_by": "sinoragd",
        "pack": args.pack.as_ref().map(|p| p.display().to_string()),
        "tools": tools
    });

    println!("{}", serde_json::to_string_pretty(&manifest)?);
    Ok(())
}

fn missing_requirements(pack_root: Option<&std::path::Path>, requires: &[&'static str]) -> Vec<String> {
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
        "phrase_v3.index" => pack::DEFAULT_PHRASE,
        "tfidf_v3.index" => pack::DEFAULT_TFIDF,
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
        "phrase_v3.index" => std::path::Path::new("data/derived/phrase_v3.index").exists(),
        "tfidf_v3.index" => std::path::Path::new("data/derived/tfidf_v3.index").exists(),
        "registry.sqlite" => std::path::Path::new("data/derived/registry.sqlite").exists(),
        _ => false,
    }
}
