use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const CATALOG_BATCH_SIZE: usize = 500;
const BATCH_INSERT_WORK: usize = 200;
const BATCH_INSERT_OBS: usize = 200;

#[derive(Debug, Clone)]
pub struct WorkItem {
    pub item_id: String,
    pub artifact_type: String,
    pub path: String,
    pub content_hash: String,
    pub modified_utc: String,
    pub seed_passage_id: String,
    pub phrase: String,
    pub normalized_phrase: String,
    pub lens: String,
    pub status: String,
    pub change_count: i32,
    pub one_line_summary: String,
    pub summary_json: String,
}

#[derive(Debug, Clone)]
pub struct PhraseObservation {
    pub item_id: String,
    pub phrase: String,
    pub normalized_phrase: String,
    pub total_hits: i32,
    pub result_count: i32,
    pub graph_potential: String,
    pub risks: String,
    pub recommended_next_seed: String,
    pub summary_json: String,
}

#[derive(Debug, Clone)]
pub struct SeedObservation {
    pub item_id: String,
    pub seed_passage_id: String,
    pub lens: String,
    pub accepted_claim_count: i32,
    pub rejected_claim_count: i32,
    pub node_count: i32,
    pub edge_count: i32,
    pub frontier_count: i32,
    pub similar_count: i32,
    pub next_seed_count: i32,
    pub summary_json: String,
}

#[derive(Debug, Clone)]
pub struct ParsedArtifact {
    pub item: WorkItem,
    pub phrase_observations: Vec<PhraseObservation>,
    pub seed_observations: Vec<SeedObservation>,
}

pub fn init_registry(db_path: &Path) -> Result<()> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).context("Failed to create registry directory")?;
    }

    let con = Connection::open(db_path).context("Failed to open registry database")?;

    // Enable WAL mode for better concurrency (allows concurrent readers)
    con.execute("PRAGMA journal_mode=WAL", [])
        .context("Failed to enable WAL mode")?;
    
    // Set synchronous mode to NORMAL for better performance while maintaining durability
    con.execute("PRAGMA synchronous=NORMAL", [])
        .context("Failed to set synchronous mode")?;
    
    // Set busy timeout to 5 seconds to handle contention gracefully
    con.execute("PRAGMA busy_timeout=5000", [])
        .context("Failed to set busy timeout")?;

    con.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS work_items (
            item_id TEXT PRIMARY KEY,
            artifact_type TEXT,
            path TEXT,
            content_hash TEXT,
            modified_utc TEXT,
            seed_passage_id TEXT,
            phrase TEXT,
            normalized_phrase TEXT,
            lens TEXT,
            status TEXT,
            change_count INTEGER,
            one_line_summary TEXT,
            summary_json TEXT
        );

        CREATE TABLE IF NOT EXISTS work_edges (
            from_item_id TEXT,
            to_item_id TEXT,
            relation TEXT,
            PRIMARY KEY (from_item_id, to_item_id, relation)
        );

        CREATE TABLE IF NOT EXISTS phrase_observations (
            item_id TEXT,
            phrase TEXT,
            normalized_phrase TEXT,
            total_hits INTEGER,
            result_count INTEGER,
            graph_potential TEXT,
            risks TEXT,
            recommended_next_seed TEXT,
            summary_json TEXT
        );

        CREATE TABLE IF NOT EXISTS seed_observations (
            item_id TEXT,
            seed_passage_id TEXT,
            lens TEXT,
            accepted_claim_count INTEGER,
            rejected_claim_count INTEGER,
            node_count INTEGER,
            edge_count INTEGER,
            frontier_count INTEGER,
            similar_count INTEGER,
            next_seed_count INTEGER,
            summary_json TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_work_seed ON work_items(seed_passage_id);
        CREATE INDEX IF NOT EXISTS idx_work_phrase ON work_items(normalized_phrase);
        CREATE INDEX IF NOT EXISTS idx_phrase_norm ON phrase_observations(normalized_phrase);
        ",
    )
    .context("Failed to create registry tables")?;

    Ok(())
}

pub fn catalog_runs(runs_root: &Path, db_path: &Path) -> Result<Value> {
    init_registry(db_path)?;

    let supported_suffixes = [".json", ".md"];
    let mut artifacts: Vec<PathBuf> = Vec::new();

    for entry in WalkDir::new(runs_root).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_file()
            && path.file_name() != Some(std::ffi::OsStr::new(".gitkeep"))
            && path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    supported_suffixes
                        .iter()
                        .any(|s| s == &format!(".{}", e.to_lowercase()))
                })
                .unwrap_or(false)
        {
            artifacts.push(path.to_path_buf());
        }
    }

    artifacts.sort();

    let total = artifacts.len();
    let mut cataloged = 0;

    for chunk in artifacts.chunks(CATALOG_BATCH_SIZE) {
        let batch_parsed: Vec<Option<ParsedArtifact>> = chunk
            .iter()
            .map(|path| parse_artifact(path, runs_root))
            .collect();

        let parsed_items: Vec<ParsedArtifact> =
            batch_parsed.into_iter().filter_map(|item| item).collect();

        upsert_items_batch(db_path, &parsed_items)?;
        cataloged += parsed_items.len();
    }

    link_related_items(db_path)?;

    Ok(json!({
        "scanned": total,
        "cataloged": cataloged
    }))
}

pub fn record_payload(
    db_path: &Path,
    artifact_type: &str,
    payload: &Value,
    path: Option<&Path>,
    seed_passage_id: &str,
    phrase: &str,
) -> Result<String> {
    init_registry(db_path)?;

    let content = serde_json::to_vec(payload)?;
    let digest = sha256_hash(&content);
    let path_str = path
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let item_id = item_id(&path_str, &digest);

    let summary = summarize_payload(artifact_type, payload, path, seed_passage_id, phrase);

    let normalized_phrase = crate::normalize::normalize_zh(
        summary
            .get("phrase")
            .and_then(|v| v.as_str())
            .unwrap_or(phrase),
    );

    let item = WorkItem {
        item_id: item_id.clone(),
        artifact_type: artifact_type.to_string(),
        path: path_str,
        content_hash: digest,
        modified_utc: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        seed_passage_id: summary
            .get("seed_passage_id")
            .and_then(|v| v.as_str())
            .unwrap_or(seed_passage_id)
            .to_string(),
        phrase: summary
            .get("phrase")
            .and_then(|v| v.as_str())
            .unwrap_or(phrase)
            .to_string(),
        normalized_phrase,
        lens: summary
            .get("lens")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        status: summary
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("completed")
            .to_string(),
        change_count: summary
            .get("change_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32,
        one_line_summary: summary
            .get("one_line_summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        summary_json: serde_json::to_string(&summary)?,
    };

    let phrase_obs = phrase_observations(&item.item_id, artifact_type, payload);
    let seed_obs = seed_observations(&item.item_id, artifact_type, payload, &summary);

    let parsed = ParsedArtifact {
        item,
        phrase_observations: phrase_obs,
        seed_observations: seed_obs,
    };

    upsert_items_batch(db_path, &[parsed])?;
    link_related_items(db_path)?;

    Ok(item_id)
}

pub fn upsert_items(db_path: &Path, parsed_items: &[ParsedArtifact]) -> Result<()> {
    upsert_items_batch(db_path, parsed_items)
}

fn upsert_items_batch(db_path: &Path, parsed_items: &[ParsedArtifact]) -> Result<()> {
    let mut con = rusqlite::Connection::open(db_path).context("Failed to open registry database")?;
    
    // Configure connection for concurrency
    con.execute("PRAGMA busy_timeout=5000", [])
        .context("Failed to set busy timeout")?;
    
    let tx = con.transaction()?;

    {
        let mut work_stmt = tx.prepare_cached(
            "INSERT OR REPLACE INTO work_items VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )?;
        let mut phrase_del = tx.prepare_cached("DELETE FROM phrase_observations WHERE item_id = ?")?;
        let mut seed_del = tx.prepare_cached("DELETE FROM seed_observations WHERE item_id = ?")?;
        let mut phrase_stmt = tx.prepare_cached(
            "INSERT INTO phrase_observations VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )?;
        let mut seed_stmt = tx.prepare_cached(
            "INSERT INTO seed_observations VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )?;

        for parsed in parsed_items {
            let item = &parsed.item;
            work_stmt.execute(params![
                &item.item_id,
                &item.artifact_type,
                &item.path,
                &item.content_hash,
                &item.modified_utc,
                &item.seed_passage_id,
                &item.phrase,
                &item.normalized_phrase,
                &item.lens,
                &item.status,
                item.change_count,
                &item.one_line_summary,
                &item.summary_json,
            ])?;

            phrase_del.execute(params![&item.item_id])?;
            for obs in &parsed.phrase_observations {
                phrase_stmt.execute(params![
                    &obs.item_id,
                    &obs.phrase,
                    &obs.normalized_phrase,
                    obs.total_hits,
                    obs.result_count,
                    &obs.graph_potential,
                    &obs.risks,
                    &obs.recommended_next_seed,
                    &obs.summary_json,
                ])?;
            }

            seed_del.execute(params![&item.item_id])?;
            for obs in &parsed.seed_observations {
                seed_stmt.execute(params![
                    &obs.item_id,
                    &obs.seed_passage_id,
                    &obs.lens,
                    obs.accepted_claim_count,
                    obs.rejected_claim_count,
                    obs.node_count,
                    obs.edge_count,
                    obs.frontier_count,
                    obs.similar_count,
                    obs.next_seed_count,
                    &obs.summary_json,
                ])?;
            }
        }
    }

    tx.commit()?;
    let _ = BATCH_INSERT_WORK;
    let _ = BATCH_INSERT_OBS;
    Ok(())
}

pub fn prior_work(registry: &Path, seed_passage_id: &str, limit: usize) -> Result<Vec<Value>> {
    ensure_registry(registry)?;

    let con = Connection::open(registry).context("Failed to open registry database")?;
    con.execute("PRAGMA busy_timeout=5000", [])
        .context("Failed to set busy timeout")?;
    let limit = std::cmp::max(1, limit);

    let mut stmt = con.prepare(
        "
        SELECT artifact_type, path, seed_passage_id, lens, status, change_count,
               one_line_summary, summary_json
        FROM work_items
        WHERE seed_passage_id = ?
        ORDER BY modified_utc DESC, artifact_type
        LIMIT ?
        ",
    )?;

    let rows = stmt
        .query_map(params![seed_passage_id, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i32>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut results = Vec::new();
    for row in rows {
        let summary_json: Value = serde_json::from_str(&row.7).unwrap_or(Value::Null);
        results.push(json!({
            "artifact_type": row.0,
            "path": row.1,
            "seed_passage_id": row.2,
            "lens": row.3,
            "status": row.4,
            "change_count": row.5,
            "one_line_summary": row.6,
            "summary": summary_json
        }));
    }

    Ok(results)
}

pub fn phrase_status(registry: &Path, phrase: &str, limit: usize) -> Result<Value> {
    ensure_registry(registry)?;

    let normalized = crate::normalize::normalize_zh(phrase);
    let con = Connection::open(registry).context("Failed to open registry database")?;
    con.execute("PRAGMA busy_timeout=5000", [])
        .context("Failed to set busy timeout")?;
    let limit = std::cmp::max(1, limit);

    let mut stmt = con.prepare(
        "
        SELECT phrase, total_hits, result_count, graph_potential, risks,
               recommended_next_seed, summary_json
        FROM phrase_observations
        WHERE normalized_phrase = ?
        ORDER BY total_hits DESC, result_count DESC
        LIMIT ?
        ",
    )?;

    let rows = stmt
        .query_map(params![&normalized, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i32>(1)?,
                row.get::<_, i32>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut observations = Vec::new();
    for row in rows {
        let summary_json: Value = serde_json::from_str(&row.6).unwrap_or(Value::Null);
        observations.push(json!({
            "phrase": row.0,
            "total_hits": row.1,
            "result_count": row.2,
            "graph_potential": row.3,
            "risks": row.4,
            "recommended_next_seed": row.5,
            "summary": summary_json
        }));
    }

    Ok(json!({
        "phrase": phrase,
        "normalized_phrase": normalized,
        "observation_count": observations.len(),
        "observations": observations
    }))
}

pub fn work_summary(registry: &Path, limit: usize) -> Result<Vec<Value>> {
    ensure_registry(registry)?;

    let con = Connection::open(registry).context("Failed to open registry database")?;
    con.execute("PRAGMA busy_timeout=5000", [])
        .context("Failed to set busy timeout")?;
    let limit = std::cmp::max(1, limit);

    let mut stmt = con.prepare(
        "
        SELECT artifact_type, path, seed_passage_id, phrase, lens, status,
               change_count, one_line_summary
        FROM work_items
        ORDER BY modified_utc DESC, change_count DESC
        LIMIT ?
        ",
    )?;

    let rows = stmt
        .query_map(params![limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i32>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut results = Vec::new();
    for row in rows {
        results.push(json!({
            "artifact_type": row.0,
            "path": row.1,
            "seed_passage_id": row.2,
            "phrase": row.3,
            "lens": row.4,
            "status": row.5,
            "change_count": row.6,
            "one_line_summary": row.7
        }));
    }

    Ok(results)
}

fn parse_artifact(path: &Path, runs_root: &Path) -> Option<ParsedArtifact> {
    let rel_path = path
        .strip_prefix(runs_root)
        .ok()?
        .to_string_lossy()
        .to_string();
    let content = fs::read(path).ok()?;
    let digest = sha256_hash(&content);
    let item_id = item_id(&rel_path, &digest);
    let payload = load_payload(path, &content)?;
    let artifact_type = artifact_type(&rel_path, &payload);
    let summary = summarize_payload(&artifact_type, &payload, Some(path), "", "");
    let modified = fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| {
            DateTime::<Utc>::from(t)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                .into()
        })
        .unwrap_or_default();

    let normalized_phrase = crate::normalize::normalize_zh(
        summary.get("phrase").and_then(|v| v.as_str()).unwrap_or(""),
    );

    let item = WorkItem {
        item_id,
        artifact_type,
        path: path.to_string_lossy().to_string(),
        content_hash: digest,
        modified_utc: modified,
        seed_passage_id: summary
            .get("seed_passage_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        phrase: summary
            .get("phrase")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        normalized_phrase,
        lens: summary
            .get("lens")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        status: summary
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("completed")
            .to_string(),
        change_count: summary
            .get("change_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32,
        one_line_summary: summary
            .get("one_line_summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        summary_json: serde_json::to_string(&summary).ok()?,
    };

    let phrase_obs = phrase_observations(&item.item_id, &item.artifact_type, &payload);
    let seed_obs = seed_observations(&item.item_id, &item.artifact_type, &payload, &summary);

    Some(ParsedArtifact {
        item,
        phrase_observations: phrase_obs,
        seed_observations: seed_obs,
    })
}

fn summarize_payload(
    artifact_type: &str,
    payload: &Value,
    path: Option<&Path>,
    seed_passage_id: &str,
    phrase: &str,
) -> Value {
    if let Some(obj) = payload.as_object() {
        match artifact_type {
            "semantic_research" => {
                let schema = obj.get("schema").and_then(|v| v.as_str()).unwrap_or("");
                let query = obj.get("query").and_then(|v| v.as_object());
                let phrase_value = query
                    .and_then(|q| q.get("raw"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(phrase);
                let returned = obj
                    .get("results")
                    .and_then(|v| v.as_object())
                    .and_then(|r| r.get("returned_count"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or_else(|| {
                        obj.get("evidence")
                            .and_then(|v| v.as_array())
                            .map(|a| a.len() as i64)
                            .unwrap_or(0)
                    });
                json!({
                    "phrase": phrase_value,
                    "lens": query.and_then(|q| q.get("query_type")).and_then(|v| v.as_str()).unwrap_or(""),
                    "change_count": returned,
                    "one_line_summary": format!("Semantic research {} for {}: {} evidence/results", schema, phrase_value, returned),
                    "status": "completed",
                    "result_count": returned
                })
            }
            "search_result" => {
                let total_hits = obj
                    .get("total_matches")
                    .or(obj.get("returned_count"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let phrase_value = obj.get("phrase").and_then(|v| v.as_str()).unwrap_or(phrase);
                let result_count = obj
                    .get("returned_count")
                    .or(obj.get("results"))
                    .and_then(|v| v.as_array())
                    .map(|a| a.len() as i64)
                    .unwrap_or(0);
                json!({
                    "phrase": phrase_value,
                    "change_count": total_hits,
                    "one_line_summary": format!("Search for {}: {} total hits", phrase_value, total_hits),
                    "status": "completed",
                    "result_count": result_count
                })
            }
            "frontier_packet" => {
                let similar = obj
                    .get("similar_passages")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let phrases = obj
                    .get("phrase_frontiers")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let next_seeds = obj
                    .get("next_seed_candidates")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let seed = obj
                    .get("seed_passage_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(seed_passage_id);
                json!({
                    "seed_passage_id": seed,
                    "change_count": similar + phrases + next_seeds,
                    "one_line_summary": format!("Frontier packet for {}: {} similar, {} phrase frontiers", seed, similar, phrases),
                    "status": "completed",
                    "similar_count": similar,
                    "frontier_count": phrases,
                    "next_seed_count": next_seeds
                })
            }
            "frontier_report" => {
                let frontiers = obj
                    .get("frontiers")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let searches = obj
                    .get("searches_run")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let seed = obj
                    .get("seed_passage_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| {
                        let task_id = obj
                            .get("source_task_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let result = seed_from_task_id(task_id);
                        if result.is_empty() {
                            seed_passage_id
                        } else {
                            seed_passage_id
                        }
                    });
                json!({
                    "seed_passage_id": seed,
                    "lens": obj.get("active_lens").and_then(|v| v.as_str()).unwrap_or(""),
                    "change_count": frontiers + searches,
                    "one_line_summary": format!("Frontier report for {}: {} frontiers, {} searches", seed, frontiers, searches),
                    "status": obj.get("document_level_map_status").and_then(|v| v.as_str()).unwrap_or("completed"),
                    "frontier_count": frontiers
                })
            }
            "dossier" => {
                let empty: Vec<Value> = vec![];
                let rings = obj
                    .get("rings")
                    .and_then(|v| v.as_array())
                    .unwrap_or(&empty);
                let ring_items: i64 = rings
                    .iter()
                    .filter_map(|r| r.as_object())
                    .map(|r| {
                        r.get("items")
                            .and_then(|v| v.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0) as i64
                    })
                    .sum();
                let seed = obj
                    .get("seed_passage_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(seed_passage_id);
                json!({
                    "seed_passage_id": seed,
                    "lens": obj.get("active_lens").and_then(|v| v.as_str()).unwrap_or(""),
                    "change_count": ring_items + obj.get("unresolved_gaps").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0) as i64,
                    "one_line_summary": format!("Dossier for {}: {} ring items", seed, ring_items),
                    "status": "completed",
                    "scene_1_mapped": obj.get("scene_1_mapped").and_then(|v| v.as_bool()).unwrap_or(false),
                    "scene_2_mapped": obj.get("scene_2_mapped").and_then(|v| v.as_bool()).unwrap_or(false)
                })
            }
            "graph_draft" => {
                let nodes = obj
                    .get("nodes")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let edges = obj
                    .get("edges")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let seed = obj
                    .get("nodes")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_object())
                    .and_then(|o| o.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(seed_passage_id);
                json!({
                    "seed_passage_id": seed,
                    "change_count": nodes + edges,
                    "one_line_summary": format!("Graph draft: {} nodes, {} edges", nodes, edges),
                    "status": "completed",
                    "node_count": nodes,
                    "edge_count": edges
                })
            }
            "task_packet" => {
                let seed = obj
                    .get("seed")
                    .and_then(|v| v.as_object())
                    .and_then(|o| o.get("passage_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(seed_passage_id);
                let candidates = obj
                    .get("candidates")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                json!({
                    "seed_passage_id": seed,
                    "change_count": candidates,
                    "one_line_summary": format!("Task packet for {}: {} candidates", seed, candidates),
                    "status": "completed"
                })
            }
            "adjudication" => {
                let seed = obj
                    .get("seed_passage_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(seed_passage_id);
                let accepted = obj
                    .get("accepted_claims")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let rejected = obj
                    .get("rejected_candidates")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                json!({
                    "seed_passage_id": seed,
                    "change_count": accepted + rejected,
                    "one_line_summary": format!("Adjudication for {}: {} accepted, {} rejected", seed, accepted, rejected),
                    "status": "completed",
                    "accepted_claim_count": accepted,
                    "rejected_claim_count": rejected
                })
            }
            "survey_json" => {
                let count = count_json_changes(payload);
                json!({
                    "change_count": count,
                    "one_line_summary": format!("Survey data: {} structured entries", count),
                    "status": "completed"
                })
            }
            _ => json!({
                "change_count": 0,
                "one_line_summary": format!("{} at {}", artifact_type, path.map(|p| p.display().to_string()).unwrap_or_default()),
                "status": "completed"
            }),
        }
    } else if let Some(_arr) = payload.as_array() {
        if artifact_type == "readzen_collection" {
            let passages: usize = payload
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|item| item.as_object())
                .map(|item| {
                    item.get("passages")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0)
                })
                .sum();
            let edges: usize = payload
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|item| item.as_object())
                .map(|item| {
                    item.get("edges")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0)
                })
                .sum();
            json!({
                "change_count": passages + edges,
                "one_line_summary": format!("ReadZen collection export: {} passages, {} edges", passages, edges),
                "status": "completed",
                "node_count": passages,
                "edge_count": edges
            })
        } else {
            json!({
                "change_count": 0,
                "one_line_summary": format!("{} at {}", artifact_type, path.map(|p| p.display().to_string()).unwrap_or_default()),
                "status": "completed"
            })
        }
    } else if let Some(s) = payload.as_str() {
        let seeds = passage_ids(s);
        let headings = Regex::new(r"^#+\s").unwrap().find_iter(s).count();
        let table_rows = Regex::new(r"^\|.*\|$").unwrap().find_iter(s).count();
        json!({
            "seed_passage_id": seeds.first().map(|s| s.as_str()).unwrap_or(seed_passage_id),
            "change_count": headings + table_rows,
            "one_line_summary": format!("{}: {} headings, {} table rows", artifact_type.replace('_', " "), headings, table_rows),
            "status": "completed",
            "recommended_next_actions": seeds.iter().take(10).cloned().collect::<Vec<_>>()
        })
    } else {
        json!({
            "change_count": 0,
            "one_line_summary": format!("{} at {}", artifact_type, path.map(|p| p.display().to_string()).unwrap_or_default()),
            "status": "completed"
        })
    }
}

fn phrase_observations(
    item_id: &str,
    artifact_type: &str,
    payload: &Value,
) -> Vec<PhraseObservation> {
    let mut observations = Vec::new();

    if let Some(obj) = payload.as_object() {
        if artifact_type == "semantic_research" {
            let query = obj.get("query").and_then(|v| v.as_object());
            let phrase = query
                .and_then(|q| q.get("raw"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let result_count = obj
                .get("results")
                .and_then(|v| v.as_object())
                .and_then(|r| r.get("returned_count"))
                .and_then(|v| v.as_i64())
                .unwrap_or_else(|| {
                    obj.get("evidence")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len() as i64)
                        .unwrap_or(0)
                });
            observations.push(PhraseObservation {
                item_id: item_id.to_string(),
                phrase: phrase.to_string(),
                normalized_phrase: crate::normalize::normalize_zh(phrase),
                total_hits: result_count as i32,
                result_count: result_count as i32,
                graph_potential: "semantic_research".to_string(),
                risks: obj
                    .get("caveats")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .take(3)
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                    .unwrap_or_default(),
                recommended_next_seed: String::new(),
                summary_json: serde_json::to_string(obj).unwrap_or_default(),
            });
        }

        if artifact_type == "search_result" {
            let phrase = obj.get("phrase").and_then(|v| v.as_str()).unwrap_or("");
            let total_hits = obj
                .get("total_matches")
                .or(obj.get("returned_count"))
                .and_then(|v| v.as_i64())
                .or_else(|| {
                    obj.get("results")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len() as i64)
                })
                .unwrap_or(0);
            let result_count = obj
                .get("returned_count")
                .or(obj.get("results"))
                .and_then(|v| v.as_array())
                .map(|a| a.len() as i64)
                .unwrap_or(0);
            observations.push(PhraseObservation {
                item_id: item_id.to_string(),
                phrase: phrase.to_string(),
                normalized_phrase: crate::normalize::normalize_zh(phrase),
                total_hits: total_hits as i32,
                result_count: result_count as i32,
                graph_potential: "unknown".to_string(),
                risks: "unreviewed search result".to_string(),
                recommended_next_seed: String::new(),
                summary_json: serde_json::to_string(&json!({"source": "search_result"}))
                    .unwrap_or_default(),
            });
        }

        if artifact_type == "frontier_report" || artifact_type == "frontier_packet" {
            if let Some(searches) = obj.get("searches_run").and_then(|v| v.as_array()) {
                for search in searches {
                    if let Some(search_obj) = search.as_object() {
                        let phrase = search_obj
                            .get("phrase")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let total_hits = search_obj
                            .get("total_matches")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        observations.push(PhraseObservation {
                            item_id: item_id.to_string(),
                            phrase: phrase.to_string(),
                            normalized_phrase: crate::normalize::normalize_zh(phrase),
                            total_hits: total_hits as i32,
                            result_count: 0,
                            graph_potential: search_obj
                                .get("graph_potential")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            risks: search_obj
                                .get("risks")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            recommended_next_seed: search_obj
                                .get("recommended_next_seed_passage_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            summary_json: serde_json::to_string(search_obj).unwrap_or_default(),
                        });
                    }
                }
            }

            if let Some(frontiers) = obj.get("frontiers").and_then(|v| v.as_array()) {
                for frontier in frontiers {
                    if let Some(frontier_obj) = frontier.as_object() {
                        if let Some(seed_phrases) =
                            frontier_obj.get("seed_phrases").and_then(|v| v.as_array())
                        {
                            for phrase in seed_phrases {
                                let phrase_str = phrase.as_str().unwrap_or("");
                                let total_hits = frontier_obj
                                    .get("searches_run")
                                    .and_then(|v| v.as_array())
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|s| s.as_object())
                                            .filter_map(|s| {
                                                s.get("total_matches").and_then(|v| v.as_i64())
                                            })
                                            .max()
                                            .unwrap_or(0)
                                    })
                                    .unwrap_or(0);
                                observations.push(PhraseObservation {
                                    item_id: item_id.to_string(),
                                    phrase: phrase_str.to_string(),
                                    normalized_phrase: crate::normalize::normalize_zh(phrase_str),
                                    total_hits: total_hits as i32,
                                    result_count: 0,
                                    graph_potential: frontier_obj
                                        .get("graph_potential")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    risks: frontier_obj
                                        .get("risks")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    recommended_next_seed: frontier_obj
                                        .get("recommended_next_seed_passage_id")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    summary_json: serde_json::to_string(frontier_obj)
                                        .unwrap_or_default(),
                                });
                            }
                        }
                    }
                }
            }

            if let Some(phrase_frontiers) = obj.get("phrase_frontiers").and_then(|v| v.as_array()) {
                for frontier in phrase_frontiers {
                    if let Some(frontier_obj) = frontier.as_object() {
                        let phrase = frontier_obj
                            .get("phrase")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let total_hits = frontier_obj
                            .get("total_hits")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        let sample_count = frontier_obj
                            .get("sample_count")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        observations.push(PhraseObservation {
                            item_id: item_id.to_string(),
                            phrase: phrase.to_string(),
                            normalized_phrase: crate::normalize::normalize_zh(phrase),
                            total_hits: total_hits as i32,
                            result_count: sample_count as i32,
                            graph_potential: frontier_obj
                                .get("graph_value_score")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                                .or_else(|| {
                                    frontier_obj
                                        .get("graph_value_score")
                                        .and_then(|v| v.as_f64())
                                        .map(|f| f.to_string())
                                })
                                .unwrap_or_default(),
                            risks: "candidate phrase frontier; review required".to_string(),
                            recommended_next_seed: String::new(),
                            summary_json: serde_json::to_string(frontier_obj).unwrap_or_default(),
                        });
                    }
                }
            }
        }
    }

    observations
}

fn seed_observations(
    item_id: &str,
    artifact_type: &str,
    _payload: &Value,
    summary: &Value,
) -> Vec<SeedObservation> {
    let seed = summary
        .get("seed_passage_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if seed.is_empty() {
        return Vec::new();
    }

    vec![SeedObservation {
        item_id: item_id.to_string(),
        seed_passage_id: seed.to_string(),
        lens: summary
            .get("lens")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        accepted_claim_count: summary
            .get("accepted_claim_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32,
        rejected_claim_count: summary
            .get("rejected_claim_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32,
        node_count: summary
            .get("node_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32,
        edge_count: summary
            .get("edge_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32,
        frontier_count: summary
            .get("frontier_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32,
        similar_count: summary
            .get("similar_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32,
        next_seed_count: summary
            .get("next_seed_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32,
        summary_json: {
            let mut obj = summary.clone();
            obj["artifact_type"] = Value::String(artifact_type.to_string());
            serde_json::to_string(&obj).unwrap_or_default()
        },
    }]
}

fn link_related_items(db_path: &Path) -> Result<()> {
    let con = Connection::open(db_path).context("Failed to open registry database")?;

    let mut stmt = con.prepare(
        "
        SELECT item_id, seed_passage_id, artifact_type
        FROM work_items
        WHERE seed_passage_id != ''
        LIMIT 10000
        ",
    )?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut by_seed: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (item_id, seed, artifact_type) in rows {
        by_seed
            .entry(seed)
            .or_default()
            .push((item_id, artifact_type));
    }

    let mut edge_rows: Vec<(String, String, String)> = Vec::new();
    for items in by_seed.values() {
        let seeds: Vec<String> = items
            .iter()
            .filter(|(_, at)| at == "task_packet")
            .map(|(id, _)| id.clone())
            .collect();

        for seed_item_id in &seeds {
            for (item_id, artifact_type) in items {
                if item_id == seed_item_id {
                    continue;
                }
                edge_rows.push((
                    seed_item_id.clone(),
                    item_id.clone(),
                    format!("same_seed_{}", artifact_type),
                ));
            }
        }
    }

    {
        let mut stmt = con.prepare("INSERT OR IGNORE INTO work_edges VALUES (?, ?, ?)")?;
        for (from_id, to_id, relation) in &edge_rows {
            stmt.execute(params![from_id, to_id, relation])?;
        }
    }

    Ok(())
}

fn ensure_registry(db_path: &Path) -> Result<()> {
    if !db_path.exists() {
        init_registry(db_path)?;
    }
    Ok(())
}

fn artifact_type(rel_path: &str, payload: &Value) -> String {
    if let Some(schema) = payload.get("schema").and_then(|v| v.as_str()) {
        if schema.starts_with("readzen-first-attestation-")
            || schema.starts_with("readzen-phrase-history-")
            || schema.starts_with("readzen-person-")
            || schema.starts_with("readzen-canonical-source-")
            || schema.starts_with("readzen-timeline-")
            || schema.starts_with("readzen-similar-phrase-")
        {
            return "semantic_research".to_string();
        }
    }
    let parts: Vec<&str> = rel_path.split('/').collect();
    if parts.iter().any(|p| *p == "search") {
        return "search_result".to_string();
    }
    if parts.iter().any(|p| *p == "frontiers") {
        if let Some(obj) = payload.as_object() {
            if obj.get("schema").and_then(|v| v.as_str())
                == Some("readzen-graphdiscovery-frontier-v1")
            {
                return "frontier_packet".to_string();
            }
        }
        return "frontier_report".to_string();
    }
    if parts.iter().any(|p| *p == "dossiers") {
        return "dossier".to_string();
    }
    if parts.iter().any(|p| *p == "drafts") {
        return "graph_draft".to_string();
    }
    if parts.iter().any(|p| *p == "tasks") {
        return "task_packet".to_string();
    }
    if parts.iter().any(|p| *p == "adjudications") {
        return "adjudication".to_string();
    }
    if parts.iter().any(|p| *p == "readzen") {
        return "readzen_collection".to_string();
    }
    if rel_path.ends_with(".md") {
        return "survey_markdown".to_string();
    }
    if rel_path.ends_with(".json") {
        return "survey_json".to_string();
    }
    "artifact".to_string()
}

fn load_payload(path: &Path, content: &[u8]) -> Option<Value> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => serde_json::from_slice(content).ok(),
        Some("md") => serde_json::to_value(String::from_utf8_lossy(content).to_string()).ok(),
        _ => serde_json::to_value(String::from_utf8_lossy(content).to_string()).ok(),
    }
}

fn item_id(path: &str, digest: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}:{}", path, digest));
    let result = hasher.finalize();
    hex::encode(result)[..24].to_string()
}

fn sha256_hash(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    hex::encode(result)
}

fn seed_from_nodes(nodes: &Value) -> String {
    nodes
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|first| first.as_object())
        .and_then(|obj| obj.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn seed_from_task_id(_task_id: &str) -> String {
    String::new()
}

fn count_json_changes(payload: &Value) -> i64 {
    if let Some(obj) = payload.as_object() {
        obj.values().map(count_json_changes).sum()
    } else if let Some(arr) = payload.as_array() {
        arr.len() as i64
    } else {
        0
    }
}

fn passage_ids(text: &str) -> Vec<String> {
    let pattern = Regex::new(r"[A-Z]+/[A-Z0-9]+/[A-Z0-9]+n[AB]?\d+\.xml#[A-Za-z0-9_.:-]+").unwrap();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for mat in pattern.find_iter(text) {
        seen.insert(mat.as_str().to_string());
    }
    seen.into_iter().collect()
}
