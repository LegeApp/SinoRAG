pub mod absence_check;
pub mod build_pack;
pub mod canonical_source;
pub mod catalog_index;
pub mod cef;
pub mod cluster_hits;
pub mod collocation_search;
pub mod compare_usage;
pub mod document_table;
pub mod estimate;
pub mod expand_context;
pub mod expand_context_adaptive;
pub mod export;
pub mod find_first_mention;
pub mod first_attestation;
pub mod frontier;
pub mod ingest;
pub mod ingest_terebess;
pub mod kanripo;
pub mod outline_search;
pub mod passage;
pub mod person_history;
pub mod person_resolve;
pub mod phrase_history;
pub mod phrase_index;
pub mod query_expand_terms;
pub mod research_packet;
pub mod run_tools;
pub mod search;
pub mod seed_pick;
pub mod similar_phrase;
pub mod status;
pub mod taxonomy;
pub mod tfidf;
pub mod timeline;
pub mod tool_call;
pub mod tools_manifest;
pub mod trace_term_usage;
pub mod validate;
pub mod vector_index;

use crate::cli::{Cli, Command, IndexCommand};
use crate::document_table::{match_index_fingerprint, DocumentTable, IndexCoverage};
use crate::phrase_index::PhraseIndex;
use crate::tfidf::index::TfidfIndex;
use anyhow::Result;
use serde_json::json;

#[allow(clippy::too_many_arguments)]
pub fn build_all_indexes(
    parquet: std::path::PathBuf,
    doc_table: std::path::PathBuf,
    phrase_out: std::path::PathBuf,
    tfidf_out: std::path::PathBuf,
    phrase_gram_len: usize,
    min_ngram: usize,
    max_ngram: usize,
    min_df: u32,
    max_df_ratio: f32,
    max_features: usize,
    buckets: usize,
    temp_dir: Option<std::path::PathBuf>,
) -> Result<()> {
    let phrase_temp = temp_dir.as_ref().map(|p| p.join("phrase.work"));
    let tfidf_temp = temp_dir.as_ref().map(|p| p.join("tfidf.work"));
    let doc_table_loaded = DocumentTable::load(&doc_table)?;

    eprintln!("=== Combined index build (phrase + tfidf) ===");
    if phrase_index_is_current(&phrase_out, &doc_table, &doc_table_loaded, phrase_gram_len)? {
        eprintln!("Phrase index is current; skipping.");
    } else {
        phrase_index::build(
            parquet.clone(),
            doc_table.clone(),
            phrase_out,
            phrase_gram_len,
            buckets,
            phrase_temp,
        )?;
    }

    let params = crate::tfidf::index::TfidfParams {
        min_ngram,
        max_ngram,
        min_df,
        max_df_ratio,
        max_features,
        dtype: "float32".to_string(),
        analyzer: "char".to_string(),
    };
    if tfidf_index_is_current(&tfidf_out, &doc_table, &doc_table_loaded, &params)? {
        eprintln!("TF-IDF index is current; skipping.");
        Ok(())
    } else {
        crate::tfidf::index::build(parquet, doc_table, tfidf_out, params, buckets, tfidf_temp)
    }
}

fn phrase_index_is_current(
    index_path: &std::path::Path,
    doc_table_path: &std::path::Path,
    doc_table: &DocumentTable,
    gram_len: usize,
) -> Result<bool> {
    if !index_path.exists() {
        return Ok(false);
    }
    let info = match PhraseIndex::header_info(index_path) {
        Ok(info) => info,
        Err(err) => {
            eprintln!("Existing phrase index is unreadable ({err}); rebuilding.");
            return Ok(false);
        }
    };
    if info.get("gram_len").and_then(|v| v.as_u64()) != Some(gram_len as u64) {
        eprintln!("Existing phrase index has different gram length; rebuilding.");
        return Ok(false);
    }
    let Some(fp) = info.get("doc_table_fingerprint").and_then(|v| v.as_str()) else {
        eprintln!("Existing phrase index has no doc-table fingerprint; rebuilding.");
        return Ok(false);
    };
    match match_index_fingerprint(doc_table, doc_table_path, fp)? {
        Some(IndexCoverage::Full) => Ok(true),
        Some(IndexCoverage::Base { .. }) => {
            eprintln!("Existing phrase index covers only the doc-table base; rebuilding.");
            Ok(false)
        }
        None => {
            eprintln!("Existing phrase index fingerprint differs from doc_table; rebuilding.");
            Ok(false)
        }
    }
}

fn tfidf_index_is_current(
    index_path: &std::path::Path,
    doc_table_path: &std::path::Path,
    doc_table: &DocumentTable,
    params: &crate::tfidf::index::TfidfParams,
) -> Result<bool> {
    if !index_path.exists() {
        return Ok(false);
    }
    let info = match TfidfIndex::header_info(index_path) {
        Ok(info) => info,
        Err(err) => {
            eprintln!("Existing TF-IDF index is unreadable ({err}); rebuilding.");
            return Ok(false);
        }
    };
    if !tfidf_params_match(&info, params) {
        eprintln!("Existing TF-IDF index has different build parameters; rebuilding.");
        return Ok(false);
    }
    let Some(fp) = info.get("doc_table_fingerprint").and_then(|v| v.as_str()) else {
        eprintln!("Existing TF-IDF index has no doc-table fingerprint; rebuilding.");
        return Ok(false);
    };
    match match_index_fingerprint(doc_table, doc_table_path, fp)? {
        Some(IndexCoverage::Full) => Ok(true),
        Some(IndexCoverage::Base { .. }) => {
            eprintln!("Existing TF-IDF index covers only the doc-table base; rebuilding.");
            Ok(false)
        }
        None => {
            eprintln!("Existing TF-IDF index fingerprint differs from doc_table; rebuilding.");
            Ok(false)
        }
    }
}

fn tfidf_params_match(info: &serde_json::Value, params: &crate::tfidf::index::TfidfParams) -> bool {
    let Some(obj) = info.get("params") else {
        return false;
    };
    let ngrams = obj.get("ngram_range").and_then(|v| v.as_array());
    let min_n = ngrams.and_then(|v| v.first()).and_then(|v| v.as_u64());
    let max_n = ngrams.and_then(|v| v.get(1)).and_then(|v| v.as_u64());
    let min_df = obj.get("min_df").and_then(|v| v.as_u64());
    let max_features = obj.get("max_features").and_then(|v| v.as_u64());
    let max_df = obj.get("max_df").and_then(|v| v.as_f64());

    min_n == Some(params.min_ngram as u64)
        && max_n == Some(params.max_ngram as u64)
        && min_df == Some(params.min_df as u64)
        && max_features == Some(params.max_features as u64)
        && max_df
            .map(|v| (v - params.max_df_ratio as f64).abs() < f64::EPSILON)
            .unwrap_or(false)
}

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Status { data } => status::run(data),
        Command::Index { command } => match command {
            IndexCommand::Phrase {
                parquet,
                doc_table,
                out,
                gram_len,
                buckets,
                temp_dir,
            } => phrase_index::build(parquet, doc_table, out, gram_len, buckets, temp_dir),
            IndexCommand::Tfidf {
                parquet,
                doc_table,
                out,
                min_ngram,
                max_ngram,
                min_df,
                max_df_ratio,
                max_features,
                buckets,
                temp_dir,
            } => {
                let params = crate::tfidf::index::TfidfParams {
                    min_ngram,
                    max_ngram,
                    min_df,
                    max_df_ratio,
                    max_features,
                    dtype: "float32".to_string(),
                    analyzer: "char".to_string(),
                };
                crate::tfidf::index::build(parquet, doc_table, out, params, buckets, temp_dir)
            }
            IndexCommand::PhraseInfo { index } => phrase_index::info(index),
            IndexCommand::TfidfInfo { index } => tfidf::info(index),
            IndexCommand::VectorExport {
                parquet,
                doc_table,
                out,
                limit,
            } => vector_index::export(parquet, doc_table, out, limit).await,
            IndexCommand::VectorBuild {
                doc_table,
                embeddings,
                out,
                model_id,
                model_revision,
                max_nb_connection,
                ef_construction,
                nb_layer,
            } => vector_index::build(
                doc_table,
                embeddings,
                out,
                model_id,
                model_revision,
                max_nb_connection,
                ef_construction,
                nb_layer,
            ),
            IndexCommand::VectorInfo { index } => vector_index::info(index),
        },
        Command::OptionalIndexes {
            parquet,
            doc_table,
            phrase_out,
            tfidf_out,
            phrase_gram_len,
            min_ngram,
            max_ngram,
            min_df,
            max_df_ratio,
            max_features,
            buckets,
            temp_dir,
        } => build_all_indexes(
            parquet,
            doc_table,
            phrase_out,
            tfidf_out,
            phrase_gram_len,
            min_ngram,
            max_ngram,
            min_df,
            max_df_ratio,
            max_features,
            buckets,
            temp_dir,
        ),
        Command::Ingest {
            source,
            path,
            out_jsonl,
            out_parquet,
            zen_only,
            resume,
            build_phrase_index,
            phrase_index_out,
            phrase_gram_len,
            build_tfidf,
            tfidf_out,
            catalog_index_out,
            phrase_max_memory,
        } => {
            use crate::cli::IngestSource;
            let (corpus, kanripo_input) = match source {
                IngestSource::Cbeta => (Some(path), None),
                IngestSource::Kanripo => (None, Some(path)),
                IngestSource::Cef => {
                    cef::ingest(path, out_parquet.clone()).await?;
                    return ingest::post_ingest(ingest::PostIngestOptions {
                        out_parquet,
                        build_phrase_index,
                        phrase_index_out,
                        phrase_gram_len,
                        build_tfidf,
                        tfidf_out,
                        catalog_index_out,
                        phrase_max_memory,
                    });
                }
                IngestSource::Terebess => {
                    let images_dir = std::path::PathBuf::from("data/derived/terebess_images");
                    ingest_terebess::run(path, out_parquet.clone(), images_dir, 500)?;
                    return ingest::post_ingest(ingest::PostIngestOptions {
                        out_parquet,
                        build_phrase_index,
                        phrase_index_out,
                        phrase_gram_len,
                        build_tfidf,
                        tfidf_out,
                        catalog_index_out,
                        phrase_max_memory,
                    });
                }
            };
            ingest::run(
                corpus,
                kanripo_input,
                out_jsonl,
                out_parquet,
                zen_only,
                resume,
                build_phrase_index,
                phrase_index_out,
                phrase_gram_len,
                build_tfidf,
                tfidf_out,
                catalog_index_out,
                phrase_max_memory,
            )
            .await
        }
        Command::ExpandContext {
            parquet,
            passage_id,
            session,
            hit,
            before,
            after,
            out,
        } => expand_context::run(parquet, passage_id, session, hit, before, after, out).await,
        Command::CefValidate { input } => cef::validate(input).map(|report| {
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
        }),
        Command::CefInit { out } => cef::init(out),
        Command::CefStats { input } => cef::stats(input),
        Command::IngestCef { input, out_parquet } => {
            cef::ingest(input, out_parquet.clone()).await?;
            ingest::post_ingest(ingest::PostIngestOptions {
                out_parquet,
                build_phrase_index: false,
                phrase_index_out: std::path::PathBuf::from("data/derived/phrase.index"),
                phrase_gram_len: 4,
                build_tfidf: false,
                tfidf_out: Some(std::path::PathBuf::from("data/derived/tfidf.index")),
                catalog_index_out: Some(std::path::PathBuf::from("data/derived/catalog.index")),
                phrase_max_memory: None,
            })
        }
        Command::KanripoToTei {
            input,
            out_corpus,
            snapshot_id,
        } => kanripo::run(input, out_corpus, snapshot_id),
        Command::KanripoManifest { input, out } => kanripo::manifest(input, out),
        Command::Search {
            parquet,
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
            out,
            registry: _,
        } => {
            use crate::tools::{EngineConfig, ToolEngine};

            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index: None,
                tfidf_index: None,
                vector_index: None,
                catalog_index: None,
                doc_table: None,
                registry: None,
                output_root: None,
            };

            let engine = ToolEngine::open(config).await?;

            let canon_opt = if canon.is_empty() {
                None
            } else {
                Some(canon.join(","))
            };
            let tradition_opt = if tradition.is_empty() {
                None
            } else {
                Some(tradition.join(","))
            };
            let period_opt = if period.is_empty() {
                None
            } else {
                Some(period.join(","))
            };
            let origin_opt = if origin.is_empty() {
                None
            } else {
                Some(origin.join(","))
            };

            let req = crate::tools::requests::SearchRequest {
                phrase: phrase.unwrap_or_default(),
                limit,
                mode: "hits".to_string(),
                depth: "exact".to_string(),
                group_by: "work".to_string(),
                include_variants: false,
                limit_per_group: 5,
                brief: false,
                canon: canon_opt,
                source_work_id,
                tradition: tradition_opt,
                period: period_opt,
                origin: origin_opt,
                author,
                title,
                heading_path_prefix,
            };

            let res = engine.search_impl(req).await?;

            if let Some(out_path) = out {
                std::fs::write(out_path, serde_json::to_string_pretty(&res)?)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&res)?);
            }
            Ok(())
        }
        Command::Passage { id, parquet, out } => {
            use crate::tools::{EngineConfig, ToolEngine};

            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index: None,
                tfidf_index: None,
                vector_index: None,
                catalog_index: None,
                doc_table: None,
                registry: None,
                output_root: None,
            };

            let engine = ToolEngine::open(config).await?;

            let req = crate::tools::requests::PassageRequest { id };
            let res = engine.passage_impl(req).await?;

            if let Some(out_path) = out {
                std::fs::write(out_path, serde_json::to_string_pretty(&res)?)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&res)?);
            }
            Ok(())
        }
        Command::PriorWork {
            registry,
            seed,
            limit,
        } => {
            let payload = json!({
                "registry": registry.display().to_string(),
                "seed_passage_id": seed,
                "items": crate::registry::prior_work(&registry, &seed, limit)?,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
            Ok(())
        }
        Command::PhraseStatus {
            registry,
            phrase,
            limit,
        } => {
            let mut payload = crate::registry::phrase_status(&registry, &phrase, limit)?;
            if let Some(obj) = payload.as_object_mut() {
                obj.insert(
                    "registry".to_string(),
                    json!(registry.display().to_string()),
                );
            }
            println!("{}", serde_json::to_string_pretty(&payload)?);
            Ok(())
        }
        Command::Taxonomy { parquet } => taxonomy::run(parquet).await,
        Command::WorkSummary { registry, limit } => {
            let payload = json!({
                "registry": registry.display().to_string(),
                "items": crate::registry::work_summary(&registry, limit)?,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
            Ok(())
        }
        Command::Catalog { runs, registry } => {
            let payload = crate::registry::catalog_runs(&runs, &registry)?;
            println!("{}", serde_json::to_string_pretty(&payload)?);
            Ok(())
        }
        Command::TfidfBuild {
            parquet,
            doc_table,
            out,
            min_ngram,
            max_ngram,
            min_df,
            max_df_ratio,
            max_features,
            buckets,
            temp_dir,
        } => {
            let params = crate::tfidf::index::TfidfParams {
                min_ngram,
                max_ngram,
                min_df,
                max_df_ratio,
                max_features,
                dtype: "float32".to_string(),
                analyzer: "char".to_string(),
            };
            crate::tfidf::index::build(parquet, doc_table, out, params, buckets, temp_dir)
        }
        Command::TfidfInfo { index } => tfidf::info(index),
        Command::PhraseIndexBuild {
            parquet,
            doc_table,
            out,
            gram_len,
            buckets,
            temp_dir,
        } => phrase_index::build(parquet, doc_table, out, gram_len, buckets, temp_dir),
        Command::PhraseIndexInfo { index } => phrase_index::info(index),
        Command::PhraseIndexSearch {
            parquet,
            index,
            phrase,
            limit,
            out,
        } => {
            use crate::tools::{EngineConfig, ToolEngine};

            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index: Some(index),
                tfidf_index: None,
                vector_index: None,
                catalog_index: None,
                doc_table: None,
                registry: None,
                output_root: None,
            };

            let engine = ToolEngine::open(config).await?;

            let req = crate::tools::requests::PhraseIndexSearchRequest { phrase, limit };

            let res = engine.phrase_index_search_impl(req).await?;

            if let Some(out_path) = out {
                std::fs::write(out_path, serde_json::to_string_pretty(&res)?)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&res)?);
            }
            Ok(())
        }
        Command::CatalogIndexBuild {
            parquet,
            out,
            debug_json,
            doc_table,
        } => catalog_index::build(parquet, out, debug_json, doc_table),
        Command::CatalogIndexInfo { index } => {
            use crate::tools::{EngineConfig, ToolEngine};

            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: None,
                phrase_index: None,
                tfidf_index: None,
                vector_index: None,
                catalog_index: Some(index),
                doc_table: None,
                registry: None,
                output_root: None,
            };

            let engine = ToolEngine::open(config).await?;

            let req = crate::tools::requests::CatalogIndexInfoRequest {};
            let res = engine.catalog_index_info_impl(req).await?;
            println!("{}", serde_json::to_string_pretty(&res)?);
            Ok(())
        }
        Command::DocTableBuild {
            parquet,
            out,
            append_to,
        } => document_table::build(parquet, out, append_to),
        Command::IngestTerebess {
            input,
            out_parquet,
            images_dir,
            min_body_chars,
        } => {
            ingest_terebess::run(input, out_parquet.clone(), images_dir, min_body_chars)?;
            ingest::post_ingest(ingest::PostIngestOptions {
                out_parquet,
                build_phrase_index: false,
                phrase_index_out: std::path::PathBuf::from("data/derived/phrase.index"),
                phrase_gram_len: 4,
                build_tfidf: false,
                tfidf_out: Some(std::path::PathBuf::from("data/derived/tfidf.index")),
                catalog_index_out: Some(std::path::PathBuf::from("data/derived/catalog.index")),
                phrase_max_memory: None,
            })
        }
        Command::BuildPack { pack, pack_id } => build_pack::run(pack, pack_id),
        Command::ExpandContextAdaptive {
            parquet,
            catalog,
            passage_id,
            max_chars,
            out,
        } => expand_context_adaptive::run(parquet, catalog, passage_id, max_chars, out).await,
        Command::FindFirstMention {
            parquet,
            phrase_index,
            doc_table,
            phrase,
            scope_canon,
            scope_period,
            scope_source_work_id,
            limit,
            out,
        } => {
            find_first_mention::run(
                parquet,
                phrase_index,
                doc_table,
                phrase,
                scope_canon,
                scope_period,
                scope_source_work_id,
                limit,
                out,
            )
            .await
        }
        Command::TraceTermUsage {
            parquet,
            phrase_index,
            doc_table,
            phrase,
            group_by,
            limit_total,
            limit_per_group,
            out,
        } => {
            trace_term_usage::run(
                parquet,
                phrase_index,
                doc_table,
                phrase,
                group_by,
                limit_total,
                limit_per_group,
                out,
            )
            .await
        }
        Command::QueryExpandTerms {
            phrase,
            mode,
            person_alias,
            max,
            out,
        } => query_expand_terms::run(phrase, mode, person_alias, max, out),
        Command::ResearchPacketBuild {
            pack,
            out,
            recipe,
            brief,
            keep_temp,
            topic,
            notes,
            phrase,
            seed_passage,
            person,
            person_alias,
            work,
            canon,
            period,
        } => {
            research_packet::build(
                pack,
                out,
                recipe,
                brief,
                keep_temp,
                topic,
                notes,
                phrase,
                seed_passage,
                person,
                person_alias,
                work,
                canon,
                period,
            )
            .await
        }
        Command::Works {
            index,
            tradition,
            period,
            canon,
            author,
            limit,
        } => {
            use crate::tools::{EngineConfig, ToolEngine};

            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: None,
                phrase_index: None,
                tfidf_index: None,
                vector_index: None,
                catalog_index: Some(index),
                doc_table: None,
                registry: None,
                output_root: None,
            };

            let engine = ToolEngine::open(config).await?;

            let req = crate::tools::requests::WorksRequest {
                tradition,
                period,
                canon,
                author,
                limit,
            };

            let res = engine.works_impl(req).await?;
            println!("{}", serde_json::to_string_pretty(&res)?);
            Ok(())
        }
        Command::Outline {
            index,
            work,
            node,
            max_depth,
        } => catalog_index::outline(index, work, node, max_depth),
        Command::Sections {
            index,
            work,
            max_depth,
        } => catalog_index::sections(index, work, max_depth),
        Command::Scope { index, node } => catalog_index::scope(index, node),
        Command::ExportMarkdown { input, out, title } => export::markdown(input, out, title),
        Command::ExportReadzen { input, out, name } => export::readzen(input, out, name),
        Command::GraphBuild {
            input,
            out,
            kind,
            name,
        } => {
            let graph_kind = match kind.as_str() {
                "evidence" => export::GraphKind::Evidence,
                "timeline" => export::GraphKind::Timeline,
                "lineage" => export::GraphKind::Lineage,
                other => anyhow::bail!(
                    "unknown graph kind `{other}`; expected evidence, timeline, or lineage"
                ),
            };
            export::graph(input, out, graph_kind, name)
        }
        Command::ReportBuild {
            input,
            out,
            title,
            essay_max_pages,
        } => export::report_build(input, out, title, essay_max_pages),
        Command::ExportPdf {
            input_markdown,
            out,
            side_by_side,
        } => export::pdf(input_markdown, out, side_by_side),
        Command::Similar {
            parquet,
            index,
            seed,
            limit,
            shared_ngram_limit,
            shared_phrase_limit,
            min_shared_phrase_len,
            out,
        } => {
            use crate::tools::{EngineConfig, ToolEngine};

            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index: None,
                tfidf_index: Some(index),
                vector_index: None,
                catalog_index: None,
                doc_table: None,
                registry: None,
                output_root: None,
            };

            let engine = ToolEngine::open(config).await?;

            let req = crate::tools::requests::SimilarRequest {
                seed,
                limit,
                shared_ngram_limit,
                shared_phrase_limit,
                min_shared_phrase_len,
            };

            let res = engine.similar_impl(req).await?;

            if let Some(out_path) = out {
                std::fs::write(out_path, serde_json::to_string_pretty(&res)?)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&res)?);
            }
            Ok(())
        }
        Command::SimilarBatch {
            parquet,
            index,
            seeds,
            limit,
            shared_ngram_limit,
            shared_phrase_limit,
            min_shared_phrase_len,
            out,
        } => {
            tfidf::similar_batch(
                parquet,
                index,
                seeds,
                limit,
                shared_ngram_limit,
                shared_phrase_limit,
                min_shared_phrase_len,
                out,
            )
            .await
        }
        Command::Frontier {
            seed,
            parquet,
            index,
            corpus: _,
            limit,
            phrase_limit,
            out,
            registry: _,
        } => {
            use crate::tools::{EngineConfig, ToolEngine};

            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index: None,
                tfidf_index: Some(index),
                vector_index: None,
                catalog_index: None,
                doc_table: None,
                registry: None,
                output_root: None,
            };

            let engine = ToolEngine::open(config).await?;

            let req = crate::tools::requests::FrontierRequest {
                seed,
                limit,
                phrase_limit,
            };

            let res = engine.frontier_impl(req).await?;

            if let Some(out_path) = out {
                std::fs::write(out_path, serde_json::to_string_pretty(&res)?)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&res)?);
            }
            Ok(())
        }
        Command::Validate { adjudication } => validate::run(adjudication),
        Command::SeedPick {
            parquet,
            registry,
            tradition,
            period,
            limit,
        } => {
            use crate::tools::{EngineConfig, ToolEngine};

            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index: None,
                tfidf_index: None,
                vector_index: None,
                catalog_index: None,
                doc_table: None,
                registry: Some(registry),
                output_root: None,
            };

            let engine = ToolEngine::open(config).await?;

            let req = crate::tools::requests::SeedPickRequest {
                tradition,
                period,
                limit,
            };

            let res = engine.seed_pick_impl(req).await?;
            println!("{}", serde_json::to_string_pretty(&res)?);
            Ok(())
        }
        Command::PhraseHistory {
            phrase,
            parquet,
            include_variants,
            timeline,
            phrase_index,
            out,
        } => {
            use crate::tools::{EngineConfig, ToolEngine};

            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index,
                tfidf_index: None,
                vector_index: None,
                catalog_index: None,
                doc_table: None,
                registry: None,
                output_root: None,
            };

            let engine = ToolEngine::open(config).await?;

            let req = crate::tools::requests::PhraseHistoryRequest {
                phrase,
                include_variants,
                timeline,
            };

            let res = engine.phrase_history_impl(req).await?;

            if let Some(out_path) = out {
                std::fs::write(out_path, serde_json::to_string_pretty(&res)?)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&res)?);
            }
            Ok(())
        }
        Command::FirstAttestation {
            phrase,
            parquet,
            limit,
            phrase_index,
            out,
        } => {
            use crate::tools::{EngineConfig, ToolEngine};

            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index,
                tfidf_index: None,
                vector_index: None,
                catalog_index: None,
                doc_table: None,
                registry: None,
                output_root: None,
            };

            let engine = ToolEngine::open(config).await?;

            let req = crate::tools::requests::FirstAttestationRequest {
                phrase,
                scope_canon: vec![],
                scope_period: vec![],
                scope_source_work_id: None,
                limit,
            };

            let res = engine.first_attestation_impl(req).await?;

            if let Some(out_path) = out {
                std::fs::write(out_path, serde_json::to_string_pretty(&res)?)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&res)?);
            }
            Ok(())
        }
        Command::PersonResolve {
            name,
            alias,
            parquet,
            out,
        } => person_resolve::run(name, alias, parquet, out).await,
        Command::PersonHistory {
            name,
            alias,
            parquet,
            limit,
            out,
        } => person_history::run(name, alias, parquet, limit, out).await,
        Command::CanonicalSource {
            phrase,
            parquet,
            canon,
            limit,
            phrase_index,
            out,
        } => {
            use crate::tools::{EngineConfig, ToolEngine};

            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index,
                tfidf_index: None,
                vector_index: None,
                catalog_index: None,
                doc_table: None,
                registry: None,
                output_root: None,
            };

            let engine = ToolEngine::open(config).await?;

            let canon_opt = if canon.is_empty() {
                None
            } else {
                Some(canon.join(","))
            };

            let req = crate::tools::requests::CanonicalSourceRequest {
                phrase,
                limit,
                canon: canon_opt,
            };

            let res = engine.canonical_source_impl(req).await?;

            if let Some(out_path) = out {
                std::fs::write(out_path, serde_json::to_string_pretty(&res)?)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&res)?);
            }
            Ok(())
        }
        Command::Timeline {
            phrase,
            parquet,
            include_variants,
            limit,
            phrase_index,
            out,
        } => timeline::run(phrase, parquet, include_variants, limit, phrase_index, out).await,
        Command::SimilarPhrase {
            phrase,
            parquet,
            index,
            limit,
            out,
        } => similar_phrase::run(phrase, parquet, index, limit, out).await,
        Command::OutlineSearch {
            parquet,
            phrase_index,
            doc_table,
            catalog,
            phrase,
            node_id,
            work_id,
            group_by,
            limit_total,
            limit_per_group,
            out,
        } => {
            outline_search::run(
                parquet,
                phrase_index,
                doc_table,
                catalog,
                phrase,
                node_id,
                work_id,
                group_by,
                limit_total,
                limit_per_group,
                out,
            )
            .await
        }
        Command::HeadingSearch {
            query,
            parquet,
            canon,
            source_work_id,
            period,
            limit,
            brief,
            out,
        } => {
            use crate::tools::{EngineConfig, ToolEngine};

            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index: None,
                tfidf_index: None,
                vector_index: None,
                catalog_index: None,
                doc_table: None,
                registry: None,
                output_root: None,
            };
            let engine = ToolEngine::open(config).await?;
            let res = engine
                .heading_search_impl(crate::tools::requests::HeadingSearchRequest {
                    query,
                    limit,
                    canon,
                    source_work_id,
                    period,
                    brief,
                })
                .await?;
            if let Some(out_path) = out {
                std::fs::write(out_path, serde_json::to_string_pretty(&res)?)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&res)?);
            }
            Ok(())
        }
        Command::ClusterHits {
            parquet,
            phrase_index,
            doc_table,
            catalog,
            phrase,
            cluster_by,
            limit_total,
            limit_per_cluster,
            out,
        } => {
            cluster_hits::run(
                parquet,
                phrase_index,
                doc_table,
                catalog,
                phrase,
                cluster_by,
                limit_total,
                limit_per_cluster,
                out,
            )
            .await
        }
        Command::AbsenceCheck {
            parquet,
            phrase_index,
            doc_table,
            catalog,
            phrase,
            scope_work_id,
            scope_canon,
            scope_period,
            scope_node_id,
            limit,
            out,
        } => {
            absence_check::run(
                parquet,
                phrase_index,
                doc_table,
                catalog,
                phrase,
                scope_work_id,
                scope_canon,
                scope_period,
                scope_node_id,
                limit,
                out,
            )
            .await
        }
        Command::CollocationSearch {
            parquet,
            phrase_index,
            doc_table,
            phrase,
            window_chars,
            gram_len,
            limit_total,
            limit_collocates,
            out,
        } => {
            collocation_search::run(
                parquet,
                phrase_index,
                doc_table,
                phrase,
                window_chars,
                gram_len,
                limit_total,
                limit_collocates,
                out,
            )
            .await
        }
        Command::CompareUsage {
            parquet,
            doc_table,
            catalog,
            scope_a_node_id,
            scope_a_work_id,
            scope_a_canon,
            scope_a_period,
            scope_b_node_id,
            scope_b_work_id,
            scope_b_canon,
            scope_b_period,
            gram_len,
            limit_passages,
            limit_terms,
            out,
        } => {
            compare_usage::run(
                parquet,
                doc_table,
                catalog,
                scope_a_node_id,
                scope_a_work_id,
                scope_a_canon,
                scope_a_period,
                scope_b_node_id,
                scope_b_work_id,
                scope_b_canon,
                scope_b_period,
                gram_len,
                limit_passages,
                limit_terms,
                out,
            )
            .await
        }
        Command::Mcp {
            transport: _,
            parquet: _,
            tfidf_index: _,
            catalog_index: _,
            registry: _,
            readonly: _,
            allow_admin_tools: _,
        } => {
            // MCP server requires rmcp dependency - commented out for now
            Err(anyhow::anyhow!(
                "MCP server requires rmcp dependency - not currently enabled"
            ))
        }
        Command::ToolsManifest {
            pack,
            format,
            include_examples,
        } => {
            tools_manifest::run(tools_manifest::ToolsManifestArgs {
                pack,
                format,
                include_examples,
            })
            .await
        }
        Command::ToolDocs { tool } => {
            let payload = crate::tools::docs::docs_payload(tool.as_deref());
            println!("{}", serde_json::to_string_pretty(&payload)?);
            Ok(())
        }
        Command::ToolCall {
            tool,
            json,
            json_file,
            pack,
            readonly,
            allow_admin_tools,
            passages_parquet,
            phrase_index,
            tfidf_index,
            vector_index,
            catalog_index,
            doc_table,
            registry,
            output_root,
        } => {
            tool_call::run(tool_call::ToolCallArgs {
                tool,
                json,
                json_file,
                pack,
                readonly,
                allow_admin_tools,
                passages_parquet,
                phrase_index,
                tfidf_index,
                vector_index,
                catalog_index,
                doc_table,
                registry,
                output_root,
            })
            .await
        }
        Command::RunTools {
            input,
            output,
            pack,
            readonly,
            allow_admin_tools,
            continue_on_error,
            jobs,
            output_root,
            passages_parquet,
            phrase_index,
            tfidf_index,
            vector_index,
            catalog_index,
            doc_table,
            registry,
        } => {
            run_tools::run(run_tools::RunToolsArgs {
                input,
                output,
                pack,
                readonly,
                allow_admin_tools,
                continue_on_error,
                jobs,
                output_root,
                passages_parquet,
                phrase_index,
                tfidf_index,
                vector_index,
                catalog_index,
                doc_table,
                registry,
            })
            .await
        }
    }
}
