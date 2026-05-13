//! `trace-term-usage`: hit-counts and representative passages for a phrase,
//! grouped by period / canon / author / work.

use crate::datafusion_store::DataFusionStore;
use crate::document_table::DocumentTable;
use crate::jsonout::write_or_print;
use crate::research::{exact_phrase_rows_with_index, SearchSpec};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
pub enum GroupBy {
    Period,
    Canon,
    Author,
    Work,
}

impl GroupBy {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "period" => GroupBy::Period,
            "canon" => GroupBy::Canon,
            "author" => GroupBy::Author,
            "work" => GroupBy::Work,
            other => {
                return Err(anyhow!(
                    "unknown --group-by `{other}`; expected period|canon|author|work"
                ))
            }
        })
    }
    fn key_field(&self) -> &'static str {
        match self {
            GroupBy::Period => "period",
            GroupBy::Canon => "canon",
            GroupBy::Author => "author",
            GroupBy::Work => "source_work_id",
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    parquet: PathBuf,
    phrase_index: Option<PathBuf>,
    doc_table_path: PathBuf,
    phrase: String,
    group_by: String,
    limit_total: usize,
    limit_per_group: usize,
    out: Option<PathBuf>,
) -> Result<()> {
    let gb = GroupBy::parse(&group_by)?;
    let doc_table = DocumentTable::load(&doc_table_path)?;
    let store = DataFusionStore::open(&parquet).await?;

    let spec = SearchSpec::exact_phrase(phrase.clone(), limit_total);
    let hits = exact_phrase_rows_with_index(&store, &spec, phrase_index.as_deref()).await?;
    let total_hits = hits.len();

    // Group by the chosen field; within each group, keep top-K reps ordered
    // by (period_rank, doc_id). Also track work_count via a sub-set per group.
    let mut groups: BTreeMap<String, GroupAcc> = BTreeMap::new();
    for row in hits {
        let key = row
            .get(gb.key_field())
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

    let mut out_groups: Vec<Value> = Vec::with_capacity(groups.len());
    for (key, mut acc) in groups {
        acc.reps.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let reps: Vec<Value> = acc
            .reps
            .into_iter()
            .take(limit_per_group)
            .map(|(_, _, r)| r)
            .collect();
        let mut top_works: Vec<String> = acc.work_ids.into_iter().collect();
        top_works.sort();
        top_works.truncate(limit_per_group);
        out_groups.push(json!({
            "key": key,
            "hit_count": acc.hit_count,
            "work_count": top_works.len(),
            "top_works": top_works,
            "representative_passages": reps,
        }));
    }

    let payload = json!({
        "schema": "sinoragd-term-usage-trace-v1",
        "phrase": phrase,
        "group_by": group_by,
        "groups": out_groups,
        "search_strategy": {
            "used_phrase_index": phrase_index.is_some(),
            "total_hits": total_hits,
            "limit_total": limit_total,
            "limit_per_group": limit_per_group,
        }
    });
    write_or_print(&payload, out)
}

#[derive(Debug, Default)]
struct GroupAcc {
    hit_count: u32,
    work_ids: std::collections::BTreeSet<String>,
    reps: Vec<(i32, u32, Value)>,
}
