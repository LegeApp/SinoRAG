use anyhow::Result;
use rustc_hash::FxHashMap;
use std::path::PathBuf;

pub async fn run(parquet_path: PathBuf) -> Result<()> {
    let store = crate::datafusion_store::DataFusionStore::open(&parquet_path).await?;

    let canons = store
        .query_json("SELECT canon, COUNT(*) as cnt FROM passages GROUP BY canon ORDER BY cnt DESC")
        .await?;

    let periods = store
        .query_json("SELECT period, COUNT(*) as cnt FROM passages GROUP BY period ORDER BY period ASC")
        .await?;

    let origins = store
        .query_json("SELECT origin, COUNT(*) as cnt FROM passages GROUP BY origin ORDER BY cnt DESC")
        .await?;

    let trad_rows = store
        .query_json("SELECT DISTINCT traditions FROM passages WHERE traditions IS NOT NULL AND traditions != '[]' AND traditions != ''")
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
    let mut traditions: Vec<serde_json::Value> = trad_counts
        .into_iter()
        .map(|(name, work_count)| serde_json::json!({ "tradition": name, "work_count": work_count }))
        .collect();
    traditions.sort_by(|a, b| {
        let ca = a["work_count"].as_u64().unwrap_or(0);
        let cb = b["work_count"].as_u64().unwrap_or(0);
        cb.cmp(&ca)
    });

    let out = serde_json::json!({
        "schema": "sinoragd-taxonomy-v1",
        "note": "Use these values with --canon / --tradition / --period / --origin filters on search and works commands.",
        "canon": canons.iter().map(|r| serde_json::json!({
            "code": r.get("canon").and_then(|v| v.as_str()).unwrap_or(""),
            "passage_count": r.get("cnt").and_then(|v| v.as_i64()).unwrap_or(0),
        })).collect::<Vec<_>>(),
        "period": periods.iter().map(|r| serde_json::json!({
            "name": r.get("period").and_then(|v| v.as_str()).unwrap_or(""),
            "passage_count": r.get("cnt").and_then(|v| v.as_i64()).unwrap_or(0),
        })).collect::<Vec<_>>(),
        "tradition": traditions,
        "origin": origins.iter().map(|r| serde_json::json!({
            "name": r.get("origin").and_then(|v| v.as_str()).unwrap_or(""),
            "passage_count": r.get("cnt").and_then(|v| v.as_i64()).unwrap_or(0),
        })).collect::<Vec<_>>(),
    });

    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
