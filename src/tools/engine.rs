use crate::catalog_index::CorpusCatalogIndex;
use crate::datafusion_store::DataFusionStore;
use crate::document_table::DocumentTable;
use crate::phrase_index::PhraseIndex;
use crate::text_analyzer::{analyze, AnalyzeOptions, AnalyzeScratch, FilterMode};
use crate::tfidf::TfidfIndex;
use crate::tools::errors::ToolError;
use crate::tools::spec::{ToolSafety, ToolSpec};
use anyhow::Result;
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{OnceCell, Semaphore};

/// Configuration for the ToolEngine
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub pack: Option<PathBuf>,
    pub passages_parquet: Option<PathBuf>,
    pub phrase_index: Option<PathBuf>,
    pub tfidf_index: Option<PathBuf>,
    pub catalog_index: Option<PathBuf>,
    pub doc_table: Option<PathBuf>,
    pub registry: Option<PathBuf>,
    pub readonly: bool,
    pub allow_admin_tools: bool,
    pub output_root: Option<PathBuf>,
    pub max_heavy_concurrency: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            pack: None,
            passages_parquet: None,
            phrase_index: None,
            tfidf_index: None,
            catalog_index: None,
            doc_table: None,
            registry: None,
            readonly: false,
            allow_admin_tools: false,
            output_root: None,
            max_heavy_concurrency: 1,
        }
    }
}

/// The ToolEngine owns shared state and manages lazy-loaded resources
pub struct ToolEngine {
    pub config: EngineConfig,

    // Lazy-loaded heavy resources
    passages: OnceCell<Arc<DataFusionStore>>,
    phrase: OnceCell<Arc<PhraseIndex>>,
    tfidf: OnceCell<Arc<TfidfIndex>>,
    catalog: OnceCell<Arc<CorpusCatalogIndex>>,
    doc_table: OnceCell<Arc<DocumentTable>>,
    registry: OnceCell<Arc<()>>, // Placeholder - Registry may not exist yet

    heavy_slots: Semaphore,
}

fn expand_optional_filter(value: Option<&str>) -> Vec<String> {
    value
        .map(|v| crate::commands::search::expand_values(&[v.to_string()]))
        .unwrap_or_default()
}

/// Like `expand_optional_filter` but also resolves numeric tradition IDs.
fn expand_tradition_filter(value: Option<&str>) -> Vec<String> {
    expand_optional_filter(value)
        .into_iter()
        .map(|t| crate::taxonomy_legend::resolve_tradition(&t).to_string())
        .collect()
}

/// Like `expand_optional_filter` but also resolves numeric period IDs.
fn expand_period_filter(value: Option<&str>) -> Vec<String> {
    expand_optional_filter(value)
        .into_iter()
        .map(|p| crate::taxonomy_legend::resolve_period(&p).to_string())
        .collect()
}

/// Like `expand_optional_filter` but also resolves numeric origin IDs.
fn expand_origin_filter(value: Option<&str>) -> Vec<String> {
    expand_optional_filter(value)
        .into_iter()
        .map(|o| crate::taxonomy_legend::resolve_origin(&o).to_string())
        .collect()
}

fn exact_any_sql(where_parts: &mut Vec<String>, column: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    let quoted = values
        .iter()
        .map(|v| crate::datafusion_store::sql_literal(v))
        .collect::<Vec<_>>()
        .join(", ");
    where_parts.push(format!("{column} IN ({quoted})"));
}

fn tradition_contains_sql(column: &str, value: &str) -> String {
    let token = serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""));
    crate::datafusion_store::string_contains_sql(column, &token)
}

fn brief_row(row: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "passage_id": row.get("passage_id").and_then(|v| v.as_str()).unwrap_or(""),
        "source_work_id": row.get("source_work_id").and_then(|v| v.as_str()).unwrap_or(""),
        "main_title": row.get("main_title").and_then(|v| v.as_str()).unwrap_or(""),
        "heading": row.get("heading").and_then(|v| v.as_str()).unwrap_or(""),
        "period": row.get("period").and_then(|v| v.as_str()).unwrap_or(""),
        "zh_quote": row.get("zh_text_raw").and_then(|v| v.as_str()).unwrap_or(""),
    })
}

fn count_unique_ngrams_with_terms(
    text: &str,
    gram_len: usize,
    counts: &mut FxHashMap<u64, u32>,
    terms: &mut FxHashMap<u64, String>,
) -> u32 {
    if gram_len == 0 {
        return 0;
    }
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    if chars.len() < gram_len {
        return 0;
    }

    let mut seen = rustc_hash::FxHashSet::default();
    for window in chars.windows(gram_len) {
        let term: String = window.iter().collect();
        let hash = xxhash_rust::xxh3::xxh3_64(term.as_bytes());
        if seen.insert(hash) {
            *counts.entry(hash).or_insert(0) += 1;
            terms.entry(hash).or_insert(term);
        }
    }
    seen.len() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_filter_expands_csv_values() {
        assert_eq!(expand_optional_filter(None), Vec::<String>::new());
        assert_eq!(
            expand_optional_filter(Some("T, X, T")),
            vec!["T".to_string(), "X".to_string()]
        );
    }

    #[test]
    fn exact_any_uses_in_clause_for_multi_value_filters() {
        let mut where_parts = Vec::new();
        exact_any_sql(
            &mut where_parts,
            "period",
            &["Tang".to_string(), "Song".to_string()],
        );
        assert_eq!(where_parts, vec!["period IN ('Tang', 'Song')".to_string()]);
    }

    #[test]
    fn tradition_match_targets_json_array_token() {
        assert_eq!(
            tradition_contains_sql("traditions", "canon"),
            "strpos(traditions, '\"canon\"') > 0"
        );
    }
}

impl ToolEngine {
    pub async fn open(config: EngineConfig) -> Result<Self> {
        let max_heavy = config.max_heavy_concurrency.max(1);

        Ok(Self {
            config,
            passages: OnceCell::new(),
            phrase: OnceCell::new(),
            tfidf: OnceCell::new(),
            catalog: OnceCell::new(),
            doc_table: OnceCell::new(),
            registry: OnceCell::new(),
            heavy_slots: Semaphore::new(max_heavy),
        })
    }

    /// Resolve the passages parquet path
    pub fn resolve_passages_path(&self) -> Result<PathBuf> {
        if let Some(ref path) = self.config.passages_parquet {
            return Ok(path.clone());
        }

        if let Some(ref pack) = self.config.pack.as_ref() {
            let pack_path = pack.join(crate::pack::DEFAULT_PASSAGES);
            if pack_path.exists() {
                return Ok(pack_path);
            }
        }

        // Default path
        let default = PathBuf::from("data/passages.parquet");
        if default.exists() {
            return Ok(default);
        }

        Err(anyhow::anyhow!("Cannot resolve passages.parquet path"))
    }

    /// Resolve the phrase index path
    pub fn resolve_phrase_path(&self) -> Result<PathBuf> {
        if let Some(ref path) = self.config.phrase_index {
            return Ok(path.clone());
        }

        if let Some(pack) = self.config.pack.as_ref() {
            let pack_path = pack.join(crate::pack::DEFAULT_PHRASE);
            if pack_path.exists() {
                return Ok(pack_path);
            }
        }

        let default = PathBuf::from("data/derived/phrase_v3.index");
        if default.exists() {
            return Ok(default);
        }

        Err(anyhow::anyhow!("Cannot resolve phrase_v3.index path"))
    }

    /// Resolve the tfidf index path
    pub fn resolve_tfidf_path(&self) -> Result<PathBuf> {
        if let Some(ref path) = self.config.tfidf_index {
            return Ok(path.clone());
        }

        if let Some(pack) = self.config.pack.as_ref() {
            let pack_path = pack.join(crate::pack::DEFAULT_TFIDF);
            if pack_path.exists() {
                return Ok(pack_path);
            }
        }

        let default = PathBuf::from("data/derived/tfidf_v3.index");
        if default.exists() {
            return Ok(default);
        }

        Err(anyhow::anyhow!("Cannot resolve tfidf_v3.index path"))
    }

    /// Resolve a phrase index path if present, and validate it against the
    /// active doc table before any doc_id-bearing lookup uses it.
    pub async fn optional_phrase_path(&self) -> Result<Option<PathBuf>> {
        let Ok(path) = self.resolve_phrase_path() else {
            return Ok(None);
        };
        self.ensure_phrase_index_matches_doc_table(&path).await?;
        Ok(Some(path))
    }

    /// Resolve a TF-IDF path if present, and validate it against the active
    /// doc table before any doc_id-bearing lookup uses it.
    pub async fn optional_tfidf_path(&self) -> Result<Option<PathBuf>> {
        let Ok(path) = self.resolve_tfidf_path() else {
            return Ok(None);
        };
        self.ensure_tfidf_index_matches_doc_table(&path).await?;
        Ok(Some(path))
    }

    async fn ensure_phrase_index_matches_doc_table(&self, path: &Path) -> Result<()> {
        let info = PhraseIndex::header_info(path)?;
        let fingerprint = info
            .get("doc_table_fingerprint")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        self.ensure_index_matches_doc_table("phrase index", path, fingerprint)
            .await
    }

    async fn ensure_tfidf_index_matches_doc_table(&self, path: &Path) -> Result<()> {
        let info = TfidfIndex::header_info(path)?;
        let fingerprint = info
            .get("doc_table_fingerprint")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        self.ensure_index_matches_doc_table("TF-IDF index", path, fingerprint)
            .await
    }

    async fn ensure_index_matches_doc_table(
        &self,
        index_name: &str,
        index_path: &Path,
        fingerprint: &str,
    ) -> Result<()> {
        let doc_table_path = self.resolve_doc_table_path()?;
        let doc_table = self.doc_table().await?;
        let coverage = crate::document_table::match_index_fingerprint(
            &doc_table,
            &doc_table_path,
            fingerprint,
        )?;
        if coverage.is_none() {
            anyhow::bail!(
                "{} fingerprint does not match active doc_table; rebuild {} for {}",
                index_name,
                index_name,
                doc_table_path.display()
            );
        }
        if !index_path.exists() {
            anyhow::bail!("{} not found at {}", index_name, index_path.display());
        }
        Ok(())
    }

    /// Resolve the catalog index path
    pub fn resolve_catalog_path(&self) -> Result<PathBuf> {
        if let Some(ref path) = self.config.catalog_index {
            return Ok(path.clone());
        }

        if let Some(pack) = self.config.pack.as_ref() {
            let pack_path = pack.join(crate::pack::DEFAULT_CATALOG);
            if pack_path.exists() {
                return Ok(pack_path);
            }
        }

        let default = PathBuf::from("data/derived/catalog.index");
        if default.exists() {
            return Ok(default);
        }

        Err(anyhow::anyhow!("Cannot resolve catalog.index path"))
    }

    /// Resolve the doc table path
    pub fn resolve_doc_table_path(&self) -> Result<PathBuf> {
        if let Some(ref path) = self.config.doc_table {
            return Ok(path.clone());
        }

        if let Some(pack) = self.config.pack.as_ref() {
            let pack_path = pack.join(crate::pack::DEFAULT_DOC_TABLE);
            if pack_path.exists() {
                return Ok(pack_path);
            }
        }

        let default = PathBuf::from("data/derived/doc_table.bin");
        if default.exists() {
            return Ok(default);
        }

        Err(anyhow::anyhow!("Cannot resolve doc_table.bin path"))
    }

    /// Resolve the registry path
    pub fn resolve_registry_path(&self) -> Result<PathBuf> {
        if let Some(ref path) = self.config.registry {
            return Ok(path.clone());
        }

        if let Some(pack) = self.config.pack.as_ref() {
            let pack_path = pack.join(crate::pack::DEFAULT_REGISTRY);
            if pack_path.exists() {
                return Ok(pack_path);
            }
        }

        let default = PathBuf::from("data/derived/registry.sqlite");
        if default.exists() {
            return Ok(default);
        }

        Err(anyhow::anyhow!("Cannot resolve registry.sqlite path"))
    }

    /// Get or load the passages store
    pub async fn passages(&self) -> Result<Arc<DataFusionStore>> {
        self.passages
            .get_or_try_init(|| async {
                let path = self.resolve_passages_path()?;
                Ok(Arc::new(DataFusionStore::open(&path).await?))
            })
            .await
            .map(Clone::clone)
    }

    /// Get or load the phrase index
    pub async fn phrase(&self) -> Result<Arc<PhraseIndex>> {
        self.phrase
            .get_or_try_init(|| async {
                let path = self.resolve_phrase_path()?;
                self.ensure_phrase_index_matches_doc_table(&path).await?;
                Ok(Arc::new(PhraseIndex::open(&path)?))
            })
            .await
            .map(Clone::clone)
    }

    /// Get or load the tfidf index
    pub async fn tfidf(&self) -> Result<Arc<TfidfIndex>> {
        self.tfidf
            .get_or_try_init(|| async {
                let path = self.resolve_tfidf_path()?;
                self.ensure_tfidf_index_matches_doc_table(&path).await?;
                Ok(Arc::new(TfidfIndex::open(&path)?))
            })
            .await
            .map(Clone::clone)
    }

    /// Get or load the catalog index
    pub async fn catalog(&self) -> Result<Arc<CorpusCatalogIndex>> {
        self.catalog
            .get_or_try_init(|| async {
                let path = self.resolve_catalog_path()?;
                Ok(Arc::new(CorpusCatalogIndex::load(&path)?))
            })
            .await
            .map(Clone::clone)
    }

    /// Get or load the doc table
    pub async fn doc_table(&self) -> Result<Arc<DocumentTable>> {
        self.doc_table
            .get_or_try_init(|| async {
                let path = self.resolve_doc_table_path()?;
                Ok(Arc::new(DocumentTable::load(&path)?))
            })
            .await
            .map(Clone::clone)
    }

    /// Get or load the registry
    pub async fn registry(&self) -> Result<Arc<()>> {
        self.registry
            .get_or_try_init(|| async {
                // Placeholder - Registry may not exist yet
                Ok(Arc::new(()))
            })
            .await
            .map(Clone::clone)
    }

    /// Ensure write operations are allowed
    pub fn ensure_write_allowed(&self, tool: &str, output_path: &Path) -> Result<()> {
        if self.config.readonly {
            return Err(crate::tools::errors::ToolError::ReadonlyViolation {
                tool: tool.to_string(),
            }
            .into_anyhow());
        }

        // If output_root is set, ensure output path is under it
        if let Some(ref root) = self.config.output_root {
            Self::ensure_under_root(root, output_path)?;
        }

        Ok(())
    }

    /// Ensure a path is under a root directory
    fn ensure_under_root(root: &Path, path: &Path) -> Result<()> {
        let root = root
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("Cannot canonicalize root {}: {}", root.display(), e))?;

        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Output path has no parent: {}", path.display()))?;

        std::fs::create_dir_all(parent).map_err(|e| {
            anyhow::anyhow!("Cannot create parent directory {}: {}", parent.display(), e)
        })?;

        let parent = parent.canonicalize().map_err(|e| {
            anyhow::anyhow!("Cannot canonicalize parent {}: {}", parent.display(), e)
        })?;

        if !parent.starts_with(&root) {
            return Err(crate::tools::errors::ToolError::OutputPathViolation {
                path: path.to_path_buf(),
                root,
            }
            .into_anyhow());
        }

        Ok(())
    }

    /// Run a future with a heavy slot (for concurrency control)
    pub async fn with_heavy_slot<T, F>(&self, fut: F) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        let _permit = self.heavy_slots.acquire().await?;
        fut.await
    }

    /// Implement the status tool
    pub async fn status_impl(&self) -> Result<crate::tools::responses::StatusResponse> {
        use crate::tools::errors::ToolError;
        use crate::tools::responses::StatusResponse;
        use crate::tools::spec::{ToolSafety, ToolSpec};

        let data_root = self
            .config
            .pack
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "data".to_string());

        let passages_parquet_exists = self.resolve_passages_path().is_ok();
        let phrase_index_exists = self.resolve_phrase_path().is_ok();
        let tfidf_index_exists = self.resolve_tfidf_path().is_ok();
        let catalog_index_exists = self.resolve_catalog_path().is_ok();
        let doc_table_exists = self.resolve_doc_table_path().is_ok();
        let registry_exists = self.resolve_registry_path().is_ok();

        Ok(StatusResponse {
            schema: "sinoragd-status-v1",
            data_root,
            passages_parquet_exists,
            phrase_index_exists,
            tfidf_index_exists,
            catalog_index_exists,
            doc_table_exists,
            registry_exists,
        })
    }

    /// Implement the passage tool
    pub async fn passage_impl(
        &self,
        req: crate::tools::requests::PassageRequest,
    ) -> Result<crate::tools::responses::PassageResponse> {
        use crate::datafusion_store::sql_literal;
        use crate::tools::responses::PassageResponse;

        let passages = self.passages().await?;
        let sql = format!(
            "SELECT passage_id, zh_text_raw, source_work_id, main_title, heading, \
                    canon, period, traditions, origin, author, source_rel_path, xml_id \
             FROM passages WHERE passage_id = {} LIMIT 1",
            sql_literal(&req.id)
        );
        let mut rows = passages.query_json(&sql).await?;
        let row = rows
            .drain(..)
            .next()
            .ok_or_else(|| anyhow::anyhow!("Passage not found: {}", req.id))?;

        Ok(PassageResponse {
            schema: "sinoragd-passage-v1",
            passage_id: req.id.clone(),
            zh_quote: row
                .get("zh_text_raw")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            source_work_id: row
                .get("source_work_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            main_title: row
                .get("main_title")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            heading: row
                .get("heading")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        })
    }

    /// Implement the search tool
    pub async fn search_impl(
        &self,
        req: crate::tools::requests::SearchRequest,
    ) -> Result<crate::tools::responses::SearchResponse> {
        use crate::datafusion_store::{sql_literal, string_contains_sql};
        use crate::normalize::normalize_zh;
        use crate::research_tools::phrase::phrase_rows_with_explicit_doc_table;
        use crate::research_tools::scopes::{group_hits_by_outline_node, OutlineSearchLevel};
        use crate::tools::responses::{
            ClusterHitsCluster, SearchHit, SearchResponse, SearchStrategy, TermUsageGroup,
        };

        let passages = self.passages().await?;
        let canon = expand_optional_filter(req.canon.as_deref());
        let tradition = expand_tradition_filter(req.tradition.as_deref());
        let period = expand_period_filter(req.period.as_deref());
        let origin = expand_origin_filter(req.origin.as_deref());
        let normalized = normalize_zh(&req.phrase);
        let mode = match req.mode.as_str() {
            "clusters" | "trace" | "all" => req.mode.clone(),
            _ => "hits".to_string(),
        };
        let depth = match req.depth.as_str() {
            "expanded" | "reuse" => req.depth.as_str(),
            _ => "exact",
        };

        let mut phrases = vec![req.phrase.clone()];
        if req.include_variants || matches!(depth, "expanded" | "reuse") {
            let tables = crate::templates::variants::VariantTables::load();
            let mut seen = std::collections::BTreeSet::<String>::new();
            seen.insert(req.phrase.clone());
            for v in tables.term_variants(&req.phrase) {
                if seen.insert(v.clone()) {
                    phrases.push(v);
                }
            }
            let cur = phrases.clone();
            for p in cur {
                for v in tables.orthographic_flips(&p, 20) {
                    if seen.insert(v.clone()) {
                        phrases.push(v);
                    }
                }
            }
            phrases.truncate(20);
        }

        let doc_table = self.doc_table().await.ok();
        let phrase_index_path = if doc_table.is_some() {
            self.optional_phrase_path().await?
        } else {
            None
        };
        let catalog = self.catalog().await.ok();

        let doc_range = if let (Some(catalog), Some(work_id)) =
            (catalog.as_deref(), req.source_work_id.as_deref())
        {
            self.resolve_doc_range(catalog, None, Some(work_id))?
        } else {
            None
        };

        let canon_for_index = if canon.len() == 1 {
            Some(canon[0].as_str())
        } else {
            None
        };
        let period_for_index = if period.len() == 1 {
            Some(period[0].as_str())
        } else {
            None
        };
        let limit = req.limit.max(1);
        let per_phrase_limit = if phrases.len() > 1 {
            limit.saturating_mul(2).max(limit)
        } else {
            limit
        };

        let mut rows = Vec::<serde_json::Value>::new();
        let mut strategies = Vec::<serde_json::Value>::new();

        for phrase in &phrases {
            let phrase_rows = if let Some(doc_table) = doc_table.as_deref() {
                let (candidate_rows, strategy) = phrase_rows_with_explicit_doc_table(
                    &passages,
                    doc_table,
                    phrase_index_path.as_deref(),
                    phrase,
                    per_phrase_limit,
                    doc_range,
                    canon_for_index,
                    period_for_index,
                )
                .await?;
                strategies.push(serde_json::json!({
                    "phrase": phrase,
                    "normalized_phrase": normalize_zh(phrase),
                    "strategy": strategy,
                }));
                candidate_rows
            } else {
                let normalized_phrase = normalize_zh(phrase);
                let mut where_parts = Vec::new();
                if !normalized_phrase.is_empty() {
                    where_parts.push(string_contains_sql(
                        "zh_text_normalized",
                        &normalized_phrase,
                    ));
                }
                if let Some(canon) = canon_for_index {
                    where_parts.push(format!("canon = {}", sql_literal(canon)));
                }
                if let Some(period) = period_for_index {
                    where_parts.push(format!("period = {}", sql_literal(period)));
                }
                let where_sql = if where_parts.is_empty() {
                    "true".to_string()
                } else {
                    where_parts.join(" AND ")
                };
                let sql = format!(
                    "SELECT passage_id, source_rel_path, xml_id, div_path, heading, heading_path, \
                            from_lb, to_lb, zh_text_raw, zh_text_normalized, text_type, \
                            contains_person, contains_term, contains_foreign, canon, canon_name, \
                            traditions, period, origin, author, main_title, period_rank, \
                            source_corpus, source_work_id, source_section_id, source_locator, \
                            source_url, edition_siglum, edition_label, rights_id, rights_notes, \
                            retrieval_method, snapshot_id, quality_flags_json \
                     FROM passages \
                     WHERE {where_sql} \
                     ORDER BY period_rank ASC, source_rel_path ASC, from_lb ASC, xml_id ASC \
                     LIMIT {per_phrase_limit}"
                );
                strategies.push(serde_json::json!({
                    "phrase": phrase,
                    "normalized_phrase": normalized_phrase,
                    "strategy": {
                        "used_phrase_index": false,
                        "scope_scan": "parquet_global_no_doc_table",
                        "limit": per_phrase_limit,
                    },
                }));
                passages.query_json(&sql).await?
            };
            rows.extend(phrase_rows);
        }

        rows.retain(|row| {
            if !canon.is_empty() {
                let value = row.get("canon").and_then(|v| v.as_str()).unwrap_or("");
                if !canon.iter().any(|c| c == value) {
                    return false;
                }
            }
            if !period.is_empty() {
                let value = row.get("period").and_then(|v| v.as_str()).unwrap_or("");
                if !period.iter().any(|p| p == value) {
                    return false;
                }
            }
            if !origin.is_empty() {
                let value = row.get("origin").and_then(|v| v.as_str()).unwrap_or("");
                if !origin.iter().any(|o| o == value) {
                    return false;
                }
            }
            if !tradition.is_empty() {
                let values = row.get("traditions").and_then(|v| v.as_array());
                let has_match = values
                    .map(|vals| {
                        vals.iter()
                            .filter_map(|v| v.as_str())
                            .any(|v| tradition.iter().any(|t| t == v))
                    })
                    .unwrap_or(false);
                if !has_match {
                    return false;
                }
            }
            if let Some(ref author) = req.author {
                let value = row
                    .get("author")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_lowercase();
                if !value.contains(&author.to_lowercase()) {
                    return false;
                }
            }
            if let Some(ref title) = req.title {
                let value = row
                    .get("main_title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_lowercase();
                if !value.contains(&title.to_lowercase()) {
                    return false;
                }
            }
            if let Some(ref work_id) = req.source_work_id {
                let value = row
                    .get("source_work_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if value != work_id {
                    return false;
                }
            }
            if let Some(ref prefix) = req.heading_path_prefix {
                let value = row
                    .get("heading_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !value.starts_with(prefix) {
                    return false;
                }
            }
            true
        });

        let mut deduped = Vec::new();
        let mut seen = std::collections::BTreeSet::<String>::new();
        for row in rows {
            let key = row
                .get("passage_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if key.is_empty() || seen.insert(key) {
                deduped.push(row);
            }
        }
        if let Some(doc_table) = doc_table.as_deref() {
            deduped.sort_by_key(|row| {
                let pid = row.get("passage_id").and_then(|v| v.as_str()).unwrap_or("");
                doc_table.doc_id(pid).unwrap_or(u32::MAX)
            });
        } else {
            deduped.sort_by(|a, b| {
                let ak = (
                    a.get("period_rank").and_then(|v| v.as_i64()).unwrap_or(99),
                    a.get("source_rel_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                    a.get("from_lb").and_then(|v| v.as_str()).unwrap_or(""),
                    a.get("xml_id").and_then(|v| v.as_str()).unwrap_or(""),
                );
                let bk = (
                    b.get("period_rank").and_then(|v| v.as_i64()).unwrap_or(99),
                    b.get("source_rel_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                    b.get("from_lb").and_then(|v| v.as_str()).unwrap_or(""),
                    b.get("xml_id").and_then(|v| v.as_str()).unwrap_or(""),
                );
                ak.cmp(&bk)
            });
        }
        let verified_count = deduped.len();
        deduped.truncate(limit);

        let hits: Vec<SearchHit> = deduped
            .iter()
            .map(|row| SearchHit {
                passage_id: row
                    .get("passage_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                source_work_id: row
                    .get("source_work_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                main_title: row
                    .get("main_title")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                heading: row
                    .get("heading")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                zh_quote: row
                    .get("zh_text_raw")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .chars()
                    .take(if req.brief { 120 } else { usize::MAX })
                    .collect(),
                score: None,
            })
            .collect();

        let clusters = if matches!(mode.as_str(), "clusters" | "all") {
            if let (Some(catalog), Some(doc_table)) = (catalog.as_deref(), doc_table.as_deref()) {
                let target = match req.group_by.as_str() {
                    "division" => OutlineSearchLevel::Division,
                    _ => OutlineSearchLevel::Work,
                };
                let doc_rows: Vec<(u32, serde_json::Value)> = deduped
                    .iter()
                    .filter_map(|row| {
                        let pid = row.get("passage_id").and_then(|v| v.as_str())?;
                        Some((doc_table.doc_id(pid)?, row.clone()))
                    })
                    .collect();
                let doc_ids: Vec<u32> = doc_rows.iter().map(|(did, _)| *did).collect();
                let mut sorted_groups: Vec<(u32, u32)> =
                    group_hits_by_outline_node(catalog, &doc_ids, target)
                        .into_iter()
                        .collect();
                sorted_groups.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                Some(
                    sorted_groups
                        .into_iter()
                        .take(req.limit_per_group)
                        .map(|(node_id, count)| {
                            let node = catalog.get_node(node_id);
                            let node_doc_range =
                                node.and_then(|n| n.first_doc_id.zip(n.last_doc_id));
                            let representative_passages = doc_rows
                                .iter()
                                .filter(|(did, _)| {
                                    if let Some((lo, hi)) = node_doc_range {
                                        *did >= lo && *did <= hi
                                    } else {
                                        false
                                    }
                                })
                                .take(3)
                                .map(|(did, row)| {
                                    let mut r = row.clone();
                                    if let Some(obj) = r.as_object_mut() {
                                        obj.insert("doc_id".to_string(), serde_json::json!(*did));
                                    }
                                    if req.brief {
                                        brief_row(&r)
                                    } else {
                                        r
                                    }
                                })
                                .collect();
                            ClusterHitsCluster {
                                node_id,
                                label: node.map(|n| n.label.clone()).unwrap_or_default(),
                                heading_path: node
                                    .map(|n| n.heading_path.clone())
                                    .unwrap_or_default(),
                                node_kind: node
                                    .map(|n| format!("{:?}", &n.node_kind))
                                    .unwrap_or_default(),
                                hit_count: count,
                                representative_passages,
                            }
                        })
                        .collect(),
                )
            } else {
                let key_field = match req.group_by.as_str() {
                    "division" => "heading_path",
                    _ => "source_work_id",
                };
                let mut groups =
                    std::collections::BTreeMap::<String, (u32, Vec<serde_json::Value>)>::new();
                for row in &deduped {
                    let key = row
                        .get(key_field)
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .or_else(|| row.get("source_work_id").and_then(|v| v.as_str()))
                        .unwrap_or("(unknown)")
                        .to_string();
                    let acc = groups.entry(key).or_insert_with(|| (0, Vec::new()));
                    acc.0 += 1;
                    if acc.1.len() < 3 {
                        acc.1.push(if req.brief {
                            brief_row(row)
                        } else {
                            row.clone()
                        });
                    }
                }
                let mut sorted_groups: Vec<(String, u32, Vec<serde_json::Value>)> = groups
                    .into_iter()
                    .map(|(key, (count, reps))| (key, count, reps))
                    .collect();
                sorted_groups.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                Some(
                    sorted_groups
                        .into_iter()
                        .take(req.limit_per_group)
                        .enumerate()
                        .map(
                            |(idx, (label, count, representative_passages))| ClusterHitsCluster {
                                node_id: idx as u32,
                                label: label.clone(),
                                heading_path: label,
                                node_kind: "MetadataFallback".to_string(),
                                hit_count: count,
                                representative_passages,
                            },
                        )
                        .collect(),
                )
            }
        } else {
            None
        };

        let trace_groups = if matches!(mode.as_str(), "trace" | "all") {
            let key_field = match req.group_by.as_str() {
                "canon" => "canon",
                "author" => "author",
                "work" | "division" => "source_work_id",
                _ => "period",
            };
            struct GroupAcc {
                hit_count: u32,
                work_ids: std::collections::BTreeSet<String>,
                reps: Vec<(i32, u32, serde_json::Value)>,
            }
            impl Default for GroupAcc {
                fn default() -> Self {
                    Self {
                        hit_count: 0,
                        work_ids: std::collections::BTreeSet::new(),
                        reps: Vec::new(),
                    }
                }
            }
            let mut groups = std::collections::BTreeMap::<String, GroupAcc>::new();
            for row in &deduped {
                let key = row
                    .get(key_field)
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unknown)")
                    .to_string();
                let acc = groups.entry(key).or_default();
                acc.hit_count += 1;
                if let Some(wid) = row.get("source_work_id").and_then(|v| v.as_str()) {
                    acc.work_ids.insert(wid.to_string());
                }
                let did = doc_table
                    .as_deref()
                    .and_then(|dt| {
                        row.get("passage_id")
                            .and_then(|v| v.as_str())
                            .and_then(|pid| dt.doc_id(pid))
                    })
                    .unwrap_or(u32::MAX);
                let pr = row.get("period_rank").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                acc.reps.push((pr, did, row.clone()));
            }
            Some(
                groups
                    .into_iter()
                    .map(|(key, mut acc)| {
                        acc.reps.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
                        let mut top_works: Vec<String> = acc.work_ids.into_iter().collect();
                        top_works.sort();
                        top_works.truncate(req.limit_per_group);
                        TermUsageGroup {
                            key,
                            hit_count: acc.hit_count,
                            work_count: top_works.len(),
                            top_works,
                            representative_passages: acc
                                .reps
                                .into_iter()
                                .take(req.limit_per_group)
                                .map(|(_, _, row)| row)
                                .collect(),
                        }
                    })
                    .collect(),
            )
        } else {
            None
        };

        let used_phrase_index = strategies.iter().any(|s| {
            s.pointer("/strategy/used_phrase_index")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        });

        Ok(SearchResponse {
            schema: "sinoragd-search-v1",
            phrase: req.phrase,
            mode,
            brief: req.brief,
            expanded_phrases: phrases,
            hits,
            clusters,
            trace_groups,
            search_strategy: SearchStrategy {
                method: if used_phrase_index {
                    "phrase_index_verified_by_parquet".to_string()
                } else {
                    "parquet_strpos_scan".to_string()
                },
                filters: serde_json::json!({
                    "canon": canon,
                    "tradition": tradition,
                    "period": period,
                    "origin": origin,
                    "author": req.author,
                    "title": req.title,
                    "source_work_id": req.source_work_id,
                    "heading_path_prefix": req.heading_path_prefix,
                    "mode": req.mode,
                    "depth": req.depth,
                    "group_by": req.group_by,
                    "include_variants": req.include_variants,
                    "brief": req.brief,
                    "normalized_phrase": normalized,
                    "limit": limit,
                    "limit_per_group": req.limit_per_group,
                    "layers": strategies,
                }),
                candidate_count: None,
                verified_count: Some(verified_count),
            },
        })
    }

    /// Implement the canonical-source tool
    pub async fn canonical_source_impl(
        &self,
        req: crate::tools::requests::CanonicalSourceRequest,
    ) -> Result<crate::tools::responses::CanonicalSourceResponse> {
        use crate::datafusion_store::{sql_literal, string_contains_sql};
        use crate::normalize::normalize_zh;
        use crate::tools::responses::{
            CanonicalSourceHit, CanonicalSourceResponse, SearchStrategy,
        };

        let passages = self.passages().await?;

        let normalized = normalize_zh(&req.phrase);
        let mut where_parts = Vec::new();
        if !normalized.is_empty() {
            where_parts.push(string_contains_sql("zh_text_normalized", &normalized));
        }

        let canon = expand_optional_filter(req.canon.as_deref());
        if canon.is_empty() {
            where_parts.push("canon IS NOT NULL AND canon != ''".to_string());
        } else {
            exact_any_sql(&mut where_parts, "canon", &canon);
        }

        let where_sql = if where_parts.is_empty() {
            "true".to_string()
        } else {
            where_parts.join(" AND ")
        };
        let limit = req.limit.max(1);
        let sql = format!(
            "SELECT passage_id, source_work_id, main_title, heading, zh_text_raw, \
                    canon, traditions, period, origin, period_rank, source_rel_path, from_lb, xml_id \
             FROM passages \
             WHERE {where_sql} \
             ORDER BY period_rank ASC, source_rel_path ASC, from_lb ASC, xml_id ASC \
             LIMIT {limit}"
        );

        let rows = passages.query_json(&sql).await?;

        let hits: Vec<CanonicalSourceHit> = rows
            .iter()
            .take(req.limit)
            .map(|row| CanonicalSourceHit {
                passage_id: row
                    .get("passage_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                source_work_id: row
                    .get("source_work_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                main_title: row
                    .get("main_title")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                heading: row
                    .get("heading")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                zh_quote: row
                    .get("zh_text_raw")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                is_canon_side: true,
            })
            .collect();

        let hit_count = hits.len();

        Ok(CanonicalSourceResponse {
            schema: "sinoragd-canonical-source-v1",
            phrase: req.phrase,
            hits,
            search_strategy: SearchStrategy {
                method: "full_text_with_tradition_filter".to_string(),
                filters: serde_json::json!({
                    "canon": req.canon,
                    "normalized_phrase": normalized,
                    "canonical_filter": if canon.is_empty() { "canon != ''" } else { "canon IN (...)" }
                }),
                candidate_count: Some(rows.len()),
                verified_count: Some(hit_count),
            },
        })
    }

    /// Implement the heading-search tool
    pub async fn heading_search_impl(
        &self,
        req: crate::tools::requests::HeadingSearchRequest,
    ) -> Result<crate::tools::responses::HeadingSearchResponse> {
        use crate::datafusion_store::{sql_literal, string_contains_sql};
        use crate::tools::responses::{HeadingSearchHit, HeadingSearchResponse, SearchStrategy};

        let passages = self.passages().await?;
        let canon = expand_optional_filter(req.canon.as_deref());
        let period = expand_period_filter(req.period.as_deref());
        let normalized_query = crate::normalize::normalize_zh(&req.query);
        let mut where_parts = Vec::new();
        if !req.query.is_empty() {
            where_parts.push(format!(
                "(strpos(lower(heading), lower({q})) > 0 OR strpos(lower(heading_path), lower({q})) > 0 OR {norm})",
                q = sql_literal(&req.query),
                norm = string_contains_sql("zh_text_normalized", &normalized_query),
            ));
        }
        exact_any_sql(&mut where_parts, "canon", &canon);
        exact_any_sql(&mut where_parts, "period", &period);
        if let Some(ref work_id) = req.source_work_id {
            where_parts.push(format!("source_work_id = {}", sql_literal(work_id)));
        }

        let where_sql = if where_parts.is_empty() {
            "true".to_string()
        } else {
            where_parts.join(" AND ")
        };
        let limit = req.limit.max(1);
        let sql = format!(
            "SELECT passage_id, source_rel_path, xml_id, div_path, heading, heading_path, \
                    from_lb, to_lb, zh_text_raw, zh_text_normalized, text_type, canon, \
                    canon_name, traditions, period, origin, author, main_title, period_rank, \
                    source_work_id, source_section_id, source_locator, source_url \
             FROM passages \
             WHERE {where_sql} \
             ORDER BY period_rank ASC, source_rel_path ASC, from_lb ASC, xml_id ASC \
             LIMIT {limit}"
        );
        let rows = passages.query_json(&sql).await?;
        let sections: Vec<HeadingSearchHit> = rows
            .iter()
            .map(|row| HeadingSearchHit {
                source_work_id: row
                    .get("source_work_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                main_title: row
                    .get("main_title")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                heading: row
                    .get("heading")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                heading_path: row
                    .get("heading_path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                passage_id: row
                    .get("passage_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                sample: row
                    .get("zh_text_raw")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .chars()
                    .take(if req.brief { 80 } else { 240 })
                    .collect(),
                metadata: if req.brief {
                    None
                } else {
                    Some(serde_json::json!({
                        "canon": row.get("canon"),
                        "period": row.get("period"),
                        "author": row.get("author"),
                        "source_rel_path": row.get("source_rel_path"),
                        "from_lb": row.get("from_lb"),
                        "to_lb": row.get("to_lb"),
                    }))
                },
            })
            .collect();
        Ok(HeadingSearchResponse {
            schema: "sinoragd-heading-search-v1",
            query: req.query,
            brief: req.brief,
            returned_count: sections.len(),
            sections,
            search_strategy: SearchStrategy {
                method: "heading_path_metadata_scan".to_string(),
                filters: serde_json::json!({
                    "canon": canon,
                    "period": period,
                    "source_work_id": req.source_work_id,
                    "normalized_query": normalized_query,
                    "limit": limit,
                    "brief": req.brief,
                }),
                candidate_count: None,
                verified_count: Some(rows.len()),
            },
        })
    }

    /// Implement the tool-docs tool
    pub async fn tool_docs_impl(
        &self,
        req: crate::tools::requests::ToolDocsRequest,
    ) -> Result<crate::tools::responses::ToolDocsResponse> {
        let docs = crate::tools::docs::docs_payload(req.tool.as_deref());
        Ok(crate::tools::responses::ToolDocsResponse {
            schema: "sinoragd-tool-docs-v1",
            tool: req.tool,
            docs,
        })
    }

    /// Implement the validate-adjudication tool
    pub async fn validate_adjudication_impl(
        &self,
        req: crate::tools::requests::ValidateAdjudicationRequest,
    ) -> Result<crate::tools::responses::ValidateAdjudicationResponse> {
        use crate::commands::validate;
        use crate::tools::responses::ValidateAdjudicationResponse;

        // validate::run returns Result<()> and prints to stdout
        // For now, we'll just call it and assume success if it doesn't error
        validate::run(req.path.clone())?;

        Ok(ValidateAdjudicationResponse {
            schema: "sinoragd-validate-adjudication-v1",
            path: req.path,
            valid: true,
            errors: vec![],
            warnings: vec![],
        })
    }

    /// Implement the graph-build tool
    pub async fn graph_build_impl(
        &self,
        req: crate::tools::requests::GraphBuildRequest,
    ) -> Result<crate::tools::responses::GraphBuildResponse> {
        use crate::commands::export;
        use crate::tools::responses::GraphBuildResponse;

        self.ensure_write_allowed("graph-build", &req.out)?;

        let graph_kind = match req.kind.as_str() {
            "evidence" => export::GraphKind::Evidence,
            "timeline" => export::GraphKind::Timeline,
            "lineage" => export::GraphKind::Lineage,
            other => anyhow::bail!(
                "unknown graph kind `{}`; expected evidence, timeline, or lineage",
                other
            ),
        };

        export::graph(
            req.input.clone(),
            Some(req.out.clone()),
            graph_kind,
            Some(req.name.clone()),
        )?;

        // Read back the graph to get counts
        let graph_content = std::fs::read_to_string(&req.out)?;
        let graph: serde_json::Value = serde_json::from_str(&graph_content)?;

        let node_count = graph["nodes"].as_array().map(|v| v.len()).unwrap_or(0);
        let edge_count = graph["edges"].as_array().map(|v| v.len()).unwrap_or(0);

        Ok(GraphBuildResponse {
            schema: "sinoragd-graph-build-v1",
            out: req.out,
            node_count,
            edge_count,
        })
    }

    /// Implement the report-build tool
    pub async fn report_build_impl(
        &self,
        req: crate::tools::requests::ReportBuildRequest,
    ) -> Result<crate::tools::responses::ReportBuildResponse> {
        use crate::commands::export;
        use crate::tools::responses::ReportBuildResponse;

        self.ensure_write_allowed("report-build", &req.out)?;

        export::report_build(
            req.inputs.clone(),
            req.out.clone(),
            req.title.clone(),
            req.essay_max_pages,
        )?;

        // Count sections in the generated markdown
        let content = std::fs::read_to_string(&req.out)?;
        let section_count = content.matches("##").count();

        Ok(ReportBuildResponse {
            schema: "sinoragd-report-build-v1",
            out: req.out,
            section_count,
        })
    }

    /// Implement the works tool
    pub async fn works_impl(
        &self,
        req: crate::tools::requests::WorksRequest,
    ) -> Result<crate::tools::responses::WorksResponse> {
        use crate::catalog_index::CorpusCatalogIndex;
        use crate::tools::responses::{WorkInfo, WorksResponse};

        let catalog_path = self.resolve_catalog_path()?;
        let catalog = CorpusCatalogIndex::load(&catalog_path)?;

        let mut filtered: Vec<_> = catalog.works.iter().collect();

        if let Some(ref tradition) = req.tradition {
            filtered.retain(|w| w.traditions.iter().any(|tr| tr == tradition));
        }
        if let Some(ref period) = req.period {
            filtered.retain(|w| &w.period == period);
        }
        if let Some(ref canon) = req.canon {
            filtered.retain(|w| &w.canon == canon);
        }
        if let Some(ref author) = req.author {
            filtered.retain(|w| &w.author == author);
        }

        filtered.truncate(req.limit);

        let works: Vec<WorkInfo> = filtered
            .iter()
            .map(|w| WorkInfo {
                work_id: w.work_id.clone(),
                main_title: w.main_title.clone(),
                author: Some(w.author.clone()),
                period: Some(w.period.clone()),
                canon: Some(w.canon.clone()),
                traditions: w.traditions.clone(),
                passage_count: w.passage_count as usize,
            })
            .collect();

        Ok(WorksResponse {
            schema: "sinoragd-works-v1",
            works,
        })
    }

    /// Implement the catalog-index-info tool
    pub async fn catalog_index_info_impl(
        &self,
        _req: crate::tools::requests::CatalogIndexInfoRequest,
    ) -> Result<crate::tools::responses::CatalogIndexInfoResponse> {
        use crate::catalog_index::CorpusCatalogIndex;
        use crate::tools::responses::CatalogIndexInfoResponse;

        let catalog_path = self.resolve_catalog_path()?;
        let catalog = CorpusCatalogIndex::load(&catalog_path)?;
        let info = catalog.info_payload();

        Ok(CatalogIndexInfoResponse {
            schema: "sinoragd-catalog-index-info-v1",
            info,
        })
    }

    /// Implement the similar tool
    pub async fn similar_impl(
        &self,
        req: crate::tools::requests::SimilarRequest,
    ) -> Result<crate::tools::responses::SimilarResponse> {
        use crate::commands::tfidf::similar_passages_with_index;
        use crate::document_table::DocumentTable;
        use crate::tools::responses::SimilarResponse;

        let passages = self.passages().await?;
        let tfidf = self.tfidf().await?;
        let doc_table = self.doc_table().await?;

        let similar_passages = similar_passages_with_index(
            &passages,
            &tfidf,
            &req.seed,
            req.limit,
            req.shared_ngram_limit,
            req.shared_phrase_limit,
            req.min_shared_phrase_len,
            &doc_table,
        )
        .await?;

        Ok(SimilarResponse {
            schema: "sinoragd-similar-v1",
            seed: req.seed,
            similar_passages,
        })
    }

    /// Implement the frontier tool
    pub async fn frontier_impl(
        &self,
        req: crate::tools::requests::FrontierRequest,
    ) -> Result<crate::tools::responses::FrontierResponse> {
        use crate::commands::frontier;
        use crate::commands::tfidf::similar_passages_with_index;
        use crate::document_table::DocumentTable;
        use crate::registry;
        use crate::tools::responses::FrontierResponse;

        let passages = self.passages().await?;
        let tfidf = self.tfidf().await?;
        let doc_table = self.doc_table().await?;
        let registry_path = self.resolve_registry_path()?;

        // Get seed passage
        let seed_row = passages.get_passage(&req.seed).await?;

        // Get similar passages
        let similar = similar_passages_with_index(
            &passages, &tfidf, &req.seed, req.limit, 12, // shared_ngram_limit
            8,  // shared_phrase_limit
            4,  // min_shared_phrase_len
            &doc_table,
        )
        .await?;

        // Get phrase frontiers
        let phrase_frontiers =
            frontier::phrase_frontiers(&passages, &seed_row, req.phrase_limit).await?;

        // Get prior work
        let prior_work = if registry_path.exists() {
            registry::prior_work(&registry_path, &req.seed, 10)?
        } else {
            Vec::new()
        };

        let payload = serde_json::json!({
            "schema": "readzen-graphdiscovery-frontier-v1",
            "seed_passage_id": req.seed,
            "seed": seed_row,
            "similar_passages": similar,
            "phrase_frontiers": phrase_frontiers,
            "facet_summary": frontier::facet_summary(&similar),
            "next_seed_candidates": frontier::next_seed_candidates(&similar),
            "prior_work": prior_work,
        });

        Ok(FrontierResponse {
            schema: "sinoragd-frontier-v1",
            seed_passage_id: req.seed,
            payload,
        })
    }

    /// Implement the first-attestation tool
    pub async fn first_attestation_impl(
        &self,
        req: crate::tools::requests::FirstAttestationRequest,
    ) -> Result<crate::tools::responses::FirstAttestationResponse> {
        use crate::research_tools::phrase::phrase_rows_with_explicit_doc_table;
        use crate::tools::responses::{FirstAttestationResponse, ScopeInfo, SearchStrategyInfo};

        let passages = self.passages().await?;
        let doc_table = self.doc_table().await?;
        let phrase_index_path = self.optional_phrase_path().await?;

        let internal_limit = req.limit.max(10_000);
        let (raw_hits, _) = phrase_rows_with_explicit_doc_table(
            &passages,
            &doc_table,
            phrase_index_path.as_deref(),
            &req.phrase,
            internal_limit,
            None,
            None,
            None,
        )
        .await?;

        // Apply scope_period and scope_source_work_id post-hoc
        let hits: Vec<serde_json::Value> = raw_hits
            .into_iter()
            .filter(|row| {
                if !req.scope_canon.is_empty() {
                    let c = row.get("canon").and_then(|v| v.as_str()).unwrap_or("");
                    if !req.scope_canon.iter().any(|s| s == c) {
                        return false;
                    }
                }
                if !req.scope_period.is_empty() {
                    let p = row.get("period").and_then(|v| v.as_str()).unwrap_or("");
                    if !req.scope_period.iter().any(|s| s == p) {
                        return false;
                    }
                }
                if let Some(work) = &req.scope_source_work_id {
                    let w = row
                        .get("source_work_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if w != work {
                        return false;
                    }
                }
                true
            })
            .collect();
        let verified = hits.len();

        // Sort by (period_rank, doc_id)
        let mut scored: Vec<(i32, u32, serde_json::Value)> = Vec::with_capacity(hits.len());
        for row in hits {
            let pid = row.get("passage_id").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(did) = doc_table.doc_id(pid) {
                let pr = doc_table
                    .period_ranks
                    .get(did as usize)
                    .copied()
                    .unwrap_or(0);
                scored.push((pr, did, row));
            }
        }
        scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

        let total = scored.len();
        let take = req.limit.min(total);
        let mut iter = scored.into_iter().take(take);
        let first = iter.next().map(|(pr, did, mut row)| {
            if let Some(obj) = row.as_object_mut() {
                obj.insert("period_rank".to_string(), serde_json::json!(pr));
                obj.insert("doc_id".to_string(), serde_json::json!(did));
            }
            row
        });
        let next_earlier: Vec<serde_json::Value> = iter
            .map(|(pr, did, mut row)| {
                if let Some(obj) = row.as_object_mut() {
                    obj.insert("period_rank".to_string(), serde_json::json!(pr));
                    obj.insert("doc_id".to_string(), serde_json::json!(did));
                }
                row
            })
            .collect();

        Ok(FirstAttestationResponse {
            schema: "sinoragd-first-attestation-v1",
            phrase: req.phrase,
            first,
            next_earlier,
            scope: ScopeInfo {
                canon: req.scope_canon,
                period: req.scope_period,
                source_work_id: req.scope_source_work_id,
            },
            search_strategy: SearchStrategyInfo {
                used_phrase_index: phrase_index_path.is_some(),
                candidates_verified: verified,
                after_scope_and_sort: total,
                limit: req.limit,
            },
        })
    }

    /// Implement the phrase-history tool
    pub async fn phrase_history_impl(
        &self,
        req: crate::tools::requests::PhraseHistoryRequest,
    ) -> Result<crate::tools::responses::PhraseHistoryResponse> {
        use crate::commands::phrase_history;
        use crate::tools::responses::PhraseHistoryResponse;

        let passages = self.passages().await?;
        let phrase_index_path = self.optional_phrase_path().await?;

        let payload = phrase_history::phrase_history(
            req.phrase,
            &passages,
            req.include_variants,
            req.timeline,
            phrase_index_path,
        )
        .await?;

        Ok(PhraseHistoryResponse {
            schema: "sinoragd-phrase-history-v1",
            payload,
        })
    }

    /// Implement the phrase-index-search tool
    pub async fn phrase_index_search_impl(
        &self,
        req: crate::tools::requests::PhraseIndexSearchRequest,
    ) -> Result<crate::tools::responses::PhraseIndexSearchResponse> {
        use crate::research_tools::phrase::phrase_rows_with_explicit_doc_table;
        use crate::tools::responses::PhraseIndexSearchResponse;

        let passages = self.passages().await?;
        let doc_table = self.doc_table().await?;
        let phrase_index_path = self.optional_phrase_path().await?;

        if phrase_index_path.is_none() {
            return Err(ToolError::MissingPhraseIndex {
                path: self
                    .config
                    .phrase_index
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("data/derived/phrase_v3.index")),
            }
            .into_anyhow());
        }

        let (rows, _) = phrase_rows_with_explicit_doc_table(
            &passages,
            &doc_table,
            phrase_index_path.as_deref(),
            &req.phrase,
            req.limit,
            None,
            None,
            None,
        )
        .await?;

        Ok(PhraseIndexSearchResponse {
            schema: "sinoragd-phrase-index-search-v1",
            phrase: req.phrase,
            returned_count: rows.len(),
            limit: req.limit.max(1),
            results: rows,
        })
    }

    /// Implement the seed-pick tool
    pub async fn seed_pick_impl(
        &self,
        req: crate::tools::requests::SeedPickRequest,
    ) -> Result<crate::tools::responses::SeedPickResponse> {
        use crate::commands::seed_pick;
        use crate::datafusion_store::{sql_literal, string_contains_sql};
        use crate::tools::responses::{FilterInfo, SeedPickResponse};

        let passages = self.passages().await?;
        let registry_path = self.resolve_registry_path().ok();

        // Get already worked passage IDs from registry
        let mut already_worked: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        if let Some(ref path) = registry_path {
            if path.exists() {
                if let Ok(con) = rusqlite::Connection::open(path) {
                    if let Ok(mut stmt) = con.prepare(
                        "SELECT DISTINCT seed_passage_id FROM seed_observations WHERE seed_passage_id != ''",
                    ) {
                        if let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) {
                            for row in rows.flatten() {
                                already_worked.insert(row);
                            }
                        }
                    }
                }
            }
        }

        // Build WHERE clauses
        let mut where_clauses = vec!["true".to_string()];
        for t in &req.tradition {
            let t = crate::taxonomy_legend::resolve_tradition(t);
            where_clauses.push(tradition_contains_sql("traditions", t));
        }
        for p in &req.period {
            let p = crate::taxonomy_legend::resolve_period(p);
            where_clauses.push(format!("period = {}", sql_literal(p)));
        }
        if !already_worked.is_empty() {
            let id_list = already_worked
                .iter()
                .map(|pid| sql_literal(pid))
                .collect::<Vec<_>>()
                .join(", ");
            where_clauses.push(format!("passage_id NOT IN ({})", id_list));
        }

        let sql = format!(
            r#"
            SELECT passage_id, source_rel_path, xml_id, heading, from_lb, to_lb,
                   zh_text_raw, canon, canon_name, traditions, period, origin, author, main_title,
                   period_rank
            FROM passages
            WHERE {}
            ORDER BY period_rank ASC, source_rel_path ASC, from_lb ASC
            LIMIT {}
            "#,
            where_clauses.join(" AND "),
            req.limit.max(1)
        );

        let results = passages.query_json(&sql).await?;

        Ok(SeedPickResponse {
            schema: "sinoragd-seed-pick-v1",
            limit: req.limit,
            already_worked_count: already_worked.len(),
            filters: FilterInfo {
                tradition: req.tradition,
                period: req.period,
            },
            candidates: results,
        })
    }

    /// Implement the expand-context-adaptive tool
    pub async fn expand_context_adaptive_impl(
        &self,
        req: crate::tools::requests::ExpandContextAdaptiveRequest,
    ) -> Result<crate::tools::responses::ExpandContextAdaptiveResponse> {
        use crate::catalog_index::OutlineNodeKind;
        use crate::tools::responses::{ExpandContextAdaptiveResponse, SearchStrategyInfoAdaptive};

        let catalog = self.catalog().await?;
        let doc_table = self.doc_table().await?;
        let passages = self.passages().await?;

        // doc_id lookup
        let doc_id = doc_table
            .doc_id(&req.passage_id)
            .ok_or_else(|| anyhow::anyhow!("passage not found in doc_table: {}", req.passage_id))?;
        let mut node_id = *catalog.doc_parent.get(&doc_id).ok_or_else(|| {
            anyhow::anyhow!("doc_id {} has no catalog node (rebuild catalog?)", doc_id)
        })?;

        let leaf_kind = catalog
            .get_node(node_id)
            .map(|n| format!("{:?}", n.node_kind))
            .unwrap_or_default();
        let mut climbed = 0u32;

        // Climb until the node's cjk_char_count fits the budget or we reach Work
        let mut prev_node_id = node_id;
        loop {
            let node = catalog
                .get_node(node_id)
                .ok_or_else(|| anyhow::anyhow!("bad node_id"))?;
            let fits = (node.cjk_char_count as usize) <= req.max_chars;
            let at_work = matches!(node.node_kind, OutlineNodeKind::Work);
            if fits || at_work {
                break;
            }
            match node.parent_id {
                Some(parent) => {
                    let parent_node = catalog
                        .get_node(parent)
                        .ok_or_else(|| anyhow::anyhow!("bad parent_id"))?;
                    if matches!(
                        parent_node.node_kind,
                        OutlineNodeKind::Canon | OutlineNodeKind::Corpus
                    ) {
                        node_id = prev_node_id;
                        break;
                    }
                    prev_node_id = node_id;
                    node_id = parent;
                    climbed += 1;
                }
                None => break,
            }
        }

        let selected = catalog
            .get_node(node_id)
            .ok_or_else(|| anyhow::anyhow!("no selected node"))?;
        let first = selected
            .first_doc_id
            .ok_or_else(|| anyhow::anyhow!("node has no doc range"))?;
        let last = selected
            .last_doc_id
            .ok_or_else(|| anyhow::anyhow!("node has no doc range"))?;

        // Fetch every passage with doc_id in [first, last] for the selected work
        let mut passage_ids: Vec<String> = Vec::with_capacity((last - first + 1) as usize);
        for did in first..=last {
            if let Some(pid) = doc_table.passage_id(did) {
                passage_ids.push(pid.to_string());
            }
        }

        let rows = passages
            .passages_by_ids(
                &passage_ids,
                "passage_id, main_title, source_work_id, source_rel_path, \
             from_lb, to_lb, period, zh_text_normalized as zh_text",
            )
            .await?;

        let char_count: usize = rows
            .iter()
            .filter_map(|r| r.get("zh_text").and_then(|v| v.as_str()))
            .map(|t| t.chars().count())
            .sum();

        Ok(ExpandContextAdaptiveResponse {
            schema: "sinoragd-expand-context-adaptive-v1",
            seed_passage_id: req.passage_id,
            selected_node_id: selected.node_id,
            selected_node_kind: format!("{:?}", selected.node_kind),
            selected_label: selected.label.clone(),
            heading_path: vec![selected.heading_path.clone()],
            work_id: Some(selected.work_id.clone()),
            passage_count: rows.len(),
            char_count,
            passages: rows,
            search_strategy: SearchStrategyInfoAdaptive {
                budget: req.max_chars,
                climbed_levels: climbed,
                leaf_kind,
                mode: "auto".to_string(),
            },
        })
    }

    /// Implement the trace-term-usage tool
    pub async fn trace_term_usage_impl(
        &self,
        req: crate::tools::requests::TraceTermUsageRequest,
    ) -> Result<crate::tools::responses::TraceTermUsageResponse> {
        use crate::research_tools::phrase::phrase_rows_with_explicit_doc_table;
        use crate::tools::responses::{
            TermUsageGroup, TermUsageSearchStrategy, TraceTermUsageResponse,
        };

        let passages = self.passages().await?;
        let doc_table = self.doc_table().await?;
        let phrase_index_path = self.optional_phrase_path().await?;

        let key_field = match req.group_by.as_str() {
            "period" => "period",
            "canon" => "canon",
            "author" => "author",
            "work" => "source_work_id",
            _ => "period",
        };

        let (hits, _) = phrase_rows_with_explicit_doc_table(
            &passages,
            &doc_table,
            phrase_index_path.as_deref(),
            &req.phrase,
            req.limit_total,
            None,
            None,
            None,
        )
        .await?;
        let total_hits = hits.len();

        // Group by the chosen field
        use std::collections::BTreeMap;
        struct GroupAcc {
            hit_count: u32,
            work_ids: std::collections::BTreeSet<String>,
            reps: Vec<(i32, u32, serde_json::Value)>,
        }
        impl Default for GroupAcc {
            fn default() -> Self {
                Self {
                    hit_count: 0,
                    work_ids: std::collections::BTreeSet::new(),
                    reps: Vec::new(),
                }
            }
        }

        let mut groups: BTreeMap<String, GroupAcc> = BTreeMap::new();
        for row in hits {
            let key = row
                .get(key_field)
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)")
                .to_string();
            let acc = groups.entry(key).or_insert_with(GroupAcc::default);
            acc.hit_count += 1;
            if let Some(wid) = row.get("source_work_id").and_then(|v| v.as_str()) {
                acc.work_ids.insert(wid.to_string());
            }
            let pid = row.get("passage_id").and_then(|v| v.as_str()).unwrap_or("");
            let did = doc_table.doc_id(pid).unwrap_or(u32::MAX);
            let pr = if did != u32::MAX {
                doc_table
                    .period_ranks
                    .get(did as usize)
                    .copied()
                    .unwrap_or(0)
            } else {
                0
            };
            acc.reps.push((pr, did, row));
        }

        let mut out_groups: Vec<TermUsageGroup> = Vec::with_capacity(groups.len());
        for (key, mut acc) in groups {
            acc.reps.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
            let reps: Vec<serde_json::Value> = acc
                .reps
                .into_iter()
                .take(req.limit_per_group)
                .map(|(_, _, r)| r)
                .collect();
            let mut top_works: Vec<String> = acc.work_ids.into_iter().collect();
            top_works.sort();
            top_works.truncate(req.limit_per_group);
            out_groups.push(TermUsageGroup {
                key,
                hit_count: acc.hit_count,
                work_count: top_works.len(),
                top_works,
                representative_passages: reps,
            });
        }

        Ok(TraceTermUsageResponse {
            schema: "sinoragd-term-usage-trace-v1",
            phrase: req.phrase,
            group_by: req.group_by,
            groups: out_groups,
            search_strategy: TermUsageSearchStrategy {
                used_phrase_index: phrase_index_path.is_some(),
                total_hits,
                limit_total: req.limit_total,
                limit_per_group: req.limit_per_group,
            },
        })
    }

    /// Implement the query-expand-terms tool
    pub async fn query_expand_terms_impl(
        &self,
        req: crate::tools::requests::QueryExpandTermsRequest,
    ) -> Result<crate::tools::responses::QueryExpandTermsResponse> {
        use crate::templates::variants::VariantTables;
        use crate::tools::responses::{
            ExpandTermsBySource, ExpandTermsSearchStrategy, QueryExpandTermsResponse,
        };

        let tables = VariantTables::load();

        let mut variants_bucket = std::collections::BTreeSet::<String>::new();
        let mut orthographic_bucket = std::collections::BTreeSet::<String>::new();
        let mut persons_bucket = std::collections::BTreeSet::<String>::new();

        let expand_variants = matches!(req.mode.as_str(), "variants" | "all");
        let expand_orthographic = matches!(req.mode.as_str(), "orthographic" | "all");
        let expand_persons = matches!(req.mode.as_str(), "persons" | "all");

        if expand_variants {
            for v in tables.term_variants(&req.phrase) {
                if v != req.phrase {
                    variants_bucket.insert(v);
                }
            }
        }
        if expand_orthographic {
            for v in tables.orthographic_flips(&req.phrase, req.max * 2) {
                orthographic_bucket.insert(v);
            }
            // Also flip every term in the variants bucket so we cover cross-Han variants
            let cur: Vec<String> = variants_bucket.iter().cloned().collect();
            for v in cur {
                for f in tables.orthographic_flips(&v, req.max) {
                    if !variants_bucket.contains(&f) {
                        orthographic_bucket.insert(f);
                    }
                }
            }
        }
        if expand_persons {
            for a in &req.person_aliases {
                if !a.is_empty() && a != &req.phrase {
                    persons_bucket.insert(a.clone());
                }
            }
        }

        // Combined view (deduped, capped)
        let mut combined: Vec<String> = Vec::new();
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        seen.insert(req.phrase.clone());
        for v in variants_bucket
            .iter()
            .chain(orthographic_bucket.iter())
            .chain(persons_bucket.iter())
        {
            if seen.insert(v.clone()) {
                combined.push(v.clone());
                if combined.len() >= req.max {
                    break;
                }
            }
        }

        // Detect language
        let mut has_han = false;
        let mut has_latin = false;
        for ch in req.phrase.chars() {
            if (0x4E00..=0x9FFF).contains(&(ch as u32))
                || (0x3400..=0x4DBF).contains(&(ch as u32))
                || (0xF900..=0xFAFF).contains(&(ch as u32))
            {
                has_han = true;
            }
            if ch.is_ascii_alphabetic() {
                has_latin = true;
            }
        }
        let lang_guess = match (has_han, has_latin) {
            (true, false) => "zh",
            (false, true) => "en",
            (true, true) => "mixed",
            _ => "unknown",
        };

        Ok(QueryExpandTermsResponse {
            schema: "sinoragd-query-expand-terms-v1",
            input: req.phrase,
            expanded: combined,
            by_source: ExpandTermsBySource {
                variants: variants_bucket.into_iter().collect::<Vec<_>>(),
                orthographic: orthographic_bucket.into_iter().collect::<Vec<_>>(),
                persons: persons_bucket.into_iter().collect::<Vec<_>>(),
            },
            search_strategy: ExpandTermsSearchStrategy {
                mode: req.mode,
                max: req.max,
                input_lang_guess: lang_guess.to_string(),
            },
        })
    }

    /// Implement the compare-usage tool
    pub async fn compare_usage_impl(
        &self,
        req: crate::tools::requests::CompareUsageRequest,
    ) -> Result<crate::tools::responses::CompareUsageResponse> {
        use crate::research_tools::stats::log_odds_distinctive_terms;
        use crate::tools::responses::{
            CompareUsageResponse, CompareUsageScope, CompareUsageSearchStrategy, CompareUsageTerm,
        };

        let catalog = self.catalog().await?;
        let doc_table = self.doc_table().await?;
        let passages = self.passages().await?;

        // Resolve doc ranges for scopes
        let range_a = self.resolve_doc_range(
            &catalog,
            req.scope_a_node_id,
            req.scope_a_work_id.as_deref(),
        )?;
        let range_b = self.resolve_doc_range(
            &catalog,
            req.scope_b_node_id,
            req.scope_b_work_id.as_deref(),
        )?;

        let (a_terms, a_term_display, a_passage_count) = self
            .collect_scope_terms(
                &passages,
                &doc_table,
                range_a,
                req.scope_a_canon.as_deref(),
                req.scope_a_period.as_deref(),
                req.limit_passages,
                req.gram_len,
            )
            .await?;

        let (b_terms, b_term_display, b_passage_count) = self
            .collect_scope_terms(
                &passages,
                &doc_table,
                range_b,
                req.scope_b_canon.as_deref(),
                req.scope_b_period.as_deref(),
                req.limit_passages,
                req.gram_len,
            )
            .await?;

        let (a_top, b_top) = log_odds_distinctive_terms(&a_terms, &b_terms, req.limit_terms);

        let distinctive_to_a: Vec<CompareUsageTerm> = a_top
            .iter()
            .map(|t| CompareUsageTerm {
                term: a_term_display
                    .get(&t.term_hash)
                    .or_else(|| b_term_display.get(&t.term_hash))
                    .cloned(),
                term_hash: t.term_hash,
                score: t.score,
                a_count: t.a_count,
                b_count: t.b_count,
            })
            .collect();

        let distinctive_to_b: Vec<CompareUsageTerm> = b_top
            .iter()
            .map(|t| CompareUsageTerm {
                term: b_term_display
                    .get(&t.term_hash)
                    .or_else(|| a_term_display.get(&t.term_hash))
                    .cloned(),
                term_hash: t.term_hash,
                score: t.score,
                a_count: t.a_count,
                b_count: t.b_count,
            })
            .collect();

        Ok(CompareUsageResponse {
            schema: "sinoragd-compare-usage-v1",
            scope_a: CompareUsageScope {
                node_id: req.scope_a_node_id,
                work_id: req.scope_a_work_id,
                canon: req.scope_a_canon,
                period: req.scope_a_period,
                passage_count: a_passage_count,
            },
            scope_b: CompareUsageScope {
                node_id: req.scope_b_node_id,
                work_id: req.scope_b_work_id,
                canon: req.scope_b_canon,
                period: req.scope_b_period,
                passage_count: b_passage_count,
            },
            distinctive_to_a,
            distinctive_to_b,
            search_strategy: CompareUsageSearchStrategy {
                gram_len: req.gram_len,
                limit_passages: req.limit_passages,
                limit_terms: req.limit_terms,
            },
        })
    }

    /// Implement the collocation-search tool
    pub async fn collocation_search_impl(
        &self,
        req: crate::tools::requests::CollocationSearchRequest,
    ) -> Result<crate::tools::responses::CollocationSearchResponse> {
        use crate::normalize::normalize_zh;
        use crate::research_tools::phrase::phrase_rows_with_explicit_doc_table;
        use crate::research_tools::stats::score_collocates;
        use crate::tools::responses::{
            CollocateTerm, CollocationSearchResponse, CollocationSearchStrategy,
        };

        let doc_table = self.doc_table().await?;
        let passages = self.passages().await?;
        let phrase_index_path = self.optional_phrase_path().await?;

        let (hits, phrase_strategy) = phrase_rows_with_explicit_doc_table(
            &passages,
            &doc_table,
            phrase_index_path.as_deref(),
            &req.phrase,
            req.limit_total,
            None,
            None,
            None,
        )
        .await?;

        let normalized_phrase = normalize_zh(&req.phrase);

        // Collect n-gram hashes from the window around each phrase occurrence
        let mut near_counts: FxHashMap<u64, u32> = FxHashMap::default();
        let mut background_counts: FxHashMap<u64, u32> = FxHashMap::default();
        let mut term_display: FxHashMap<u64, String> = FxHashMap::default();
        let mut near_total = 0u32;
        let mut bg_total = 0u32;

        for row in &hits {
            let norm = row
                .get("zh_text_normalized")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // Background: all n-grams in the passage
            bg_total += count_unique_ngrams_with_terms(
                norm,
                req.gram_len,
                &mut background_counts,
                &mut term_display,
            );

            // Near: n-grams within the window around each occurrence
            if normalized_phrase.is_empty() {
                continue;
            }
            let mut search_start = 0usize;
            while let Some(rel_pos) = norm[search_start..].find(&normalized_phrase) {
                let byte_pos = search_start + rel_pos;
                let char_pos = norm[..byte_pos].chars().count();
                let phrase_chars = normalized_phrase.chars().count();
                let window_start = char_pos.saturating_sub(req.window_chars);
                let window_end =
                    (char_pos + phrase_chars + req.window_chars).min(norm.chars().count());

                let window_text: String = norm
                    .chars()
                    .skip(window_start)
                    .take(window_end - window_start)
                    .collect();
                near_total += count_unique_ngrams_with_terms(
                    &window_text,
                    req.gram_len,
                    &mut near_counts,
                    &mut term_display,
                );
                search_start = byte_pos + normalized_phrase.len();
            }
        }

        let collocates = score_collocates(&near_counts, &background_counts, req.limit_collocates);

        let collocates_vec: Vec<CollocateTerm> = collocates
            .iter()
            .map(|c| CollocateTerm {
                term: term_display.get(&c.term_hash).cloned(),
                term_hash: c.term_hash,
                score: c.score,
                near_count: c.near_count,
                background_count: c.background_count,
            })
            .collect();

        Ok(CollocationSearchResponse {
            schema: "sinoragd-collocation-search-v1",
            phrase: req.phrase,
            window_chars: req.window_chars,
            gram_len: req.gram_len,
            total_passages: hits.len(),
            near_ngram_count: near_total,
            background_ngram_count: bg_total,
            collocates: collocates_vec,
            search_strategy: CollocationSearchStrategy {
                phrase: phrase_strategy,
                limit_total: req.limit_total,
                limit_collocates: req.limit_collocates,
            },
        })
    }

    /// Implement the outline-search tool
    pub async fn outline_search_impl(
        &self,
        req: crate::tools::requests::OutlineSearchRequest,
    ) -> Result<crate::tools::responses::OutlineSearchResponse> {
        use crate::research_tools::phrase::phrase_rows_with_explicit_doc_table;
        use crate::research_tools::scopes::{group_hits_by_outline_node, OutlineSearchLevel};
        use crate::tools::responses::{
            OutlineSearchGroup, OutlineSearchResponse, OutlineSearchStrategy,
        };

        let passages = self.passages().await?;
        let doc_table = self.doc_table().await;
        let catalog = self.catalog().await;
        if (doc_table.is_err() || catalog.is_err())
            && req.node_id.is_none()
            && req.work_id.is_none()
        {
            let search = self
                .search_impl(crate::tools::requests::SearchRequest {
                    phrase: req.phrase.clone(),
                    limit: req.limit_total,
                    mode: "hits".to_string(),
                    depth: "exact".to_string(),
                    group_by: req.group_by.clone(),
                    include_variants: false,
                    limit_per_group: req.limit_per_group,
                    brief: true,
                    canon: None,
                    source_work_id: None,
                    tradition: None,
                    period: None,
                    origin: None,
                    author: None,
                    title: None,
                    heading_path_prefix: None,
                })
                .await?;
            let mut counts = std::collections::BTreeMap::<String, u32>::new();
            for hit in &search.hits {
                let key = match req.group_by.as_str() {
                    "division" | "passage" => hit
                        .heading
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("(unknown)"),
                    _ => hit
                        .source_work_id
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("(unknown)"),
                };
                *counts.entry(key.to_string()).or_insert(0) += 1;
            }
            let mut sorted_groups: Vec<(String, u32)> = counts.into_iter().collect();
            sorted_groups.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let groups = sorted_groups
                .iter()
                .take(req.limit_per_group)
                .enumerate()
                .map(|(idx, (label, count))| OutlineSearchGroup {
                    node_id: idx as u32,
                    label: label.clone(),
                    heading_path: label.clone(),
                    node_kind: "MetadataFallback".to_string(),
                    hit_count: *count,
                })
                .collect();
            return Ok(OutlineSearchResponse {
                schema: "sinoragd-outline-search-v1",
                phrase: req.phrase,
                start_node_id: 0,
                start_label: "corpus".to_string(),
                group_by: req.group_by,
                total_hits: search.hits.len(),
                group_count: sorted_groups.len(),
                groups,
                search_strategy: OutlineSearchStrategy {
                    phrase: serde_json::json!({
                        "used_phrase_index": false,
                        "scope_scan": "metadata_fallback",
                        "search": search.search_strategy,
                    }),
                    limit_total: req.limit_total,
                    limit_per_group: req.limit_per_group,
                },
            });
        }

        let doc_table = doc_table?;
        let catalog = catalog?;
        let phrase_index_path = self.optional_phrase_path().await?;

        let target = match req.group_by.as_str() {
            "division" => OutlineSearchLevel::Division,
            "work" => OutlineSearchLevel::Work,
            "passage" => OutlineSearchLevel::PassageRange,
            other => {
                return Err(anyhow::anyhow!(
                    "unknown group_by `{other}`; expected division|work|passage"
                ))
            }
        };

        // Resolve starting node: explicit node_id or work_id → root_node.
        // If no scope is supplied, search corpus-wide and group all hits.
        let (start_node, start_label, doc_range) = if let Some(nid) = req.node_id {
            let node = catalog
                .get_node(nid)
                .ok_or_else(|| anyhow::anyhow!("unknown node_id: {nid}"))?;
            let range = match (node.first_doc_id, node.last_doc_id) {
                (Some(l), Some(h)) => Some((l, h)),
                _ => return Err(anyhow::anyhow!("node {nid} has no doc range")),
            };
            (nid, node.label.clone(), range)
        } else if let Some(wid) = &req.work_id {
            let work = catalog
                .get_work(wid)
                .ok_or_else(|| anyhow::anyhow!("unknown work_id: {wid}"))?;
            let root = catalog
                .get_node(work.root_node)
                .ok_or_else(|| anyhow::anyhow!("work root node missing"))?;
            let range = match (root.first_doc_id, root.last_doc_id) {
                (Some(l), Some(h)) => Some((l, h)),
                _ => return Err(anyhow::anyhow!("work {wid} has no doc range")),
            };
            (work.root_node, root.label.clone(), range)
        } else {
            (0, "corpus".to_string(), None)
        };

        let (hits, phrase_strategy) = phrase_rows_with_explicit_doc_table(
            &passages,
            &doc_table,
            phrase_index_path.as_deref(),
            &req.phrase,
            req.limit_total,
            doc_range,
            None,
            None,
        )
        .await?;

        let filtered_doc_ids: Vec<u32> = hits
            .iter()
            .filter_map(|row| {
                let pid = row.get("passage_id").and_then(|v| v.as_str())?;
                doc_table.doc_id(pid)
            })
            .collect();

        let total_hits = filtered_doc_ids.len();

        // Group by the target outline level
        let group_counts = group_hits_by_outline_node(&catalog, &filtered_doc_ids, target);

        let mut sorted_groups: Vec<(u32, u32)> = group_counts.into_iter().collect();
        sorted_groups.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

        let groups: Vec<OutlineSearchGroup> = sorted_groups
            .iter()
            .take(req.limit_per_group)
            .map(|(node_id, count)| {
                let node = catalog.get_node(*node_id);
                OutlineSearchGroup {
                    node_id: *node_id,
                    label: node.map(|n| n.label.clone()).unwrap_or_default(),
                    heading_path: node.map(|n| n.heading_path.clone()).unwrap_or_default(),
                    node_kind: node
                        .map(|n| format!("{:?}", &n.node_kind))
                        .unwrap_or_default(),
                    hit_count: *count,
                }
            })
            .collect();

        Ok(OutlineSearchResponse {
            schema: "sinoragd-outline-search-v1",
            phrase: req.phrase,
            start_node_id: start_node,
            start_label,
            group_by: req.group_by,
            total_hits,
            group_count: sorted_groups.len(),
            groups,
            search_strategy: OutlineSearchStrategy {
                phrase: phrase_strategy,
                limit_total: req.limit_total,
                limit_per_group: req.limit_per_group,
            },
        })
    }

    /// Implement the cluster-hits tool
    pub async fn cluster_hits_impl(
        &self,
        req: crate::tools::requests::ClusterHitsRequest,
    ) -> Result<crate::tools::responses::ClusterHitsResponse> {
        use crate::research_tools::phrase::phrase_rows_with_explicit_doc_table;
        use crate::research_tools::scopes::{group_hits_by_outline_node, OutlineSearchLevel};
        use crate::tools::responses::{
            ClusterHitsCluster, ClusterHitsResponse, ClusterHitsSearchStrategy,
        };

        let doc_table = self.doc_table().await?;
        let catalog = self.catalog().await?;
        let passages = self.passages().await?;
        let phrase_index_path = self.optional_phrase_path().await?;

        let target = match req.cluster_by.as_str() {
            "work" => OutlineSearchLevel::Work,
            "division" => OutlineSearchLevel::Division,
            other => {
                return Err(anyhow::anyhow!(
                    "unknown cluster_by `{other}`; expected work|division"
                ))
            }
        };

        let (hits, phrase_strategy) = phrase_rows_with_explicit_doc_table(
            &passages,
            &doc_table,
            phrase_index_path.as_deref(),
            &req.phrase,
            req.limit_total,
            None,
            None,
            None,
        )
        .await?;

        // Collect (doc_id, row) pairs
        let mut doc_rows: Vec<(u32, serde_json::Value)> = Vec::with_capacity(hits.len());
        for row in hits {
            let pid = row.get("passage_id").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(did) = doc_table.doc_id(pid) {
                doc_rows.push((did, row));
            }
        }

        let doc_ids: Vec<u32> = doc_rows.iter().map(|(d, _)| *d).collect();
        let group_counts = group_hits_by_outline_node(&catalog, &doc_ids, target);

        // Sort groups by hit_count descending
        let mut sorted_groups: Vec<(u32, u32)> = group_counts.into_iter().collect();
        sorted_groups.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

        let clusters: Vec<ClusterHitsCluster> = sorted_groups
            .iter()
            .take(req.limit_per_cluster)
            .map(|(node_id, count)| {
                let node = catalog.get_node(*node_id);
                let node_doc_range = node.and_then(|n| n.first_doc_id.zip(n.last_doc_id));

                // Pick top representative passages within this cluster
                let mut reps: Vec<serde_json::Value> = doc_rows
                    .iter()
                    .filter(|(did, _)| {
                        if let Some((lo, hi)) = node_doc_range {
                            *did >= lo && *did <= hi
                        } else {
                            false
                        }
                    })
                    .take(3)
                    .map(|(did, row)| {
                        let mut r = row.clone();
                        if let Some(obj) = r.as_object_mut() {
                            obj.insert("doc_id".to_string(), serde_json::json!(*did));
                        }
                        r
                    })
                    .collect();
                reps.truncate(3);

                ClusterHitsCluster {
                    node_id: *node_id,
                    label: node.map(|n| n.label.clone()).unwrap_or_default(),
                    heading_path: node.map(|n| n.heading_path.clone()).unwrap_or_default(),
                    node_kind: node
                        .map(|n| format!("{:?}", &n.node_kind))
                        .unwrap_or_default(),
                    hit_count: *count,
                    representative_passages: reps,
                }
            })
            .collect();

        Ok(ClusterHitsResponse {
            schema: "sinoragd-cluster-hits-v1",
            phrase: req.phrase,
            cluster_by: req.cluster_by,
            total_hits: doc_rows.len(),
            cluster_count: sorted_groups.len(),
            clusters,
            search_strategy: ClusterHitsSearchStrategy {
                phrase: phrase_strategy,
                limit_total: req.limit_total,
                limit_per_cluster: req.limit_per_cluster,
            },
        })
    }

    /// Implement the absence-check tool
    pub async fn absence_check_impl(
        &self,
        req: crate::tools::requests::AbsenceCheckRequest,
    ) -> Result<crate::tools::responses::AbsenceCheckResponse> {
        use crate::research_tools::phrase::phrase_rows_with_explicit_doc_table;
        use crate::tools::responses::{
            AbsenceCheckResponse, AbsenceCheckScope, AbsenceCheckSearchStrategy,
        };

        let doc_table = self.doc_table().await?;
        let catalog = self.catalog().await?;
        let passages = self.passages().await?;
        let phrase_index_path = self.optional_phrase_path().await?;

        // Determine the doc range for the scope
        let doc_range: Option<(u32, u32)> = if let Some(nid) = req.scope_node_id {
            let node = catalog
                .get_node(nid)
                .ok_or_else(|| anyhow::anyhow!("unknown node_id: {nid}"))?;
            node.first_doc_id.zip(node.last_doc_id)
        } else if let Some(wid) = &req.scope_work_id {
            let work = catalog
                .get_work(wid)
                .ok_or_else(|| anyhow::anyhow!("unknown work_id: {wid}"))?;
            let root = catalog
                .get_node(work.root_node)
                .ok_or_else(|| anyhow::anyhow!("work root node missing"))?;
            root.first_doc_id.zip(root.last_doc_id)
        } else {
            None
        };

        let (scoped_hits, phrase_strategy) = phrase_rows_with_explicit_doc_table(
            &passages,
            &doc_table,
            phrase_index_path.as_deref(),
            &req.phrase,
            req.limit,
            doc_range,
            req.scope_canon.as_deref(),
            req.scope_period.as_deref(),
        )
        .await?;

        let found = !scoped_hits.is_empty();
        let hit_count = scoped_hits.len();

        Ok(AbsenceCheckResponse {
            schema: "sinoragd-absence-check-v1",
            phrase: req.phrase,
            scope: AbsenceCheckScope {
                work_id: req.scope_work_id,
                canon: req.scope_canon,
                period: req.scope_period,
                node_id: req.scope_node_id,
                doc_range: doc_range.map(|(l, h)| vec![l, h]),
            },
            found,
            hit_count,
            sample_hits: scoped_hits.into_iter().take(5).collect(),
            search_strategy: AbsenceCheckSearchStrategy {
                phrase: phrase_strategy,
                limit: req.limit,
            },
        })
    }

    /// Helper: resolve doc range from catalog
    fn resolve_doc_range(
        &self,
        catalog: &CorpusCatalogIndex,
        node_id: Option<u32>,
        work_id: Option<&str>,
    ) -> Result<Option<(u32, u32)>> {
        if let Some(nid) = node_id {
            let node = catalog
                .get_node(nid)
                .ok_or_else(|| anyhow::anyhow!("unknown node_id: {nid}"))?;
            return Ok(node.first_doc_id.zip(node.last_doc_id));
        }
        if let Some(wid) = work_id {
            let work = catalog
                .get_work(wid)
                .ok_or_else(|| anyhow::anyhow!("unknown work_id: {wid}"))?;
            let root = catalog
                .get_node(work.root_node)
                .ok_or_else(|| anyhow::anyhow!("work root node missing"))?;
            return Ok(root.first_doc_id.zip(root.last_doc_id));
        }
        Ok(None)
    }

    /// Helper: collect scope terms
    async fn collect_scope_terms(
        &self,
        passages: &DataFusionStore,
        doc_table: &DocumentTable,
        range: Option<(u32, u32)>,
        canon: Option<&str>,
        period: Option<&str>,
        limit_passages: usize,
        gram_len: usize,
    ) -> Result<(FxHashMap<u64, u32>, FxHashMap<u64, String>, usize)> {
        let rows = if let Some((lo, hi)) = range {
            let passage_ids: Vec<String> = (lo..=hi)
                .filter_map(|did| doc_table.passage_id(did).map(String::from))
                .take(limit_passages.max(1))
                .collect();
            passages
                .passages_by_ids(
                    &passage_ids,
                    "passage_id, zh_text_normalized, canon, period",
                )
                .await?
        } else {
            let mut where_parts = vec!["zh_text_normalized IS NOT NULL".to_string()];
            if let Some(canon) = canon {
                where_parts.push(format!(
                    "canon = {}",
                    crate::datafusion_store::sql_literal(canon)
                ));
            }
            if let Some(period) = period {
                where_parts.push(format!(
                    "period = {}",
                    crate::datafusion_store::sql_literal(period)
                ));
            }
            passages.query_json(&format!(
                "SELECT passage_id, zh_text_normalized, canon, period FROM passages WHERE {} LIMIT {}",
                where_parts.join(" AND "),
                limit_passages.max(1),
            )).await?
        };

        let mut terms: FxHashMap<u64, u32> = FxHashMap::default();
        let mut term_display: FxHashMap<u64, String> = FxHashMap::default();
        let mut passage_count = 0usize;
        for row in &rows {
            if let Some(canon_filter) = canon {
                let c = row.get("canon").and_then(|v| v.as_str()).unwrap_or("");
                if c != canon_filter {
                    continue;
                }
            }
            if let Some(period_filter) = period {
                let p = row.get("period").and_then(|v| v.as_str()).unwrap_or("");
                if p != period_filter {
                    continue;
                }
            }
            let text = row
                .get("zh_text_normalized")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if text.is_empty() {
                continue;
            }
            passage_count += 1;
            count_unique_ngrams_with_terms(text, gram_len, &mut terms, &mut term_display);
        }
        Ok((terms, term_display, passage_count))
    }
}
