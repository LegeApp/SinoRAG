//! `compare-usage`: compare two sub-corpora (defined by catalog scopes)
//! and return distinctive terms using log-odds ratio scoring.

use crate::catalog_index::CorpusCatalogIndex;
use crate::datafusion_store::DataFusionStore;
use crate::document_table::DocumentTable;
use crate::jsonout::write_or_print;
use crate::research_tools::stats::log_odds_distinctive_terms;
use crate::text_analyzer::{analyze, AnalyzeOptions, AnalyzeScratch, FilterMode};
use anyhow::{anyhow, Result};
use rustc_hash::FxHashMap;
use serde_json::{json, Value};
use std::path::PathBuf;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    parquet: PathBuf,
    doc_table_path: PathBuf,
    catalog_path: PathBuf,
    scope_a_node_id: Option<u32>,
    scope_a_work_id: Option<String>,
    scope_a_canon: Option<String>,
    scope_a_period: Option<String>,
    scope_b_node_id: Option<u32>,
    scope_b_work_id: Option<String>,
    scope_b_canon: Option<String>,
    scope_b_period: Option<String>,
    gram_len: usize,
    limit_passages: usize,
    limit_terms: usize,
    out: Option<PathBuf>,
) -> Result<()> {
    let doc_table = DocumentTable::load(&doc_table_path)?;
    let catalog = CorpusCatalogIndex::load(&catalog_path)?;
    let store = DataFusionStore::open(&parquet).await?;

    // Resolve scope A doc range.
    let range_a = resolve_doc_range(&catalog, scope_a_node_id, scope_a_work_id.as_deref())?;
    let range_b = resolve_doc_range(&catalog, scope_b_node_id, scope_b_work_id.as_deref())?;

    let analyze_opts = AnalyzeOptions {
        min_n: gram_len, max_n: gram_len,
        filter: FilterMode::WhitespaceOnly,
        apply_low_value_filter: false,
        dedup: true,
        count_tf: false,
    };
    let mut scratch = AnalyzeScratch::new();

    let (a_terms, a_passage_count) = collect_scope_terms(
        &store,
        &doc_table,
        range_a,
        scope_a_canon.as_deref(),
        scope_a_period.as_deref(),
        limit_passages,
        &analyze_opts,
        &mut scratch,
    ).await?;

    let (b_terms, b_passage_count) = collect_scope_terms(
        &store,
        &doc_table,
        range_b,
        scope_b_canon.as_deref(),
        scope_b_period.as_deref(),
        limit_passages,
        &analyze_opts,
        &mut scratch,
    ).await?;

    let (a_top, b_top) = log_odds_distinctive_terms(&a_terms, &b_terms, limit_terms);

    let distinctive_to_a: Vec<Value> = a_top.iter().map(|t| json!({
        "term_hash": t.term_hash,
        "score": t.score,
        "a_count": t.a_count,
        "b_count": t.b_count,
    })).collect();

    let distinctive_to_b: Vec<Value> = b_top.iter().map(|t| json!({
        "term_hash": t.term_hash,
        "score": t.score,
        "a_count": t.a_count,
        "b_count": t.b_count,
    })).collect();

    let payload = json!({
        "schema": "sinoragd-compare-usage-v1",
        "scope_a": {
            "node_id": scope_a_node_id,
            "work_id": scope_a_work_id,
            "canon": scope_a_canon,
            "period": scope_a_period,
            "passage_count": a_passage_count,
        },
        "scope_b": {
            "node_id": scope_b_node_id,
            "work_id": scope_b_work_id,
            "canon": scope_b_canon,
            "period": scope_b_period,
            "passage_count": b_passage_count,
        },
        "distinctive_to_a": distinctive_to_a,
        "distinctive_to_b": distinctive_to_b,
        "search_strategy": {
            "gram_len": gram_len,
            "limit_passages": limit_passages,
            "limit_terms": limit_terms,
        }
    });
    write_or_print(&payload, out)
}

fn resolve_doc_range(
    catalog: &CorpusCatalogIndex,
    node_id: Option<u32>,
    work_id: Option<&str>,
) -> Result<Option<(u32, u32)>> {
    if let Some(nid) = node_id {
        let node = catalog.get_node(nid)
            .ok_or_else(|| anyhow!("unknown node_id: {nid}"))?;
        return Ok(node.first_doc_id.zip(node.last_doc_id));
    }
    if let Some(wid) = work_id {
        let work = catalog.get_work(wid)
            .ok_or_else(|| anyhow!("unknown work_id: {wid}"))?;
        let root = catalog.get_node(work.root_node)
            .ok_or_else(|| anyhow!("work root node missing"))?;
        return Ok(root.first_doc_id.zip(root.last_doc_id));
    }
    Ok(None)
}

async fn collect_scope_terms(
    store: &DataFusionStore,
    doc_table: &DocumentTable,
    range: Option<(u32, u32)>,
    canon: Option<&str>,
    period: Option<&str>,
    limit_passages: usize,
    analyze_opts: &AnalyzeOptions,
    scratch: &mut AnalyzeScratch,
) -> Result<(FxHashMap<u64, u32>, usize)> {
    let rows = if let Some((lo, hi)) = range {
        let passage_ids: Vec<String> = (lo..=hi)
            .filter_map(|did| doc_table.passage_id(did).map(String::from))
            .take(limit_passages.max(1))
            .collect();
        store.passages_by_ids(
            &passage_ids,
            "passage_id, zh_text_normalized, canon, period",
        ).await?
    } else {
        let mut where_parts = vec!["zh_text_normalized IS NOT NULL".to_string()];
        if let Some(canon) = canon {
            where_parts.push(format!("canon = {}", crate::datafusion_store::sql_literal(canon)));
        }
        if let Some(period) = period {
            where_parts.push(format!("period = {}", crate::datafusion_store::sql_literal(period)));
        }
        store.query_json(&format!(
            "SELECT passage_id, zh_text_normalized, canon, period FROM passages WHERE {} LIMIT {}",
            where_parts.join(" AND "),
            limit_passages.max(1),
        )).await?
    };

    let mut terms: FxHashMap<u64, u32> = FxHashMap::default();
    let mut passage_count = 0usize;
    for row in &rows {
        if let Some(canon) = canon {
            let c = row.get("canon").and_then(|v| v.as_str()).unwrap_or("");
            if c != canon { continue; }
        }
        if let Some(period) = period {
            let p = row.get("period").and_then(|v| v.as_str()).unwrap_or("");
            if p != period { continue; }
        }
        let text = row.get("zh_text_normalized").and_then(|v| v.as_str()).unwrap_or("");
        if text.is_empty() {
            continue;
        }
        passage_count += 1;
        analyze(text, analyze_opts, scratch);
        for &h in &scratch.unique {
            *terms.entry(h).or_insert(0) += 1;
        }
    }
    Ok((terms, passage_count))
}
