//! Pack: the on-disk container that unifies corpus + indexes.
//!
//! Layout (paths inside the pack root, all relative in `manifest.json`):
//! ```text
//!   manifest.json
//!   data/passages.parquet/...
//!   derived/doc_table.bin
//!   derived/catalog.index
//!   derived/phrase_v2.index
//!   derived/tfidf.index
//!   derived/registry.sqlite
//! ```
//!
//! `doc_table.bin` owns the canonical `doc_id ↔ passage_id` mapping; every
//! index header carries the doc_table's fingerprint so loaders can refuse
//! mismatched combinations. The pack's own `fingerprints.doc_table` is the
//! authority — index `params_hash` values are convenience.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const PACK_SCHEMA: &str = "sinoragd-pack-v1";

pub const DEFAULT_PASSAGES: &str  = "data/passages.parquet";
pub const DEFAULT_DOC_TABLE: &str = "derived/doc_table.bin";
pub const DEFAULT_CATALOG: &str   = "derived/catalog.index";
pub const DEFAULT_REGISTRY: &str  = "derived/registry.sqlite";
pub const DEFAULT_PHRASE: &str    = "derived/phrase_v2.index";
pub const DEFAULT_TFIDF: &str     = "derived/tfidf.index";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackManifest {
    pub schema: String,
    pub pack_id: String,
    pub created_at: String,

    pub layout: Layout,
    pub fingerprints: Fingerprints,
    #[serde(default)]
    pub indexes: IndexSet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layout {
    pub passages: PathBuf,
    pub doc_table: PathBuf,
    pub catalog: PathBuf,
    pub registry: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Fingerprints {
    /// SHA-256 of sorted passage_ids (the value `DocumentTable::source_fingerprint`).
    pub doc_table: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexSet {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phrase: Option<IndexRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tfidf: Option<IndexRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexRef {
    pub path: PathBuf,
    pub doc_table_fingerprint: String,
    pub file_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params_hash: Option<String>,
}

pub struct Pack {
    pub root: PathBuf,
    pub manifest: PackManifest,
}

impl Pack {
    /// Open an existing pack by reading `manifest.json`.
    pub fn open(root: &Path) -> Result<Self> {
        let manifest_path = root.join(MANIFEST_FILE);
        let bytes = fs::read(&manifest_path)
            .with_context(|| format!("read {}", manifest_path.display()))?;
        let manifest: PackManifest = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse {}", manifest_path.display()))?;
        if manifest.schema != PACK_SCHEMA {
            return Err(anyhow!(
                "unknown pack schema `{}` (expected `{}`)",
                manifest.schema, PACK_SCHEMA
            ));
        }
        Ok(Pack { root: root.to_path_buf(), manifest })
    }

    /// Build a fresh manifest with the default layout, no fingerprints yet.
    pub fn default_layout(pack_id: impl Into<String>) -> PackManifest {
        PackManifest {
            schema: PACK_SCHEMA.to_string(),
            pack_id: pack_id.into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            layout: Layout {
                passages:  PathBuf::from(DEFAULT_PASSAGES),
                doc_table: PathBuf::from(DEFAULT_DOC_TABLE),
                catalog:   PathBuf::from(DEFAULT_CATALOG),
                registry:  PathBuf::from(DEFAULT_REGISTRY),
            },
            fingerprints: Fingerprints::default(),
            indexes: IndexSet::default(),
        }
    }

    pub fn write(root: &Path, manifest: &PackManifest) -> Result<()> {
        fs::create_dir_all(root)?;
        let path = root.join(MANIFEST_FILE);
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(manifest)?)?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn resolve(&self, rel: &Path) -> PathBuf {
        if rel.is_absolute() { rel.to_path_buf() } else { self.root.join(rel) }
    }

    pub fn passages_path(&self) -> PathBuf { self.resolve(&self.manifest.layout.passages) }
    pub fn doc_table_path(&self) -> PathBuf { self.resolve(&self.manifest.layout.doc_table) }
    pub fn catalog_path(&self)   -> PathBuf { self.resolve(&self.manifest.layout.catalog) }
    pub fn registry_path(&self)  -> PathBuf { self.resolve(&self.manifest.layout.registry) }
    pub fn phrase_path(&self) -> Option<PathBuf> {
        self.manifest.indexes.phrase.as_ref().map(|i| self.resolve(&i.path))
    }
    pub fn tfidf_path(&self) -> Option<PathBuf> {
        self.manifest.indexes.tfidf.as_ref().map(|i| self.resolve(&i.path))
    }
}
