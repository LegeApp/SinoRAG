use clap::Parser;
use graphdiscovery::commands;
use graphdiscovery::normalize::normalize_zh;
use graphdiscovery::tei::{extract_passages_from_file, load_buddhist_metadata, BuddhistMeta};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn normalize_matches_python_behavior() {
    assert_eq!(normalize_zh("佛、法　僧！"), "佛法僧");
}

#[test]
fn char_ngrams_are_char_not_byte_based() {
    let grams: Vec<_> = graphdiscovery::tfidf::ngram::char_ngrams("祖師西來意", 2, 3).collect();
    assert!(grams.contains(&"祖師".to_string()));
    assert!(grams.contains(&"祖師西".to_string()));
}

#[test]
fn quick_xml_parser_extracts_structural_passage_fields() -> TestResult {
    let dir = tempfile::tempdir()?;
    let xml_path = dir.path().join("T48n2005.xml");
    fs::write(
        &xml_path,
        r#"<?xml version="1.0" encoding="UTF-8"?>
        <TEI xmlns="http://www.tei-c.org/ns/1.0" xmlns:cb="http://www.cbeta.org/ns/1.0">
          <text><body>
            <cb:div type="case"><cb:mulu level="1">青州布衫</cb:mulu>
              <lb n="0001a01" ed="T"/>
              <p xml:id="p1">僧問。<persName>師</persName>云。<term>青州布衫</term><foreign>bodhi</foreign>重七斤。<note>skip me</note></p>
            </cb:div>
          </body></text>
        </TEI>
        "#,
    )?;

    let passages =
        extract_passages_from_file(&xml_path, "T/T48/T48n2005.xml", &BuddhistMeta::default())?;
    let passage = &passages[0];

    assert_eq!(passage.passage_id, "T/T48/T48n2005.xml#p1");
    assert_eq!(passage.div_path, "case");
    assert_eq!(passage.heading, "青州布衫");
    assert_eq!(passage.heading_path, "青州布衫");
    assert_eq!(passage.from_lb.as_deref(), Some("0001a01"));
    assert_eq!(passage.text_type, "dialogue");
    assert!(passage.contains_person);
    assert!(passage.contains_term);
    assert!(passage.contains_foreign);
    assert!(!passage.zh_text_raw.contains("skip me"));
    assert_eq!(passage.zh, passage.zh_text_raw);
    assert_eq!(passage.normalized_zh, passage.zh_text_normalized);
    Ok(())
}

#[test]
fn metadata_loader_normalizes_absolute_xml_p5_paths() -> TestResult {
    let dir = tempfile::tempdir()?;
    let corpus = write_corpus(dir.path())?;
    let metadata = load_buddhist_metadata(&corpus, None)?;
    assert_eq!(metadata["T/T48/T48n2005.xml"].main_title, "Wumenguan");
    Ok(())
}

#[test]
fn ingest_writes_jsonl_and_duckdb() -> TestResult {
    let dir = tempfile::tempdir()?;
    let corpus = write_corpus(dir.path())?;
    let runs = dir.path().join("runs");
    let jsonl_path = runs.join("passages.jsonl");
    let parquet_path = runs.join("passages.parquet");
    let db_path = runs.join("graphdiscovery.duckdb");

    run_cli(vec![
        "graphdiscovery",
        "ingest",
        "--corpus",
        corpus.to_str().unwrap(),
        "--out-jsonl",
        jsonl_path.to_str().unwrap(),
        "--out-parquet",
        parquet_path.to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
        "--zen-only",
    ])?;

    assert!(jsonl_path.is_file());
    assert!(parquet_path.join("part-000000.parquet").is_file());
    assert!(db_path.is_file());
    let row: Value =
        serde_json::from_str(fs::read_to_string(&jsonl_path)?.lines().next().unwrap())?;
    assert_eq!(row["passage_id"], "T/T48/T48n2005.xml#p1");
    assert_eq!(row["period"], "Song");
    assert_eq!(row["zh"], row["zh_text_raw"]);

    let db_rows = graphdiscovery::db::query_json(
        &db_path,
        "SELECT passage_id, canon, period FROM passages ORDER BY passage_id",
    )?;
    assert_eq!(db_rows.len(), 1);
    assert_eq!(db_rows[0]["canon"], "T");
    Ok(())
}

#[test]
fn tfidf_build_info_and_similar_use_postings_index() -> TestResult {
    let dir = tempfile::tempdir()?;
    let corpus = write_reuse_corpus(dir.path())?;
    let runs = dir.path().join("runs");
    let db_path = runs.join("graphdiscovery.duckdb");
    let index_path = runs.join("tfidf.index");
    let similar_path = runs.join("similar.json");

    run_cli(vec![
        "graphdiscovery",
        "ingest",
        "--corpus",
        corpus.to_str().unwrap(),
        "--out-jsonl",
        runs.join("passages.jsonl").to_str().unwrap(),
        "--out-parquet",
        runs.join("passages.parquet").to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
    ])?;
    run_cli(vec![
        "graphdiscovery",
        "tfidf-build",
        "--db",
        db_path.to_str().unwrap(),
        "--out",
        index_path.to_str().unwrap(),
        "--min-ngram",
        "2",
        "--max-ngram",
        "4",
        "--min-df",
        "1",
        "--max-df-ratio",
        "1.0",
    ])?;
    assert!(index_path.is_file());

    run_cli(vec![
        "graphdiscovery",
        "similar",
        "--db",
        db_path.to_str().unwrap(),
        "--index",
        index_path.to_str().unwrap(),
        "--seed",
        "T/T48/T48n2005.xml#p1",
        "--limit",
        "2",
        "--out",
        similar_path.to_str().unwrap(),
    ])?;
    let payload: Value = serde_json::from_str(&fs::read_to_string(similar_path)?)?;
    assert_eq!(payload["seed"], "T/T48/T48n2005.xml#p1");
    assert_eq!(payload["results"][0]["passage_id"], "T/T48/T48n2005.xml#p2");
    assert!(payload["results"][0]["tfidf_cosine"].as_f64().unwrap() > 0.0);
    assert!(
        payload["results"][0]["shared_ngrams"]
            .as_array()
            .unwrap()
            .len()
            > 0
    );
    Ok(())
}

#[test]
fn similar_batch_and_frontier_write_expected_payloads() -> TestResult {
    let dir = tempfile::tempdir()?;
    let corpus = write_reuse_corpus(dir.path())?;
    let runs = dir.path().join("runs");
    let db_path = runs.join("graphdiscovery.duckdb");
    let index_path = runs.join("tfidf.index");
    let seeds_path = runs.join("seeds.txt");
    let batch_path = runs.join("similar.jsonl");
    let frontier_path = runs.join("frontier.json");

    ingest_and_build_tfidf(&corpus, &runs, &db_path, &index_path)?;
    fs::write(&seeds_path, "T/T48/T48n2005.xml#p1\n# comment\n\n")?;

    run_cli(vec![
        "graphdiscovery",
        "similar-batch",
        "--db",
        db_path.to_str().unwrap(),
        "--index",
        index_path.to_str().unwrap(),
        "--seeds",
        seeds_path.to_str().unwrap(),
        "--limit",
        "2",
        "--out",
        batch_path.to_str().unwrap(),
    ])?;
    let lines: Vec<_> = fs::read_to_string(&batch_path)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<_, _>>()?;
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["seed"], "T/T48/T48n2005.xml#p1");
    assert_eq!(
        lines[0]["results"][0]["passage_id"],
        "T/T48/T48n2005.xml#p2"
    );

    run_cli(vec![
        "graphdiscovery",
        "frontier",
        "--db",
        db_path.to_str().unwrap(),
        "--index",
        index_path.to_str().unwrap(),
        "--seed",
        "T/T48/T48n2005.xml#p1",
        "--limit",
        "2",
        "--phrase-limit",
        "5",
        "--out",
        frontier_path.to_str().unwrap(),
    ])?;
    let payload: Value = serde_json::from_str(&fs::read_to_string(frontier_path)?)?;
    assert_eq!(payload["schema"], "readzen-graphdiscovery-frontier-v1");
    assert_eq!(payload["seed_passage_id"], "T/T48/T48n2005.xml#p1");
    assert_eq!(
        payload["similar_passages"][0]["passage_id"],
        "T/T48/T48n2005.xml#p2"
    );
    assert!(payload["phrase_frontiers"].as_array().unwrap().len() > 0);
    assert!(payload["next_seed_candidates"].as_array().unwrap().len() > 0);
    Ok(())
}

fn run_cli(args: Vec<&str>) -> anyhow::Result<()> {
    let cli = graphdiscovery::cli::Cli::try_parse_from(args)?;
    commands::run(cli)
}

fn ingest_and_build_tfidf(
    corpus: &Path,
    runs: &Path,
    db_path: &Path,
    index_path: &Path,
) -> TestResult {
    run_cli(vec![
        "graphdiscovery",
        "ingest",
        "--corpus",
        corpus.to_str().unwrap(),
        "--out-jsonl",
        runs.join("passages.jsonl").to_str().unwrap(),
        "--out-parquet",
        runs.join("passages.parquet").to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
    ])?;
    run_cli(vec![
        "graphdiscovery",
        "tfidf-build",
        "--db",
        db_path.to_str().unwrap(),
        "--out",
        index_path.to_str().unwrap(),
        "--min-ngram",
        "2",
        "--max-ngram",
        "4",
        "--min-df",
        "1",
        "--max-df-ratio",
        "1.0",
    ])?;
    Ok(())
}

fn write_corpus(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let corpus = root.join("CbetaZenTexts");
    let xml_dir = corpus.join("xml-p5/T/T48");
    let metadata_dir = corpus.join("CBETA_Sorting_Data");
    fs::create_dir_all(&xml_dir)?;
    fs::create_dir_all(&metadata_dir)?;

    let xml_path = xml_dir.join("T48n2005.xml");
    fs::write(
        &xml_path,
        r#"<?xml version="1.0" encoding="UTF-8"?>
        <TEI xmlns="http://www.tei-c.org/ns/1.0" xmlns:cb="http://www.cbeta.org/ns/1.0">
          <text><body>
            <cb:div type="case"><cb:mulu level="1">青州布衫</cb:mulu>
              <lb n="0001a01" ed="T"/><p xml:id="p1">僧問。如何是佛。師云。青州布衫重七斤。</p>
            </cb:div>
          </body></text>
        </TEI>
        "#,
    )?;
    fs::write(
        metadata_dir.join("buddhist_metadata_analysis.json"),
        serde_json::json!({
            "detailed_analysis": [{
                "file": xml_path.display().to_string(),
                "canon": "T",
                "canon_name": "Taisho",
                "traditions": ["Chan/Zen"],
                "period": "Song",
                "origin": "China",
                "author": "Wumen",
                "main_title": "Wumenguan"
            }]
        })
        .to_string(),
    )?;
    Ok(corpus)
}

fn write_reuse_corpus(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let corpus = root.join("CbetaZenTexts");
    let xml_dir = corpus.join("xml-p5/T/T48");
    fs::create_dir_all(&xml_dir)?;
    fs::write(
        xml_dir.join("T48n2005.xml"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
        <TEI xmlns="http://www.tei-c.org/ns/1.0" xmlns:cb="http://www.cbeta.org/ns/1.0">
          <text><body>
            <cb:div type="case"><cb:mulu level="1">青州布衫</cb:mulu>
              <lb n="0001a01" ed="T"/><p xml:id="p1">僧問。如何是佛。師云。青州布衫重七斤。</p>
              <lb n="0001a02" ed="T"/><p xml:id="p2">僧問。如何是祖師西來意。師云。青州布衫重七斤。</p>
              <lb n="0001a03" ed="T"/><p xml:id="p3">春風花草滿園香。流水高山各自長。</p>
            </cb:div>
          </body></text>
        </TEI>
        "#,
    )?;
    Ok(corpus)
}

#[test]
fn completion_registry_catalogs_prior_work_and_phrase_status() -> TestResult {
    let dir = tempfile::tempdir()?;
    let runs = dir.path().join("runs");
    let registry_path = runs.join("completions.duckdb");
    let search_dir = runs.join("text-reuse-discovery/search");
    let frontier_dir = runs.join("text-reuse-discovery/frontiers");
    fs::create_dir_all(&search_dir)?;
    fs::create_dir_all(&frontier_dir)?;

    fs::write(
        search_dir.join("鐵枷.json"),
        serde_json::json!({
            "phrase": "鐵枷",
            "normalized_phrase": "鐵枷",
            "total_matches": 71,
            "returned_count": 8,
            "results": [{"passage_id": "T/T48/T48n2005.xml#p1"}]
        })
        .to_string(),
    )?;

    fs::write(
        frontier_dir.join("seed.frontier.json"),
        serde_json::json!({
            "schema": "readzen-graphdiscovery-frontier-v1",
            "seed_passage_id": "T/T48/T48n2005.xml#p1",
            "similar_passages": [{"passage_id": "T/T48/T48n2005.xml#p2"}],
            "phrase_frontiers": [{"phrase": "鐵枷", "total_hits": 71, "sample_count": 8}],
            "next_seed_candidates": [{"passage_id": "T/T48/T48n2005.xml#p2"}]
        })
        .to_string(),
    )?;

    let catalog_result = graphdiscovery::registry::catalog_runs(&runs, &registry_path)?;
    assert_eq!(catalog_result["cataloged"], 2);

    let phrase_status = graphdiscovery::registry::phrase_status(&registry_path, "鐵枷", 20)?;
    assert_eq!(phrase_status["normalized_phrase"], "鐵枷");
    assert_eq!(phrase_status["observation_count"], 2);
    assert_eq!(phrase_status["observations"][0]["total_hits"], 71);

    let prior_work =
        graphdiscovery::registry::prior_work(&registry_path, "T/T48/T48n2005.xml#p1", 20)?;
    assert_eq!(prior_work[0]["artifact_type"], "frontier_packet");

    Ok(())
}

#[test]
fn frontier_includes_registry_prior_work() -> TestResult {
    let dir = tempfile::tempdir()?;
    let corpus = write_reuse_corpus(dir.path())?;
    let runs = dir.path().join("runs");
    let db_path = runs.join("graphdiscovery.duckdb");
    let index_path = runs.join("tfidf.index");
    let frontier_path = runs.join("frontier.json");
    let registry_path = runs.join("completions.duckdb");

    ingest_and_build_tfidf(&corpus, &runs, &db_path, &index_path)?;

    let prior_path = runs.join("frontiers/prior.frontier.json");
    fs::create_dir_all(prior_path.parent().unwrap())?;
    fs::write(
        &prior_path,
        serde_json::json!({
            "schema": "readzen-graphdiscovery-frontier-v1",
            "seed_passage_id": "T/T48/T48n2005.xml#p1",
            "similar_passages": [],
            "phrase_frontiers": [],
            "next_seed_candidates": []
        })
        .to_string(),
    )?;

    graphdiscovery::registry::catalog_runs(&runs, &registry_path)?;

    run_cli(vec![
        "graphdiscovery",
        "frontier",
        "--db",
        db_path.to_str().unwrap(),
        "--index",
        index_path.to_str().unwrap(),
        "--seed",
        "T/T48/T48n2005.xml#p1",
        "--limit",
        "2",
        "--phrase-limit",
        "5",
        "--out",
        frontier_path.to_str().unwrap(),
        "--registry",
        registry_path.to_str().unwrap(),
    ])?;

    let payload: Value = serde_json::from_str(&fs::read_to_string(frontier_path)?)?;
    assert!(!payload["prior_work"].as_array().unwrap().is_empty());
    assert!(payload["prior_work"][0]["path"]
        .as_str()
        .unwrap()
        .ends_with("prior.frontier.json"));

    Ok(())
}

#[test]
fn validate_command_checks_adjudication_structure() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adjudication_path = dir.path().join("adjudication.json");

    // Valid adjudication
    fs::write(
        &adjudication_path,
        serde_json::json!({
            "task_id": "test-task",
            "seed_passage_id": "T/T48/T48n2005.xml#p1",
            "accepted_claims": [{
                "claim_id": "claim-1",
                "evidence": [
                    {"passage_id": "seed", "zh_quote": "佛", "evidence_role": "seed"},
                    {"passage_id": "candidate", "zh_quote": "佛", "evidence_role": "candidate"}
                ],
                "matched_phrases": ["佛"],
                "relation_label": "exact-reuse",
                "review_state": "needs_review",
                "graph_hint": {"research_lens": "test"}
            }],
            "rejected_candidates": []
        })
        .to_string(),
    )?;

    run_cli(vec![
        "graphdiscovery",
        "validate",
        "--adjudication",
        adjudication_path.to_str().unwrap(),
    ])?;

    // Missing required field
    fs::write(
        &adjudication_path,
        serde_json::json!({
            "task_id": "test-task",
            "accepted_claims": [],
            "rejected_candidates": []
        })
        .to_string(),
    )?;

    let result = run_cli(vec![
        "graphdiscovery",
        "validate",
        "--adjudication",
        adjudication_path.to_str().unwrap(),
    ]);
    assert!(result.is_err());

    Ok(())
}

#[test]
fn seed_pick_excludes_already_worked_passages() -> TestResult {
    let dir = tempfile::tempdir()?;
    let corpus = write_reuse_corpus(dir.path())?;
    let runs = dir.path().join("runs");
    let db_path = runs.join("graphdiscovery.duckdb");
    let index_path = runs.join("tfidf.index");
    let registry_path = runs.join("completions.duckdb");

    ingest_and_build_tfidf(&corpus, &runs, &db_path, &index_path)?;

    // Mark p1 as already worked in registry
    fs::create_dir_all(registry_path.parent().unwrap())?;
    graphdiscovery::registry::init_registry(&registry_path)?;
    let prior_path = runs.join("frontiers/prior.frontier.json");
    fs::create_dir_all(prior_path.parent().unwrap())?;
    fs::write(
        &prior_path,
        serde_json::json!({
            "schema": "readzen-graphdiscovery-frontier-v1",
            "seed_passage_id": "T/T48/T48n2005.xml#p1",
            "similar_passages": [],
            "phrase_frontiers": [],
            "next_seed_candidates": []
        })
        .to_string(),
    )?;
    graphdiscovery::registry::catalog_runs(&runs, &registry_path)?;

    run_cli(vec![
        "graphdiscovery",
        "seed-pick",
        "--db",
        db_path.to_str().unwrap(),
        "--registry",
        registry_path.to_str().unwrap(),
        "--limit",
        "5",
    ])?;

    Ok(())
}

#[test]
fn semantic_research_commands_emit_shared_contracts() -> TestResult {
    let dir = tempfile::tempdir()?;
    let corpus = write_semantic_corpus(dir.path())?;
    let runs = dir.path().join("runs");
    let db_path = runs.join("graphdiscovery.duckdb");

    run_cli(vec![
        "graphdiscovery",
        "ingest",
        "--corpus",
        corpus.to_str().unwrap(),
        "--out-jsonl",
        runs.join("passages.jsonl").to_str().unwrap(),
        "--out-parquet",
        runs.join("passages.parquet").to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
    ])?;

    let first_path = runs.join("first.json");
    run_cli(vec![
        "graphdiscovery",
        "first-attestation",
        "--db",
        db_path.to_str().unwrap(),
        "--phrase",
        "應無所住而生其心",
        "--out",
        first_path.to_str().unwrap(),
    ])?;
    let first: Value = serde_json::from_str(&fs::read_to_string(&first_path)?)?;
    assert_eq!(first["schema"], "readzen-first-attestation-v2");
    assert_eq!(first["created_by"], "graphdiscovery-rust");
    assert!(first["evidence"].as_array().unwrap()[0]["zh_quote"]
        .as_str()
        .unwrap()
        .contains("應無所住而生其心"));
    assert!(first["earliest_exact"]["passage_id"]
        .as_str()
        .unwrap()
        .contains("T01n0001"));

    let phrase_index_path = runs.join("phrase.index");
    run_cli(vec![
        "graphdiscovery",
        "phrase-index-build",
        "--db",
        db_path.to_str().unwrap(),
        "--parquet",
        runs.join("passages.parquet").to_str().unwrap(),
        "--out",
        phrase_index_path.to_str().unwrap(),
        "--gram-len",
        "4",
    ])?;
    assert!(phrase_index_path.is_file());

    let phrase_index_search_path = runs.join("phrase-index-search.json");
    run_cli(vec![
        "graphdiscovery",
        "phrase-index-search",
        "--db",
        db_path.to_str().unwrap(),
        "--index",
        phrase_index_path.to_str().unwrap(),
        "--phrase",
        "應無所住而生其心",
        "--out",
        phrase_index_search_path.to_str().unwrap(),
    ])?;
    let phrase_index_search: Value =
        serde_json::from_str(&fs::read_to_string(phrase_index_search_path)?)?;
    assert_eq!(
        phrase_index_search["results"][0]["passage_id"],
        first["earliest_exact"]["passage_id"]
    );

    let indexed_first_path = runs.join("first-indexed.json");
    run_cli(vec![
        "graphdiscovery",
        "first-attestation",
        "--db",
        db_path.to_str().unwrap(),
        "--phrase",
        "應無所住而生其心",
        "--phrase-index",
        phrase_index_path.to_str().unwrap(),
        "--out",
        indexed_first_path.to_str().unwrap(),
    ])?;
    let indexed_first: Value = serde_json::from_str(&fs::read_to_string(indexed_first_path)?)?;
    assert_eq!(
        indexed_first["method"]["search_backend"],
        "phrase_index_verified_by_duckdb"
    );
    assert_eq!(
        indexed_first["earliest_exact"]["passage_id"],
        first["earliest_exact"]["passage_id"]
    );

    let tfidf_shard_dir = runs.join("tfidf-shards");
    run_cli(vec![
        "graphdiscovery",
        "tfidf-shard-build",
        "--parquet-root",
        runs.join("passages.parquet").to_str().unwrap(),
        "--out-dir",
        tfidf_shard_dir.to_str().unwrap(),
        "--min-ngram",
        "2",
        "--max-ngram",
        "4",
        "--min-df",
        "1",
        "--max-df-ratio",
        "1.0",
    ])?;
    assert!(tfidf_shard_dir.join("manifest.json").is_file());

    let canonical_path = runs.join("canonical.json");
    run_cli(vec![
        "graphdiscovery",
        "canonical-source",
        "--db",
        db_path.to_str().unwrap(),
        "--phrase",
        "應無所住而生其心",
        "--canon",
        "T",
        "--out",
        canonical_path.to_str().unwrap(),
    ])?;
    let canonical: Value = serde_json::from_str(&fs::read_to_string(canonical_path)?)?;
    assert_eq!(canonical["schema"], "readzen-canonical-source-v1");
    assert_eq!(canonical["results"]["source_claim"]["status"], "candidate");
    assert!(!canonical["results"]["canon_side_candidates"]
        .as_array()
        .unwrap()
        .is_empty());

    let person_path = runs.join("person.json");
    run_cli(vec![
        "graphdiscovery",
        "person-history",
        "--db",
        db_path.to_str().unwrap(),
        "--name",
        "馬祖",
        "--alias",
        "道一",
        "--out",
        person_path.to_str().unwrap(),
    ])?;
    let person: Value = serde_json::from_str(&fs::read_to_string(&person_path)?)?;
    assert_eq!(person["schema"], "readzen-person-history-v1");
    assert_eq!(
        person["results"]["earliest_unambiguous"]["matched_name_form"],
        "馬祖"
    );
    assert!(person["results"]["mentions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| {
            m.get("matched_name_forms")
                .and_then(|v| v.as_array())
                .map(|forms| forms.iter().any(|v| v.as_str() == Some("道一")))
                .unwrap_or(false)
        }));

    let timeline_path = runs.join("timeline.json");
    run_cli(vec![
        "graphdiscovery",
        "timeline",
        "--db",
        db_path.to_str().unwrap(),
        "--phrase",
        "平常心是道",
        "--out",
        timeline_path.to_str().unwrap(),
    ])?;
    let timeline: Value = serde_json::from_str(&fs::read_to_string(&timeline_path)?)?;
    assert_eq!(timeline["schema"], "readzen-timeline-v1");
    assert!(timeline["results"]["buckets"].as_array().unwrap().len() >= 2);

    let markdown_path = runs.join("first.md");
    run_cli(vec![
        "graphdiscovery",
        "export-markdown",
        "--input",
        first_path.to_str().unwrap(),
        "--out",
        markdown_path.to_str().unwrap(),
        "--title",
        "Diamond Sutra Phrase",
    ])?;
    let markdown = fs::read_to_string(&markdown_path)?;
    assert!(markdown.contains("# Diamond Sutra Phrase"));
    assert!(markdown.contains("應無所住而生其心"));
    assert!(markdown.contains("## Evidence"));

    let readzen_path = runs.join("readzen.json");
    run_cli(vec![
        "graphdiscovery",
        "export-readzen",
        "--input",
        first_path.to_str().unwrap(),
        "--out",
        readzen_path.to_str().unwrap(),
        "--name",
        "Diamond Phrase Evidence",
    ])?;
    let readzen: Value = serde_json::from_str(&fs::read_to_string(readzen_path)?)?;
    assert_eq!(readzen[0]["Name"], "Diamond Phrase Evidence");
    assert!(readzen[0]["Passages"][0]["ZhText"]
        .as_str()
        .unwrap()
        .contains("應無所住而生其心"));

    let graph_path = runs.join("evidence-graph.json");
    run_cli(vec![
        "graphdiscovery",
        "graph-build",
        "--input",
        first_path.to_str().unwrap(),
        "--out",
        graph_path.to_str().unwrap(),
        "--kind",
        "evidence",
    ])?;
    let graph: Value = serde_json::from_str(&fs::read_to_string(graph_path)?)?;
    assert_eq!(graph["schema"], "readzen-text-reuse-graph-draft-v1");
    assert_eq!(
        graph["layout_policy"]["edge_direction"],
        "all-directions-allowed"
    );

    let timeline_graph_path = runs.join("timeline-graph.json");
    run_cli(vec![
        "graphdiscovery",
        "graph-build",
        "--input",
        timeline_path.to_str().unwrap(),
        "--out",
        timeline_graph_path.to_str().unwrap(),
        "--kind",
        "timeline",
    ])?;
    let timeline_graph: Value = serde_json::from_str(&fs::read_to_string(timeline_graph_path)?)?;
    assert_eq!(timeline_graph["layout_policy"]["orientation"], "horizontal");
    assert_eq!(timeline_graph["layout_policy"]["primary_shape"], "standard");

    let lineage_graph_path = runs.join("lineage-graph.json");
    run_cli(vec![
        "graphdiscovery",
        "graph-build",
        "--input",
        person_path.to_str().unwrap(),
        "--out",
        lineage_graph_path.to_str().unwrap(),
        "--kind",
        "lineage",
    ])?;
    let lineage_graph: Value = serde_json::from_str(&fs::read_to_string(lineage_graph_path)?)?;
    assert_eq!(lineage_graph["layout_policy"]["orientation"], "vertical");
    assert_eq!(lineage_graph["layout_policy"]["primary_shape"], "person");

    let report_path = runs.join("report.json");
    run_cli(vec![
        "graphdiscovery",
        "report-build",
        "--input",
        first_path.to_str().unwrap(),
        "--input",
        timeline_path.to_str().unwrap(),
        "--out",
        report_path.to_str().unwrap(),
        "--title",
        "Combined Research Dossier",
    ])?;
    let report: Value = serde_json::from_str(&fs::read_to_string(&report_path)?)?;
    assert_eq!(report["schema"], "readzen-research-report-v1");
    assert_eq!(report["title"], "Combined Research Dossier");
    assert!(report["evidence"].as_array().unwrap().len() >= 2);
    assert_eq!(
        report["output_contracts"]["pdf"],
        "export-pdf --features pdf-export"
    );

    let similar_path = runs.join("similar-phrase.json");
    run_cli(vec![
        "graphdiscovery",
        "similar-phrase",
        "--db",
        db_path.to_str().unwrap(),
        "--phrase",
        "平常心是道",
        "--out",
        similar_path.to_str().unwrap(),
    ])?;
    let similar: Value = serde_json::from_str(&fs::read_to_string(similar_path)?)?;
    assert_eq!(similar["schema"], "readzen-similar-phrase-v1");
    assert!(!similar["results"]["candidates"]
        .as_array()
        .unwrap()
        .is_empty());

    let registry_path = runs.join("completions.duckdb");
    graphdiscovery::registry::catalog_runs(&runs, &registry_path)?;
    let phrase_status = graphdiscovery::registry::phrase_status(&registry_path, "平常心是道", 20)?;
    assert!(phrase_status["observations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|obs| {
            obs.get("graph_potential").and_then(|v| v.as_str()) == Some("semantic_research")
        }));

    Ok(())
}

fn write_semantic_corpus(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let corpus = root.join("CbetaZenTexts");
    let t01_dir = corpus.join("xml-p5/T/T01");
    let t48_dir = corpus.join("xml-p5/T/T48");
    let j01_dir = corpus.join("xml-p5/J/J01");
    let metadata_dir = corpus.join("CBETA_Sorting_Data");
    fs::create_dir_all(&t01_dir)?;
    fs::create_dir_all(&t48_dir)?;
    fs::create_dir_all(&j01_dir)?;
    fs::create_dir_all(&metadata_dir)?;

    let canon_xml = t01_dir.join("T01n0001.xml");
    fs::write(
        &canon_xml,
        r#"<?xml version="1.0" encoding="UTF-8"?>
        <TEI xmlns="http://www.tei-c.org/ns/1.0" xmlns:cb="http://www.cbeta.org/ns/1.0">
          <text><body>
            <cb:div type="sutra"><cb:mulu level="1">金剛經</cb:mulu>
              <lb n="0001a01" ed="T"/><p xml:id="p1">佛告須菩提。應無所住而生其心。菩薩如是修行。</p>
            </cb:div>
          </body></text>
        </TEI>
        "#,
    )?;

    let mazu_xml = t48_dir.join("T48n2005.xml");
    fs::write(
        &mazu_xml,
        r#"<?xml version="1.0" encoding="UTF-8"?>
        <TEI xmlns="http://www.tei-c.org/ns/1.0" xmlns:cb="http://www.cbeta.org/ns/1.0">
          <text><body>
            <cb:div type="case"><cb:mulu level="1">馬祖示眾</cb:mulu>
              <lb n="0001a01" ed="T"/><p xml:id="p1">馬祖云。平常心是道。道一又曰。應無所住而生其心。</p>
            </cb:div>
          </body></text>
        </TEI>
        "#,
    )?;

    let later_xml = j01_dir.join("J01n0001.xml");
    fs::write(
        &later_xml,
        r#"<?xml version="1.0" encoding="UTF-8"?>
        <TEI xmlns="http://www.tei-c.org/ns/1.0" xmlns:cb="http://www.cbeta.org/ns/1.0">
          <text><body>
            <cb:div type="commentary"><cb:mulu level="1">後世評唱</cb:mulu>
              <lb n="0002a01" ed="J"/><p xml:id="p1">後人評曰。馬祖平常心是道一語流行天下。</p>
            </cb:div>
          </body></text>
        </TEI>
        "#,
    )?;

    fs::write(
        metadata_dir.join("buddhist_metadata_analysis.json"),
        serde_json::json!({
            "detailed_analysis": [
                {
                    "file": canon_xml.display().to_string(),
                    "canon": "T",
                    "canon_name": "Taisho",
                    "traditions": ["Sutra"],
                    "period": "Tang",
                    "origin": "India/China",
                    "author": "Unknown",
                    "main_title": "Diamond Sutra"
                },
                {
                    "file": mazu_xml.display().to_string(),
                    "canon": "T",
                    "canon_name": "Taisho",
                    "traditions": ["Chan/Zen"],
                    "period": "Tang",
                    "origin": "China",
                    "author": "Mazu school",
                    "main_title": "Mazu Sayings"
                },
                {
                    "file": later_xml.display().to_string(),
                    "canon": "J",
                    "canon_name": "Jiaxing",
                    "traditions": ["Chan/Zen"],
                    "period": "Song",
                    "origin": "China",
                    "author": "Later compiler",
                    "main_title": "Later Commentary"
                }
            ]
        })
        .to_string(),
    )?;
    Ok(corpus)
}

#[test]
fn kanripo_to_tei_outputs_ingestible_provenance_tagged_corpus() -> TestResult {
    let dir = tempfile::tempdir()?;
    let repo = dir.path().join("texts/KR3/KR3i0042");
    fs::create_dir_all(&repo)?;
    fs::write(
        repo.join("Readme.org"),
        "#+TITLE: 菌譜\n#+PROPERTY: EDITION WYG 四庫全書・文淵閣\n",
    )?;
    fs::write(
        repo.join("KR3i0042_001.txt"),
        "菌譜卷上\n\n菌生於山林。其味清美。\n非漢非梵之書。\n",
    )?;

    let corpus = dir.path().join("kanripo-build");
    let manifest_dir = dir.path().join("kanripo-manifests");
    run_cli(vec![
        "graphdiscovery",
        "kanripo-manifest",
        "--input",
        dir.path().join("texts").to_str().unwrap(),
        "--out",
        manifest_dir.to_str().unwrap(),
    ])?;
    let work_manifest = fs::read_to_string(manifest_dir.join("work_manifest.jsonl"))?;
    let section_manifest = fs::read_to_string(manifest_dir.join("section_manifest.jsonl"))?;
    let summary: Value = serde_json::from_str(&fs::read_to_string(
        manifest_dir.join("manifest_summary.json"),
    )?)?;
    assert!(work_manifest.contains("\"source_work_id\":\"KR3i0042\""));
    assert!(section_manifest.contains("\"estimated_passage_count\":3"));
    assert_eq!(summary["work_count"], 1);
    assert_eq!(summary["section_count"], 1);

    run_cli(vec![
        "graphdiscovery",
        "kanripo-to-tei",
        "--input",
        dir.path().join("texts").to_str().unwrap(),
        "--out-corpus",
        corpus.to_str().unwrap(),
        "--snapshot-id",
        "test-snapshot",
    ])?;

    assert!(corpus.join("xml-p5/kanripo/KR3i0042.xml").is_file());
    assert!(corpus
        .join("CBETA_Sorting_Data/buddhist_metadata_analysis.json")
        .is_file());

    let runs = dir.path().join("runs");
    let db_path = runs.join("graphdiscovery.duckdb");
    run_cli(vec![
        "graphdiscovery",
        "ingest",
        "--corpus",
        corpus.to_str().unwrap(),
        "--out-jsonl",
        runs.join("passages.jsonl").to_str().unwrap(),
        "--out-parquet",
        runs.join("passages.parquet").to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
    ])?;

    let rows = graphdiscovery::db::query_json(
        &db_path,
        "SELECT source_corpus, source_work_id, rights_id, retrieval_method, snapshot_id, canon, main_title, zh_text_raw FROM passages ORDER BY passage_id",
    )?;
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["source_corpus"], "kanripo");
    assert_eq!(rows[0]["source_work_id"], "KR3i0042");
    assert_eq!(rows[0]["rights_id"], "CC-BY-SA-4.0");
    assert_eq!(rows[0]["retrieval_method"], "local-repository");
    assert_eq!(rows[0]["snapshot_id"], "test-snapshot");
    assert_eq!(rows[0]["canon"], "KANRIPO");
    assert_eq!(rows[0]["main_title"], "菌譜");
    assert!(rows
        .iter()
        .any(|row| row["zh_text_raw"].as_str().unwrap().contains("菌生於山林")));

    let search_path = runs.join("kanripo-search.json");
    run_cli(vec![
        "graphdiscovery",
        "first-attestation",
        "--db",
        db_path.to_str().unwrap(),
        "--phrase",
        "菌生於山林",
        "--out",
        search_path.to_str().unwrap(),
    ])?;
    let payload: Value = serde_json::from_str(&fs::read_to_string(search_path)?)?;
    assert_eq!(payload["evidence"][0]["source_corpus"], "kanripo");
    assert_eq!(payload["evidence"][0]["rights_id"], "CC-BY-SA-4.0");

    Ok(())
}
