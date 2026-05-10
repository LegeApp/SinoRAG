use crate::catalog_index::{CorpusCatalogIndex, NodeId, OutlineNode, DocId, WorkRecord, OutlineNodeKind};
use crate::document_table::DocumentTable;
use crate::memory::{check_memory_available, recommended_thread_count};
use crate::phrase_index::parquet_files;
use crate::tfidf::ngram::char_ngrams;
use anyhow::{anyhow, Result};
use arrow::array::{Int32Array, StringArray};
use arrow::array::Array;
use arrow::record_batch::RecordBatch;
use rustc_hash::FxHashMap;
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use std::fs::File;

struct CatalogColumns<'a> {
    passage_ids: &'a StringArray,
    zh_texts: &'a StringArray,
    source_corpuses: &'a StringArray,
    source_work_ids: &'a StringArray,
    source_rel_paths: &'a StringArray,
    canons: &'a StringArray,
    canon_names: &'a StringArray,
    traditions: &'a StringArray,
    periods: &'a StringArray,
    period_ranks: &'a Int32Array,
    origins: &'a StringArray,
    authors: &'a StringArray,
    main_titles: &'a StringArray,
}

impl<'a> CatalogColumns<'a> {
    fn new(batch: &'a RecordBatch) -> Result<Self> {
        Ok(Self {
            passage_ids: string_col(batch, "passage_id")?,
            zh_texts: string_col(batch, "zh_text_normalized")?,
            source_corpuses: string_col(batch, "source_corpus")?,
            source_work_ids: string_col(batch, "source_work_id")?,
            source_rel_paths: string_col(batch, "source_rel_path")?,
            canons: string_col(batch, "canon")?,
            canon_names: string_col(batch, "canon_name")?,
            traditions: string_col(batch, "traditions")?,
            periods: string_col(batch, "period")?,
            period_ranks: int32_col(batch, "period_rank")?,
            origins: string_col(batch, "origin")?,
            authors: string_col(batch, "author")?,
            main_titles: string_col(batch, "main_title")?,
        })
    }
}

fn string_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    let idx = batch
        .schema()
        .column_with_name(name)
        .ok_or_else(|| anyhow!("Column '{name}' not found"))?
        .0;

    batch
        .column(idx)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| anyhow!("{name} column is not StringArray"))
}

fn int32_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int32Array> {
    let idx = batch
        .schema()
        .column_with_name(name)
        .ok_or_else(|| anyhow!("Column '{name}' not found"))?
        .0;

    batch
        .column(idx)
        .as_any()
        .downcast_ref::<Int32Array>()
        .ok_or_else(|| anyhow!("{name} column is not Int32Array"))
}

pub fn build(parquet_path: PathBuf, out: PathBuf, debug_json: Option<PathBuf>, doc_table_param: Option<PathBuf>) -> Result<()> {
    let files = parquet_files(&parquet_path)?;
    println!("Found {} parquet files", files.len());

    // Load DocumentTable
    let doc_table_path = if let Some(ref dt) = doc_table_param {
        dt.clone()
    } else {
        parquet_path.join("doc_table.bin")
    };
    let doc_table = if doc_table_path.exists() {
        DocumentTable::load(&doc_table_path)?
    } else {
        anyhow::bail!("DocumentTable not found at {}. Run doc-table-build first.", doc_table_path.display());
    };

    // Memory safety check: require at least 2GB available
    check_memory_available(2.0)?;

    // Estimate per-file memory usage (conservative estimate)
    // Each file scan uses ~50MB for Arrow arrays
    let per_file_gb = 0.05;
    let recommended_threads = recommended_thread_count(per_file_gb);

    // Set Rayon thread pool to recommended size to prevent OOM
    let current_threads = rayon::current_num_threads();
    if recommended_threads < current_threads {
        println!("Limiting threads from {} to {} based on available memory", current_threads, recommended_threads);
        rayon::ThreadPoolBuilder::new()
            .num_threads(recommended_threads)
            .build_global()
            .map_err(|e| anyhow::anyhow!("Failed to set thread pool: {}", e))?;
    }

    let partials: Vec<FxHashMap<String, WorkData>> = files
        .par_iter()
        .map(|file_path| scan_catalog_file(file_path))
        .collect::<Result<Vec<_>>>()?;

    let mut work_data: FxHashMap<String, WorkData> = FxHashMap::default();
    for partial in partials {
        merge_work_maps(&mut work_data, partial);
    }

    let catalog = build_catalog_from_work_data(work_data, &doc_table);

    // Write debug JSON if requested
    if let Some(debug_path) = debug_json {
        let bytes = serde_json::to_vec_pretty(&catalog)?;
        if let Some(parent) = debug_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = debug_path.with_extension(format!(
            "{}.tmp",
            debug_path.extension()
                .and_then(|s| s.to_str())
                .unwrap_or("json")
        ));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &debug_path)?;
        println!("wrote {}", debug_path.display());
    }

    catalog.save_atomic(&out)?;
    println!("wrote {}", out.display());
    println!("works {}", catalog.works.len());
    println!("nodes {}", catalog.nodes.len());

    Ok(())
}

fn scan_catalog_file(file_path: &PathBuf) -> Result<FxHashMap<String, WorkData>> {
    let file = File::open(file_path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let reader = builder.build()?;

    let mut work_data: FxHashMap<String, WorkData> = FxHashMap::default();

    for batch_result in reader {
        let batch = batch_result?;
        let cols = CatalogColumns::new(&batch)?;

        for i in 0..batch.num_rows() {
            if cols.passage_ids.is_null(i) {
                continue;
            }

            let work_id = cols.source_work_ids.value(i);
            let entry = work_data.entry(work_id.to_string()).or_insert_with(|| WorkData {
                source_corpus: cols.source_corpuses.value(i).to_string(),
                canon: cols.canons.value(i).to_string(),
                canon_name: cols.canon_names.value(i).to_string(),
                main_title: cols.main_titles.value(i).to_string(),
                author: cols.authors.value(i).to_string(),
                period: cols.periods.value(i).to_string(),
                period_rank: if cols.period_ranks.is_null(i) { 0 } else { cols.period_ranks.value(i) },
                origin: cols.origins.value(i).to_string(),
                traditions: parse_traditions(cols.traditions.value(i)),
                source_rel_paths: HashSet::new(),
                passage_count: 0,
                cjk_char_count: 0,
            });

            entry.source_rel_paths.insert(cols.source_rel_paths.value(i).to_string());
            entry.passage_count += 1;
            entry.cjk_char_count += cols.zh_texts.value(i).chars().count() as u32;
        }
    }

    Ok(work_data)
}

fn merge_work_maps(dst: &mut FxHashMap<String, WorkData>, src: FxHashMap<String, WorkData>) {
    for (work_id, incoming) in src {
        let entry = dst.entry(work_id).or_insert_with(|| WorkData {
            source_corpus: incoming.source_corpus.clone(),
            canon: incoming.canon.clone(),
            canon_name: incoming.canon_name.clone(),
            main_title: incoming.main_title.clone(),
            author: incoming.author.clone(),
            period: incoming.period.clone(),
            period_rank: incoming.period_rank,
            origin: incoming.origin.clone(),
            traditions: incoming.traditions.clone(),
            source_rel_paths: HashSet::new(),
            passage_count: 0,
            cjk_char_count: 0,
        });

        entry.source_rel_paths.extend(incoming.source_rel_paths);
        entry.passage_count += incoming.passage_count;
        entry.cjk_char_count += incoming.cjk_char_count;
    }
}

fn build_catalog_from_work_data(work_data: FxHashMap<String, WorkData>, doc_table: &DocumentTable) -> CorpusCatalogIndex {
    let mut catalog = CorpusCatalogIndex::new();
    catalog.doc_table_fingerprint = Some(doc_table.source_fingerprint.clone());
    
    let mut next_node_id: u32 = 0;

    let mut works: Vec<_> = work_data.into_iter().collect();
    works.sort_by(|a, b| a.0.cmp(&b.0));

    // Build doc_id -> work_id mapping from DocumentTable
    let mut doc_to_work: FxHashMap<DocId, String> = FxHashMap::default();
    for (passage_id, doc_id) in doc_table.passage_id_map.iter() {
        // Extract work_id from passage_id (e.g., "T/T48/T48n2005.xml#p001a01" -> "T48n2005")
        // This is a simplified approach - in practice you might want to store this in DocumentTable
        let work_id_from_pid = extract_work_id_from_passage_id(passage_id);
        if let Some(wid) = work_id_from_pid {
            doc_to_work.insert(*doc_id, wid);
        }
    }

    for (work_id, work_data) in works {
        let root_node_id = next_node_id;
        next_node_id += 1;

        let mut source_rel_paths: Vec<String> = work_data.source_rel_paths.into_iter().collect();
        source_rel_paths.sort();

        // Find first and last doc_id for this work
        let mut first_doc_id: Option<DocId> = None;
        let mut last_doc_id: Option<DocId> = None;
        
        for (&doc_id, wid) in doc_to_work.iter() {
            if wid == &work_id {
                if first_doc_id.is_none() || doc_id < first_doc_id.unwrap() {
                    first_doc_id = Some(doc_id);
                }
                if last_doc_id.is_none() || doc_id > last_doc_id.unwrap() {
                    last_doc_id = Some(doc_id);
                }
            }
        }

        // Populate doc_parent mapping for all docs in this work
        for (&doc_id, wid) in doc_to_work.iter() {
            if wid == &work_id {
                catalog.doc_parent.insert(doc_id, root_node_id);
            }
        }

        let work_record = WorkRecord {
            work_id: work_id.clone(),
            source_corpus: work_data.source_corpus.clone(),
            canon: work_data.canon.clone(),
            canon_name: work_data.canon_name.clone(),
            main_title: work_data.main_title.clone(),
            author: work_data.author.clone(),
            period: work_data.period.clone(),
            period_rank: work_data.period_rank,
            origin: work_data.origin.clone(),
            traditions: work_data.traditions.clone(),
            source_rel_paths,
            root_node: root_node_id,
            passage_count: work_data.passage_count,
            cjk_char_count: work_data.cjk_char_count,
        };

        let work_idx = catalog.works.len();
        catalog.work_id_map.insert(work_id.clone(), work_idx);
        catalog.works.push(work_record);

        catalog.nodes.push(OutlineNode {
            node_id: root_node_id,
            parent_id: None,
            children: Vec::new(),
            source_corpus: work_data.source_corpus,
            work_id: work_id.clone(),
            source_rel_path: String::new(),
            node_kind: OutlineNodeKind::Work,
            label: work_data.main_title,
            heading_path: String::new(),
            div_path: String::new(),
            first_doc_id,
            last_doc_id,
            passage_count: work_data.passage_count,
            cjk_char_count: work_data.cjk_char_count,
            from_lb: None,
            to_lb: None,
        });
    }

    catalog
}

fn extract_work_id_from_passage_id(passage_id: &str) -> Option<String> {
    // Extract work_id from passage_id like "T/T48/T48n2005.xml#p001a01" -> "T48n2005"
    // This is a simplified approach - assumes specific format
    let parts: Vec<&str> = passage_id.split('/').collect();
    if parts.len() >= 3 {
        let filename = parts[2];
        let work_id = filename.strip_suffix(".xml")?;
        let work_id = work_id.strip_suffix("#p")?;
        Some(work_id.to_string())
    } else {
        None
    }
}

pub fn info(index_path: PathBuf) -> Result<()> {
    let catalog = CorpusCatalogIndex::load(&index_path)?;
    println!("{}", serde_json::to_string_pretty(&catalog.info_payload())?);
    Ok(())
}

pub fn works(
    index_path: PathBuf,
    tradition: Option<String>,
    period: Option<String>,
    canon: Option<String>,
    author: Option<String>,
    limit: usize,
) -> Result<()> {
    let catalog = CorpusCatalogIndex::load(&index_path)?;

    let mut filtered: Vec<_> = catalog.works.iter().collect();

    if let Some(t) = tradition {
        filtered = filtered.into_iter().filter(|w| w.traditions.iter().any(|tr| tr == &t)).collect();
    }

    if let Some(p) = period {
        filtered = filtered.into_iter().filter(|w| w.period == p).collect();
    }

    if let Some(c) = canon {
        filtered = filtered.into_iter().filter(|w| w.canon == c).collect();
    }

    if let Some(a) = author {
        filtered = filtered.into_iter().filter(|w| w.author == a).collect();
    }

    filtered.truncate(limit);

    let results: Vec<serde_json::Value> = filtered.iter().map(|w| {
        serde_json::json!({
            "work_id": w.work_id,
            "main_title": w.main_title,
            "author": w.author,
            "period": w.period,
            "canon": w.canon,
            "traditions": w.traditions,
            "passage_count": w.passage_count,
        })
    }).collect();

    println!("{}", serde_json::to_string_pretty(&results)?);
    Ok(())
}

pub fn outline(
    index_path: PathBuf,
    work: Option<String>,
    node: Option<u32>,
    max_depth: usize,
) -> Result<()> {
    let catalog = CorpusCatalogIndex::load(&index_path)?;

    let root_node_id = if let Some(work_id) = work {
        catalog.get_work(&work_id).map(|w| w.root_node).ok_or_else(|| anyhow!("Work not found"))?
    } else if let Some(node_id) = node {
        node_id
    } else {
        anyhow::bail!("Must specify either --work or --node");
    };

    let tree = build_outline_tree(&catalog, root_node_id, max_depth, 0);
    println!("{}", serde_json::to_string_pretty(&tree)?);

    Ok(())
}

pub fn sections(
    index_path: PathBuf,
    work: Option<String>,
    max_depth: usize,
) -> Result<()> {
    let catalog = CorpusCatalogIndex::load(&index_path)?;

    let mut sections = Vec::new();

    if let Some(work_id) = work {
        let work = catalog.get_work(&work_id).ok_or_else(|| anyhow!("Work not found"))?;
        sections = collect_sections(&catalog, work.root_node, max_depth, 0);
    } else {
        for work in &catalog.works {
            sections.extend(collect_sections(&catalog, work.root_node, max_depth, 0));
        }
    }

    println!("{}", serde_json::to_string_pretty(&sections)?);
    Ok(())
}

pub fn scope(index_path: PathBuf, node_id: u32) -> Result<()> {
    let catalog = CorpusCatalogIndex::load(&index_path)?;

    let node = catalog.get_node(node_id).ok_or_else(|| anyhow!("Node not found"))?;

    let scope = serde_json::json!({
        "node_id": node.node_id,
        "work_id": node.work_id,
        "passage_count": node.passage_count,
        "first_doc_id": node.first_doc_id,
        "last_doc_id": node.last_doc_id,
        "filter": {
            "source_work_id": node.work_id,
            "heading_path_prefix": node.heading_path,
        }
    });

    println!("{}", serde_json::to_string_pretty(&scope)?);
    Ok(())
}

// Helper structs and functions

struct WorkData {
    source_corpus: String,
    canon: String,
    canon_name: String,
    main_title: String,
    author: String,
    period: String,
    period_rank: i32,
    origin: String,
    traditions: Vec<String>,
    source_rel_paths: HashSet<String>,
    passage_count: u32,
    cjk_char_count: u32,
}

fn parse_traditions(s: &str) -> Vec<String> {
    s.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
}

fn is_cjk(ch: char) -> bool {
    ('\u{3400}'..='\u{4dbf}').contains(&ch)
        || ('\u{4e00}'..='\u{9fff}').contains(&ch)
        || ('\u{f900}'..='\u{faff}').contains(&ch)
        || ('\u{20000}'..='\u{2a6df}').contains(&ch)
}

fn build_outline_tree(
    catalog: &CorpusCatalogIndex,
    node_id: u32,
    max_depth: usize,
    current_depth: usize,
) -> serde_json::Value {
    let node = match catalog.get_node(node_id) {
        Some(n) => n,
        None => return serde_json::json!(null),
    };

    let children = if current_depth < max_depth {
        node.children.iter().map(|&child_id| build_outline_tree(catalog, child_id, max_depth, current_depth + 1)).collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    serde_json::json!({
        "node_id": node.node_id,
        "node_kind": format!("{:?}", node.node_kind),
        "label": node.label,
        "passage_count": node.passage_count,
        "children": children,
    })
}

fn collect_sections(
    catalog: &CorpusCatalogIndex,
    node_id: u32,
    max_depth: usize,
    current_depth: usize,
) -> Vec<serde_json::Value> {
    let node = match catalog.get_node(node_id) {
        Some(n) => n,
        None => return Vec::new(),
    };

    let mut sections = Vec::new();

    if current_depth > 0 {
        sections.push(serde_json::json!({
            "node_id": node.node_id,
            "node_kind": format!("{:?}", node.node_kind),
            "label": node.label,
            "heading_path": node.heading_path,
            "passage_count": node.passage_count,
        }));
    }

    if current_depth < max_depth {
        for &child_id in &node.children {
            sections.extend(collect_sections(catalog, child_id, max_depth, current_depth + 1));
        }
    }

    sections
}
