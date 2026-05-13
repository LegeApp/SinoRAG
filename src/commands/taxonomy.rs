use crate::taxonomy_legend as leg;
use anyhow::Result;
use rustc_hash::FxHashMap;
use std::path::PathBuf;

pub async fn run(parquet_path: PathBuf) -> Result<()> {
    let store = crate::datafusion_store::DataFusionStore::open(&parquet_path).await?;

    let canons = store
        .query_json("SELECT canon, canon_name, COUNT(*) as cnt FROM passages GROUP BY canon, canon_name ORDER BY cnt DESC")
        .await?;

    let periods = store
        .query_json(
            "SELECT period, COUNT(*) as cnt FROM passages GROUP BY period ORDER BY cnt DESC",
        )
        .await?;

    let origins = store
        .query_json(
            "SELECT origin, COUNT(*) as cnt FROM passages GROUP BY origin ORDER BY cnt DESC",
        )
        .await?;

    // traditions is a JSON array string per row; accumulate passage-level counts
    let trad_rows = store
        .query_json("SELECT traditions FROM passages WHERE traditions IS NOT NULL AND traditions != '[]' AND traditions != ''")
        .await?;
    let mut trad_counts: FxHashMap<String, usize> = FxHashMap::default();
    for row in &trad_rows {
        if let Some(s) = row.get("traditions").and_then(|v| v.as_str()) {
            if let Ok(arr) = serde_json::from_str::<Vec<String>>(s) {
                for t in arr {
                    *trad_counts.entry(t).or_insert(0) += 1;
                }
            }
        }
    }
    let mut trad_corpus: Vec<serde_json::Value> = trad_counts
        .into_iter()
        .map(|(name, passage_count)| {
            let id = leg::TRADITIONS
                .iter()
                .find(|e| e.name == name)
                .map(|e| e.id as i64);
            serde_json::json!({ "id": id, "name": name, "passage_count": passage_count })
        })
        .collect();
    trad_corpus.sort_by(|a, b| {
        b["passage_count"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["passage_count"].as_u64().unwrap_or(0))
    });

    let period_corpus: Vec<serde_json::Value> = periods
        .iter()
        .map(|r| {
            let name = r.get("period").and_then(|v| v.as_str()).unwrap_or("");
            let id = leg::PERIODS
                .iter()
                .find(|e| e.name == name)
                .map(|e| e.id as i64);
            serde_json::json!({
                "id": id,
                "name": name,
                "passage_count": r.get("cnt").and_then(|v| v.as_i64()).unwrap_or(0),
            })
        })
        .collect();

    let origin_corpus: Vec<serde_json::Value> = origins
        .iter()
        .map(|r| {
            let name = r.get("origin").and_then(|v| v.as_str()).unwrap_or("");
            let id = leg::ORIGINS
                .iter()
                .find(|e| e.name == name)
                .map(|e| e.id as i64);
            serde_json::json!({
                "id": id,
                "name": name,
                "passage_count": r.get("cnt").and_then(|v| v.as_i64()).unwrap_or(0),
            })
        })
        .collect();

    let canon_corpus: Vec<serde_json::Value> = canons
        .iter()
        .map(|r| {
            serde_json::json!({
                "code": r.get("canon").and_then(|v| v.as_str()).unwrap_or(""),
                "canon_name": r.get("canon_name").and_then(|v| v.as_str()).unwrap_or(""),
                "passage_count": r.get("cnt").and_then(|v| v.as_i64()).unwrap_or(0),
            })
        })
        .collect();

    let out = serde_json::json!({
        "schema": "sinoragd-taxonomy-v1",
        "note": "Pass id numbers OR exact name strings to --tradition / --period / --origin filters. Canon uses the 'code' string directly.",
        "legend": {
            "tradition": leg::traditions_json(),
            "period":    leg::periods_json(),
            "origin":    leg::origins_json(),
        },
        "corpus_counts": {
            "canon":     canon_corpus,
            "tradition": trad_corpus,
            "period":    period_corpus,
            "origin":    origin_corpus,
        },
    });

    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
