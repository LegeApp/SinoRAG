use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultPacket {
    pub schema: String,
    pub result_set_id: String,
    pub source_fingerprint: Option<String>,
    pub query: Value,
    pub results: Vec<SearchHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub hit_id: String,
    pub rank: usize,
    pub passage_id: String,

    #[serde(default)]
    pub score: Option<f32>,

    #[serde(default)]
    pub source_rel_path: String,

    #[serde(default)]
    pub source_work_id: String,

    #[serde(default)]
    pub xml_id: String,

    #[serde(default)]
    pub heading_path: String,

    #[serde(default)]
    pub citation: String,

    #[serde(default)]
    pub snippet: String,

    #[serde(default)]
    pub raw: Value,
}

impl SearchResultPacket {
    pub fn new(
        result_set_id: String,
        source_fingerprint: Option<String>,
        query: Value,
        results: Vec<SearchHit>,
    ) -> Self {
        Self {
            schema: "readzen-search-results-v1".to_string(),
            result_set_id,
            source_fingerprint,
            query,
            results,
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("read search result packet {}", path.display()))?;
        let packet: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse search result packet {}", path.display()))?;

        if packet.schema != "readzen-search-results-v1" {
            anyhow::bail!(
                "unsupported search packet schema `{}` in {}",
                packet.schema,
                path.display()
            );
        }

        Ok(packet)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create parent directory {}", path.display()))?;
        }

        std::fs::write(path, serde_json::to_string_pretty(self)? + "\n")
            .with_context(|| format!("write search result packet {}", path.display()))?;

        Ok(())
    }

    pub fn find_hit(&self, hit_id: &str) -> Result<&SearchHit> {
        self.results
            .iter()
            .find(|hit| hit.hit_id == hit_id)
            .ok_or_else(|| anyhow!("hit_id not found in result packet: {hit_id}"))
    }
}

pub fn make_result_set_id(prefix: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let mut hasher = Sha256::new();
    hasher.update(prefix.as_bytes());
    hasher.update(now.to_le_bytes());
    let digest = hex::encode(hasher.finalize());

    format!("{prefix}_{now}_{}", &digest[..8])
}

pub fn make_hit_id(rank: usize) -> String {
    format!("hit_{rank:06}")
}

pub fn row_to_search_hit(rank: usize, row: Value, score: Option<f32>) -> Result<SearchHit> {
    let obj = row
        .as_object()
        .ok_or_else(|| anyhow!("search row is not a JSON object"))?;

    let passage_id = get_str(obj, "passage_id")?;
    let zh_text_raw = obj
        .get("zh_text_raw")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let snippet = zh_text_raw.chars().take(160).collect::<String>();

    Ok(SearchHit {
        hit_id: make_hit_id(rank),
        rank,
        passage_id: passage_id.to_string(),
        score,
        source_rel_path: get_str_default(obj, "source_rel_path"),
        source_work_id: get_str_default(obj, "source_work_id"),
        xml_id: get_str_default(obj, "xml_id"),
        heading_path: get_str_default(obj, "heading_path"),
        citation: get_str_default(obj, "citation"),
        snippet,
        raw: row,
    })
}

fn get_str<'a>(obj: &'a serde_json::Map<String, Value>, key: &str) -> Result<&'a str> {
    obj.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing string field `{key}`"))
}

fn get_str_default(obj: &serde_json::Map<String, Value>, key: &str) -> String {
    obj.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}
