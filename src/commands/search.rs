use crate::datafusion_store::{sql_literal, string_contains_sql, DataFusionStore};
use crate::jsonout::write_or_print;
use crate::normalize::normalize_zh;
use crate::registry;
use crate::research::format_citation;
use crate::search_packet::{make_result_set_id, row_to_search_hit, SearchResultPacket};
use anyhow::Result;
use serde_json::json;
use std::path::PathBuf;

pub async fn search(
    store: &DataFusionStore,
    phrase: Option<String>,
    tradition: Vec<String>,
    period: Vec<String>,
    origin: Vec<String>,
    canon: Vec<String>,
    author: Option<String>,
    title: Option<String>,
    source_work_id: Option<String>,
    heading_path_prefix: Option<String>,
    limit: usize,
) -> Result<serde_json::Value> {
    let mut where_parts = Vec::new();
    let normalized_phrase = phrase.as_deref().map(normalize_zh).unwrap_or_default();
    if !normalized_phrase.is_empty() {
        where_parts.push(string_contains_sql(
            "zh_text_normalized",
            &normalized_phrase,
        ));
    }
    for t in expand_values(&tradition) {
        let t = crate::taxonomy_legend::resolve_tradition(&t);
        let token = serde_json::to_string(t).unwrap_or_else(|_| format!("\"{t}\""));
        where_parts.push(string_contains_sql("traditions", &token));
    }
    let resolved_periods: Vec<String> = expand_values(&period)
        .into_iter()
        .map(|p| crate::taxonomy_legend::resolve_period(&p).to_string())
        .collect();
    let resolved_origins: Vec<String> = expand_values(&origin)
        .into_iter()
        .map(|o| crate::taxonomy_legend::resolve_origin(&o).to_string())
        .collect();
    exact_any(&mut where_parts, "period", &resolved_periods);
    exact_any(&mut where_parts, "origin", &resolved_origins);
    exact_any(&mut where_parts, "canon", &expand_values(&canon));
    if let Some(author) = &author {
        where_parts.push(format!(
            "strpos(lower(author), lower({})) > 0",
            sql_literal(author)
        ));
    }
    if let Some(title) = &title {
        where_parts.push(format!(
            "strpos(lower(main_title), lower({})) > 0",
            sql_literal(title)
        ));
    }

    // Catalog index scope filters
    if let Some(work_id) = &source_work_id {
        where_parts.push(format!("source_work_id = {}", sql_literal(work_id)));
    }
    if let Some(prefix) = &heading_path_prefix {
        where_parts.push(format!(
            "heading_path LIKE {}",
            sql_literal(&format!("{}%", prefix))
        ));
    }

    let where_sql = if where_parts.is_empty() {
        "true".to_string()
    } else {
        where_parts.join(" AND ")
    };
    let limit = limit.max(1);
    let sql = format!(
        r#"
        SELECT passage_id, source_rel_path, xml_id, div_path, heading, heading_path,
               from_lb, to_lb, zh_text_raw, zh_text_normalized, text_type,
               contains_person, contains_term, contains_foreign,
               canon, canon_name, traditions, period, origin, author, main_title,
               period_rank, source_corpus, source_work_id, source_section_id, source_locator,
               source_url, edition_siglum, edition_label, rights_id, rights_notes,
               retrieval_method, snapshot_id, quality_flags_json
        FROM passages
        WHERE {where_sql}
        ORDER BY period_rank ASC, source_rel_path ASC, from_lb ASC, xml_id ASC
        LIMIT {limit}
        "#
    );
    let mut results = store.query_json(&sql).await?;

    // Add citation field to each result
    for result in &mut results {
        let from_lb = result
            .get("from_lb")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let to_lb = result
            .get("to_lb")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let citation = format_citation(result, &from_lb, &to_lb);
        if let Some(obj) = result.as_object_mut() {
            obj.insert("citation".to_string(), json!(citation));
        }
    }

    // Convert to SearchResultPacket format
    let result_set_id = make_result_set_id("search");
    let mut hits = Vec::with_capacity(results.len());
    for (idx, row) in results.into_iter().enumerate() {
        let rank = idx + 1;
        hits.push(row_to_search_hit(rank, row, None)?);
    }

    let packet = SearchResultPacket::new(
        result_set_id.clone(),
        None, // source_fingerprint - could be added later
        json!({
            "tool": "search",
            "phrase": phrase,
            "normalized_phrase": normalized_phrase,
            "filters": {
                "tradition": expand_values(&tradition),
                "period": expand_values(&period),
                "origin": expand_values(&origin),
                "canon": expand_values(&canon),
                "author": author,
                "title": title,
                "source_work_id": source_work_id,
                "heading_path_prefix": heading_path_prefix,
            },
            "limit": limit,
        }),
        hits,
    );

    let payload = serde_json::to_value(&packet)?;
    Ok(payload)
}

pub async fn run(
    parquet_path: PathBuf,
    phrase: Option<String>,
    tradition: Vec<String>,
    period: Vec<String>,
    origin: Vec<String>,
    canon: Vec<String>,
    author: Option<String>,
    title: Option<String>,
    source_work_id: Option<String>,
    heading_path_prefix: Option<String>,
    limit: usize,
    out: Option<PathBuf>,
    registry_path: PathBuf,
) -> Result<()> {
    let store = DataFusionStore::open(&parquet_path).await?;
    let payload = search(
        &store,
        phrase,
        tradition,
        period,
        origin,
        canon,
        author,
        title,
        source_work_id,
        heading_path_prefix,
        limit,
    )
    .await?;

    let _ = registry::record_payload(
        &registry_path,
        "search_result",
        &payload,
        out.as_deref(),
        "",
        payload.get("phrase").and_then(|v| v.as_str()).unwrap_or(""),
    );
    write_or_print(&payload, out)
}

pub fn expand_values(values: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        for part in value.split(',') {
            let part = part.trim();
            if !part.is_empty() && !out.iter().any(|x: &String| x == part) {
                out.push(part.to_string());
            }
        }
    }
    out
}

fn exact_any(where_parts: &mut Vec<String>, column: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    let quoted = values
        .iter()
        .map(|v| sql_literal(v))
        .collect::<Vec<_>>()
        .join(", ");
    where_parts.push(format!("{column} IN ({quoted})"));
}
