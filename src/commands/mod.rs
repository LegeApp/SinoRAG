pub mod absence_check;
pub mod build_pack;
pub mod canonical_source;
pub mod catalog_index;
pub mod cluster_hits;
pub mod collocation_search;
pub mod compare_usage;
pub mod document_table;
pub mod estimate;
pub mod expand_context_adaptive;
pub mod find_first_mention;
pub mod outline_search;
pub mod query_expand_terms;
pub mod trace_term_usage;
pub mod expand_context;
pub mod cef;
pub mod export;
pub mod first_attestation;
pub mod frontier;
pub mod ingest;
pub mod ingest_terebess;
pub mod kanripo;
pub mod passage;
pub mod person_history;
pub mod person_resolve;
pub mod phrase_history;
pub mod phrase_index;
pub mod research_packet;
pub mod search;
pub mod seed_pick;
pub mod similar_phrase;
pub mod status;
pub mod tfidf;
pub mod timeline;
pub mod validate;
pub mod tools_manifest;
pub mod tool_call;
pub mod run_tools;

use crate::cli::{Cli, Command, IndexCommand};
use anyhow::Result;
use serde_json::json;

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Status { data } => {
            use crate::tools::{ToolEngine, EngineConfig};
            
            let config = EngineConfig {
                pack: Some(data),
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: None,
                phrase_index: None,
                tfidf_index: None,
                catalog_index: None,
                doc_table: None,
                registry: None,
                output_root: None,
            };
            
            let engine = ToolEngine::open(config).await?;
            
            let res = engine.status_impl().await?;
            println!("{}", serde_json::to_string_pretty(&res)?);
            Ok(())
        },
        Command::Index { command } => match command {
            IndexCommand::Phrase { parquet, doc_table, out, gram_len, buckets, temp_dir } =>
                phrase_index::build(parquet, doc_table, out, gram_len, buckets, temp_dir),
            IndexCommand::Tfidf {
                parquet, doc_table, out,
                min_ngram, max_ngram, min_df, max_df_ratio, max_features,
                buckets, temp_dir,
            } => {
                let params = crate::tfidf::index::TfidfParams {
                    min_ngram, max_ngram, min_df, max_df_ratio, max_features,
                    dtype: "float32".to_string(),
                    analyzer: "char".to_string(),
                };
                crate::tfidf::index::build(parquet, doc_table, out, params, buckets, temp_dir)
            }
            IndexCommand::PhraseInfo { index } => phrase_index::info(index),
            IndexCommand::TfidfInfo  { index } => tfidf::info(index),
        },
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
                    return cef::ingest(path, out_parquet).await;
                }
                IngestSource::Terebess => {
                    let images_dir = std::path::PathBuf::from("data/derived/terebess_images");
                    return ingest_terebess::run(path, out_parquet, images_dir, 500);
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
            ).await
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
        Command::IngestCef { input, out_parquet } => cef::ingest(input, out_parquet).await,
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
            use crate::tools::{ToolEngine, EngineConfig};
            
            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index: None,
                tfidf_index: None,
                catalog_index: None,
                doc_table: None,
                registry: None,
                output_root: None,
            };
            
            let engine = ToolEngine::open(config).await?;
            
            let canon_opt = if canon.is_empty() { None } else { Some(canon.join(",")) };
            let tradition_opt = if tradition.is_empty() { None } else { Some(tradition.join(",")) };
            let period_opt = if period.is_empty() { None } else { Some(period.join(",")) };
            let origin_opt = if origin.is_empty() { None } else { Some(origin.join(",")) };
            
            let req = crate::tools::requests::SearchRequest {
                phrase: phrase.unwrap_or_default(),
                limit,
                canon: canon_opt,
                source_work_id,
                tradition: tradition_opt,
                period: period_opt,
                origin: origin_opt,
                author,
                title,
            };
            
            let res = engine.search_impl(req).await?;
            
            if let Some(out_path) = out {
                std::fs::write(out_path, serde_json::to_string_pretty(&res)?)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&res)?);
            }
            Ok(())
        },
        Command::Passage { id, parquet, out } => {
            use crate::tools::{ToolEngine, EngineConfig};
            
            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index: None,
                tfidf_index: None,
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
        },
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
            use crate::tools::{ToolEngine, EngineConfig};
            
            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index: Some(index),
                tfidf_index: None,
                catalog_index: None,
                doc_table: None,
                registry: None,
                output_root: None,
            };
            
            let engine = ToolEngine::open(config).await?;
            
            let req = crate::tools::requests::PhraseIndexSearchRequest {
                phrase,
                limit,
            };
            
            let res = engine.phrase_index_search_impl(req).await?;
            
            if let Some(out_path) = out {
                std::fs::write(out_path, serde_json::to_string_pretty(&res)?)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&res)?);
            }
            Ok(())
        },
        Command::CatalogIndexBuild {
            parquet,
            out,
            debug_json,
            doc_table,
        } => catalog_index::build(parquet, out, debug_json, doc_table),
        Command::CatalogIndexInfo { index } => {
            use crate::tools::{ToolEngine, EngineConfig};
            
            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: None,
                phrase_index: None,
                tfidf_index: None,
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
        },
        Command::DocTableBuild { parquet, out, append_to } => document_table::build(parquet, out, append_to),
        Command::IngestTerebess { input, out_parquet, images_dir, min_body_chars } =>
            ingest_terebess::run(input, out_parquet, images_dir, min_body_chars),
        Command::BuildPack { pack, pack_id } => build_pack::run(pack, pack_id),
        Command::ExpandContextAdaptive { parquet, catalog, passage_id, max_chars, out } =>
            expand_context_adaptive::run(parquet, catalog, passage_id, max_chars, out).await,
        Command::FindFirstMention {
            parquet, phrase_index, doc_table, phrase,
            scope_canon, scope_period, scope_source_work_id, limit, out
        } => find_first_mention::run(
            parquet, phrase_index, doc_table, phrase,
            scope_canon, scope_period, scope_source_work_id, limit, out,
        ).await,
        Command::TraceTermUsage {
            parquet, phrase_index, doc_table, phrase,
            group_by, limit_total, limit_per_group, out
        } => trace_term_usage::run(
            parquet, phrase_index, doc_table, phrase,
            group_by, limit_total, limit_per_group, out,
        ).await,
        Command::QueryExpandTerms { phrase, mode, person_alias, max, out } =>
            query_expand_terms::run(phrase, mode, person_alias, max, out),
        Command::ResearchPacketBuild {
            pack, out, recipe, brief, keep_temp,
            topic, notes, phrase, seed_passage, person, person_alias,
            work, canon, period,
        } => research_packet::build(
            pack, out, recipe, brief, keep_temp,
            topic, notes, phrase, seed_passage, person, person_alias,
            work, canon, period,
        ).await,
        Command::Works {
            index,
            tradition,
            period,
            canon,
            author,
            limit,
        } => {
            use crate::tools::{ToolEngine, EngineConfig};
            
            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: None,
                phrase_index: None,
                tfidf_index: None,
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
        },
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
        Command::Scope {
            index,
            node,
        } => catalog_index::scope(index, node),
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
            use crate::tools::{ToolEngine, EngineConfig};
            
            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index: None,
                tfidf_index: Some(index),
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
        },
        Command::SimilarBatch {
            parquet,
            index,
            seeds,
            limit,
            shared_ngram_limit,
            shared_phrase_limit,
            min_shared_phrase_len,
            out,
        } => tfidf::similar_batch(
            parquet,
            index,
            seeds,
            limit,
            shared_ngram_limit,
            shared_phrase_limit,
            min_shared_phrase_len,
            out,
        ).await,
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
            use crate::tools::{ToolEngine, EngineConfig};
            
            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index: None,
                tfidf_index: Some(index),
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
        },
        Command::Validate { adjudication } => validate::run(adjudication),
        Command::SeedPick {
            parquet,
            registry,
            tradition,
            period,
            limit,
        } => {
            use crate::tools::{ToolEngine, EngineConfig};
            
            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index: None,
                tfidf_index: None,
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
        },
        Command::PhraseHistory {
            phrase,
            parquet,
            include_variants,
            timeline,
            phrase_index,
            out,
        } => {
            use crate::tools::{ToolEngine, EngineConfig};
            
            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index,
                tfidf_index: None,
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
        },
        Command::FirstAttestation {
            phrase,
            parquet,
            limit,
            phrase_index,
            out,
        } => {
            use crate::tools::{ToolEngine, EngineConfig};
            
            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index,
                tfidf_index: None,
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
        },
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
            use crate::tools::{ToolEngine, EngineConfig};
            
            let config = EngineConfig {
                pack: None,
                readonly: true,
                allow_admin_tools: false,
                max_heavy_concurrency: 1,
                passages_parquet: Some(parquet),
                phrase_index,
                tfidf_index: None,
                catalog_index: None,
                doc_table: None,
                registry: None,
                output_root: None,
            };
            
            let engine = ToolEngine::open(config).await?;
            
            let canon_opt = if canon.is_empty() { None } else { Some(canon.join(",")) };
            
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
        },
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
            parquet, phrase_index, doc_table, catalog,
            phrase, node_id, work_id, group_by,
            limit_total, limit_per_group, out
        } => outline_search::run(
            parquet, phrase_index, doc_table, catalog,
            phrase, node_id, work_id, group_by,
            limit_total, limit_per_group, out,
        ).await,
        Command::ClusterHits {
            parquet, phrase_index, doc_table, catalog,
            phrase, cluster_by, limit_total, limit_per_cluster, out
        } => cluster_hits::run(
            parquet, phrase_index, doc_table, catalog,
            phrase, cluster_by, limit_total, limit_per_cluster, out,
        ).await,
        Command::AbsenceCheck {
            parquet, phrase_index, doc_table, catalog,
            phrase, scope_work_id, scope_canon, scope_period,
            scope_node_id, limit, out
        } => absence_check::run(
            parquet, phrase_index, doc_table, catalog,
            phrase, scope_work_id, scope_canon, scope_period,
            scope_node_id, limit, out,
        ).await,
        Command::CollocationSearch {
            parquet, phrase_index, doc_table,
            phrase, window_chars, gram_len,
            limit_total, limit_collocates, out
        } => collocation_search::run(
            parquet, phrase_index, doc_table,
            phrase, window_chars, gram_len,
            limit_total, limit_collocates, out,
        ).await,
        Command::CompareUsage {
            parquet, doc_table, catalog,
            scope_a_node_id, scope_a_work_id, scope_a_canon, scope_a_period,
            scope_b_node_id, scope_b_work_id, scope_b_canon, scope_b_period,
            gram_len, limit_passages, limit_terms, out
        } => compare_usage::run(
            parquet, doc_table, catalog,
            scope_a_node_id, scope_a_work_id, scope_a_canon, scope_a_period,
            scope_b_node_id, scope_b_work_id, scope_b_canon, scope_b_period,
            gram_len, limit_passages, limit_terms, out,
        ).await,
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
            Err(anyhow::anyhow!("MCP server requires rmcp dependency - not currently enabled"))
        }
        Command::ToolsManifest { pack, format, include_examples } => {
            tools_manifest::run(tools_manifest::ToolsManifestArgs {
                pack,
                format,
                include_examples,
            }).await
        }
        Command::ToolCall { tool, json, json_file, pack, readonly, allow_admin_tools } => {
            tool_call::run(tool_call::ToolCallArgs {
                tool,
                json,
                json_file,
                pack,
                readonly,
                allow_admin_tools,
            }).await
        }
        Command::RunTools { input, output, pack, readonly, allow_admin_tools, continue_on_error, jobs, output_root } => {
            run_tools::run(run_tools::RunToolsArgs {
                input,
                output,
                pack,
                readonly,
                allow_admin_tools,
                continue_on_error,
                jobs,
                output_root,
            }).await
        }
    }
}
