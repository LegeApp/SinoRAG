pub mod build_pack;
pub mod canonical_source;
pub mod catalog_index;
pub mod document_table;
pub mod expand_context_adaptive;
pub mod find_first_mention;
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
pub mod tfidf;
pub mod timeline;
pub mod validate;

use crate::cli::{Cli, Command};
use anyhow::Result;
use serde_json::json;

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Ingest {
            corpus,
            kanripo_input,
            sorting_data_dir,
            out,
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
        } => ingest::run(
            corpus,
            kanripo_input,
            sorting_data_dir,
            out,
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
        ).await,
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
            registry,
        } => search::run(
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
            registry,
        ).await,
        Command::Passage { id, parquet, out } => passage::run(parquet, id, out).await,
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
        } => phrase_index::search(parquet, index, phrase, limit, out).await,
        Command::CatalogIndexBuild {
            parquet,
            out,
            debug_json,
            doc_table,
        } => catalog_index::build(parquet, out, debug_json, doc_table),
        Command::CatalogIndexInfo { index } => catalog_index::info(index),
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
        } => catalog_index::works(index, tradition, period, canon, author, limit),
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
        } => tfidf::similar(
            parquet,
            index,
            seed,
            limit,
            shared_ngram_limit,
            shared_phrase_limit,
            min_shared_phrase_len,
            out,
        ).await,
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
            registry,
        } => frontier::run(seed, parquet, index, limit, phrase_limit, out, registry).await,
        Command::Validate { adjudication } => validate::run(adjudication),
        Command::SeedPick {
            parquet,
            registry,
            tradition,
            period,
            limit,
        } => seed_pick::run(parquet, registry, tradition, period, limit).await,
        Command::PhraseHistory {
            phrase,
            parquet,
            include_variants,
            timeline,
            phrase_index,
            out,
        } => phrase_history::run(phrase, parquet, include_variants, timeline, phrase_index, out).await,
        Command::FirstAttestation {
            phrase,
            parquet,
            limit,
            phrase_index,
            out,
        } => first_attestation::run(phrase, parquet, limit, phrase_index, out).await,
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
        } => canonical_source::run(phrase, parquet, canon, limit, phrase_index, out).await,
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
        Command::Mcp {
            transport,
            parquet,
            tfidf_index,
            catalog_index,
            registry,
            readonly,
            allow_admin_tools,
        } => crate::mcp::server::run(transport, parquet, tfidf_index, catalog_index, registry, readonly, allow_admin_tools),
    }
}
