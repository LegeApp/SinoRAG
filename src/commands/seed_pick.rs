use crate::datafusion_store::{sql_literal, string_contains_sql, DataFusionStore};
use anyhow::Result;
use rusqlite::Connection;
use serde_json::json;
use std::collections::HashSet;
use std::path::PathBuf;

pub async fn run(
    parquet_path: PathBuf,
    registry_path: PathBuf,
    tradition: Vec<String>,
    period: Vec<String>,
    limit: usize,
) -> Result<()> {
    let mut already_worked: HashSet<String> = HashSet::new();
    if registry_path.exists() {
        if let Ok(con) = Connection::open(&registry_path) {
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

    let mut where_clauses = vec!["true".to_string()];
    for t in &tradition {
        // traditions is JSON-encoded string; use substring match
        where_clauses.push(string_contains_sql("traditions", t));
    }
    for p in &period {
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
        limit.max(1)
    );

    let store = DataFusionStore::open(&parquet_path).await?;
    let results = store.query_json(&sql).await?;

    let payload = json!({
        "limit": limit,
        "already_worked_count": already_worked.len(),
        "filters": {
            "tradition": tradition,
            "period": period,
        },
        "candidates": results,
    });

    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}
