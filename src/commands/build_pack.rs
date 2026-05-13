//! `build-pack`: stitch already-built artifacts into a pack.
//!
//! Detects what exists in the pack root, validates that every present index's
//! header carries the matching `doc_table_fingerprint`, populates the registry
//! identity tables, and writes `manifest.json`. Refuses to wire up an index
//! whose fingerprint disagrees with the DocumentTable on disk.

use crate::catalog_index::CorpusCatalogIndex;
use crate::document_table::{match_index_fingerprint, DocumentTable, IndexCoverage};
use crate::pack::{
    self, IndexRef, IndexSet, Pack, DEFAULT_CATALOG, DEFAULT_DOC_TABLE, DEFAULT_PHRASE,
    DEFAULT_TFIDF,
};
use crate::phrase_index::PhraseIndex;
use crate::tfidf::index::TfidfIndex;
use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub fn run(pack_root: PathBuf, pack_id: Option<String>) -> Result<()> {
    if !pack_root.exists() {
        anyhow::bail!("pack root does not exist: {}", pack_root.display());
    }
    eprintln!("=== build-pack ===");
    eprintln!("Pack root: {}", pack_root.display());

    let doc_table_path = pack_root.join(DEFAULT_DOC_TABLE);
    if !doc_table_path.exists() {
        anyhow::bail!(
            "doc_table.bin missing at {}. Run `doc-table-build` first.",
            doc_table_path.display()
        );
    }
    eprintln!("[1/5] Loading DocumentTable...");
    let doc_table = DocumentTable::load(&doc_table_path)
        .with_context(|| format!("load {}", doc_table_path.display()))?;
    let dt_fp = doc_table.source_fingerprint.clone();
    eprintln!(
        "      {} passages, fingerprint {}...{}",
        doc_table.passage_ids.len(),
        &dt_fp[..8.min(dt_fp.len())],
        &dt_fp[dt_fp.len().saturating_sub(8)..]
    );

    // --- catalog ----------------------------------------------------------
    let catalog_path = pack_root.join(DEFAULT_CATALOG);
    if !catalog_path.exists() {
        anyhow::bail!(
            "catalog.index missing at {}. Run `catalog-index-build --doc-table {}` first.",
            catalog_path.display(),
            doc_table_path.display()
        );
    }
    eprintln!("[2/5] Loading catalog...");
    let catalog = CorpusCatalogIndex::load(&catalog_path)
        .with_context(|| format!("load {}", catalog_path.display()))?;
    match &catalog.doc_table_fingerprint {
        Some(fp) if fp == &dt_fp => {
            eprintln!("      catalog fingerprint matches doc_table");
        }
        Some(fp) => {
            return Err(anyhow!(
                "catalog fingerprint {} != doc_table {}. Rebuild catalog.",
                short(fp),
                short(&dt_fp)
            ));
        }
        None => {
            eprintln!(
                "      WARNING: catalog has no doc_table_fingerprint (built before fingerprint propagation). \
                 Rebuild recommended; proceeding."
            );
        }
    }
    eprintln!(
        "      {} works, {} nodes",
        catalog.works.len(),
        catalog.nodes.len()
    );

    // --- phrase index (optional) -----------------------------------------
    let phrase_path = pack_root.join(DEFAULT_PHRASE);
    let phrase_present = phrase_path.exists();
    let mut phrase_info_holder: Option<serde_json::Value> = None;
    let mut phrase_coverage: Option<IndexCoverage> = None;
    if phrase_present {
        eprintln!("[3/5] Validating phrase index...");
        let info = PhraseIndex::header_info(&phrase_path)
            .with_context(|| format!("read header {}", phrase_path.display()))?;
        let fp = info
            .get("doc_table_fingerprint")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        match match_index_fingerprint(&doc_table, &doc_table_path, fp)? {
            Some(IndexCoverage::Full) => {
                eprintln!(
                    "      OK (full coverage): {} grams, {} postings bytes",
                    info.get("num_grams").and_then(|v| v.as_u64()).unwrap_or(0),
                    info.get("postings_bytes")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                );
                phrase_coverage = Some(IndexCoverage::Full);
            }
            Some(IndexCoverage::Base { base_doc_count }) => {
                eprintln!(
                    "      OK (lineage match): covers doc_ids 0..{} of {}; rebuild to extend",
                    base_doc_count,
                    doc_table.passage_ids.len()
                );
                phrase_coverage = Some(IndexCoverage::Base { base_doc_count });
            }
            None => {
                return Err(anyhow!(
                    "phrase index fingerprint {} matches neither current doc_table {} nor lineage base. Rebuild phrase index.",
                    short(fp), short(&dt_fp)
                ));
            }
        }
        phrase_info_holder = Some(info);
    } else {
        eprintln!("[3/5] phrase index not present (skipping)");
    }

    // --- tfidf index (optional) ------------------------------------------
    let tfidf_path = pack_root.join(DEFAULT_TFIDF);
    let tfidf_present = tfidf_path.exists();
    let mut tfidf_info_holder: Option<serde_json::Value> = None;
    let mut tfidf_coverage: Option<IndexCoverage> = None;
    if tfidf_present {
        eprintln!("[4/5] Validating TF-IDF index...");
        let info = TfidfIndex::header_info(&tfidf_path)
            .with_context(|| format!("read header {}", tfidf_path.display()))?;
        let fp = info
            .get("doc_table_fingerprint")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        match match_index_fingerprint(&doc_table, &doc_table_path, fp)? {
            Some(IndexCoverage::Full) => {
                eprintln!(
                    "      OK (full coverage): {} docs, {} features",
                    info.get("documents").and_then(|v| v.as_u64()).unwrap_or(0),
                    info.get("features").and_then(|v| v.as_u64()).unwrap_or(0)
                );
                tfidf_coverage = Some(IndexCoverage::Full);
            }
            Some(IndexCoverage::Base { base_doc_count }) => {
                eprintln!(
                    "      OK (lineage match): covers doc_ids 0..{} of {}; rebuild to extend",
                    base_doc_count,
                    doc_table.passage_ids.len()
                );
                tfidf_coverage = Some(IndexCoverage::Base { base_doc_count });
            }
            None => {
                return Err(anyhow!(
                    "TF-IDF index fingerprint {} matches neither current doc_table {} nor lineage base. Rebuild TF-IDF.",
                    short(fp), short(&dt_fp)
                ));
            }
        }
        tfidf_info_holder = Some(info);
    } else {
        eprintln!("[4/5] TF-IDF index not present (skipping)");
    }
    // Coverage is recorded by the registry populator + manifest below.
    let _ = (&phrase_coverage, &tfidf_coverage);

    // --- registry + manifest ---------------------------------------------
    eprintln!("[5/5] Populating registry + writing manifest...");
    let registry_path = pack_root.join(pack::DEFAULT_REGISTRY);
    if let Some(parent) = registry_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let phrase_for_db = phrase_info_holder
        .as_ref()
        .map(|i| (phrase_path.as_path(), i));
    let tfidf_for_db = tfidf_info_holder
        .as_ref()
        .map(|i| (tfidf_path.as_path(), i));

    crate::registry::populate_identity_from_pack(
        &registry_path,
        &doc_table,
        &catalog,
        phrase_for_db,
        tfidf_for_db,
        &pack_root,
    )?;

    let mut manifest = Pack::default_layout(pack_id.unwrap_or_else(|| default_pack_id(&pack_root)));
    manifest.fingerprints.doc_table = dt_fp.clone();
    manifest.indexes = IndexSet {
        phrase: phrase_present.then(|| IndexRef {
            path: PathBuf::from(DEFAULT_PHRASE),
            doc_table_fingerprint: dt_fp.clone(),
            file_bytes: fs::metadata(&phrase_path).map(|m| m.len()).unwrap_or(0),
            params_hash: None,
        }),
        tfidf: tfidf_present.then(|| IndexRef {
            path: PathBuf::from(DEFAULT_TFIDF),
            doc_table_fingerprint: dt_fp.clone(),
            file_bytes: fs::metadata(&tfidf_path).map(|m| m.len()).unwrap_or(0),
            params_hash: None,
        }),
    };

    Pack::write(&pack_root, &manifest)?;
    eprintln!("\nwrote {}", pack_root.join(pack::MANIFEST_FILE).display());

    Ok(())
}

fn short(s: &str) -> String {
    if s.len() <= 16 {
        return s.to_string();
    }
    format!("{}…{}", &s[..8], &s[s.len() - 8..])
}

fn default_pack_id(root: &Path) -> String {
    root.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("pack")
        .to_string()
}
