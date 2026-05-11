//! Catalog index builder. Single parquet scan → flat passage rows → sorted
//! walk → nested tree of Corpus / Canon / Work / Division / PassageRange
//! nodes. doc_id is looked up from the DocumentTable so the catalog never
//! parses work_id from passage_id (the latent v2 bug).

use crate::catalog_index::{
    CorpusCatalogIndex, DocId, NodeId, OutlineNode, OutlineNodeKind, WorkRecord,
};
use crate::document_table::DocumentTable;
use crate::phrase_index::parquet_files;
use anyhow::{anyhow, Context, Result};
use arrow::array::{Array, Int32Array, StringArray};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// PassageRow — what the builder needs per passage. One row per parquet record
// that resolves to a doc_id in the DocumentTable.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct PassageRow {
    doc_id: DocId,
    source_corpus: String,
    canon: String,
    canon_name: String,
    source_work_id: String,
    source_rel_path: String,
    div_path: String,
    heading: String,
    main_title: String,
    author: String,
    period: String,
    period_rank: i32,
    origin: String,
    traditions: Vec<String>,
    cjk_char_count: u32,
    from_lb: String,
    to_lb: String,
}

// ---------------------------------------------------------------------------
// Build entry point
// ---------------------------------------------------------------------------

pub fn build(
    parquet_path: PathBuf,
    out: PathBuf,
    debug_json: Option<PathBuf>,
    doc_table_param: Option<PathBuf>,
) -> Result<()> {
    let doc_table_path = doc_table_param
        .ok_or_else(|| anyhow!("--doc-table is required (path to derived/doc_table.bin)"))?;
    if !doc_table_path.exists() {
        anyhow::bail!("DocumentTable not found at {}", doc_table_path.display());
    }
    let doc_table = DocumentTable::load(&doc_table_path)?;
    let dt_fp = doc_table.source_fingerprint.clone();

    let files = parquet_files(&parquet_path)?;
    println!("Found {} parquet files", files.len());

    println!("[1/3] scanning parquet…");
    let partials: Vec<Vec<PassageRow>> = files
        .par_iter()
        .map(|p| scan_file(p, &doc_table))
        .collect::<Result<Vec<_>>>()?;
    let mut rows: Vec<PassageRow> = partials.into_iter().flatten().collect();
    println!("[1/3] {} passage rows", rows.len());

    println!("[2/3] sorting by (corpus, canon, work, source_rel_path, doc_id)…");
    rows.sort_by(|a, b| {
        a.source_corpus.cmp(&b.source_corpus)
            .then_with(|| a.canon.cmp(&b.canon))
            .then_with(|| a.source_work_id.cmp(&b.source_work_id))
            .then_with(|| a.source_rel_path.cmp(&b.source_rel_path))
            .then_with(|| a.doc_id.cmp(&b.doc_id))
    });

    println!("[3/3] building tree…");
    let catalog = build_catalog_from_passages(&rows, dt_fp);
    println!("      works {}, nodes {}, doc_parent entries {}",
        catalog.works.len(), catalog.nodes.len(), catalog.doc_parent.len());

    if let Some(debug_path) = debug_json {
        let bytes = serde_json::to_vec_pretty(&catalog)?;
        if let Some(parent) = debug_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&debug_path, bytes)?;
        println!("wrote {}", debug_path.display());
    }
    catalog.save_atomic(&out)?;
    println!("wrote {}", out.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-file scan
// ---------------------------------------------------------------------------

fn scan_file(file_path: &PathBuf, doc_table: &DocumentTable) -> Result<Vec<PassageRow>> {
    let file = File::open(file_path)
        .with_context(|| format!("open {}", file_path.display()))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let reader = builder.build()?;

    let mut rows: Vec<PassageRow> = Vec::new();
    for batch in reader {
        let batch = batch?;
        let cols = CatalogColumns::bind(&batch)?;
        for i in 0..batch.num_rows() {
            if cols.passage_ids.is_null(i) { continue; }
            let pid = cols.passage_ids.value(i);
            let Some(&doc_id) = doc_table.passage_id_map.get(pid) else { continue };

            let zh = cols.zh_texts.value(i);
            let cjk_chars = zh.chars().filter(|c| is_cjk(*c)).count() as u32;

            rows.push(PassageRow {
                doc_id,
                source_corpus:   cols.source_corpuses.value(i).to_string(),
                canon:           cols.canons.value(i).to_string(),
                canon_name:      cols.canon_names.value(i).to_string(),
                source_work_id:  cols.source_work_ids.value(i).to_string(),
                source_rel_path: cols.source_rel_paths.value(i).to_string(),
                div_path:        cols.div_paths.value(i).to_string(),
                heading:         cols.headings.value(i).to_string(),
                main_title:      cols.main_titles.value(i).to_string(),
                author:          cols.authors.value(i).to_string(),
                period:          cols.periods.value(i).to_string(),
                period_rank:     if cols.period_ranks.is_null(i) { 0 } else { cols.period_ranks.value(i) },
                origin:          cols.origins.value(i).to_string(),
                traditions:      parse_traditions(cols.traditions.value(i)),
                cjk_char_count:  cjk_chars,
                from_lb:         opt_str(cols.from_lbs, i),
                to_lb:           opt_str(cols.to_lbs, i),
            });
        }
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Tree construction
// ---------------------------------------------------------------------------

fn build_catalog_from_passages(rows: &[PassageRow], dt_fp: String) -> CorpusCatalogIndex {
    let mut catalog = CorpusCatalogIndex::new();
    catalog.doc_table_fingerprint = Some(dt_fp);

    // Per-group walk. Rows are already sorted (corpus, canon, work, source_rel_path, doc_id).
    // Walk them, opening/closing nodes as group-keys change. Maintain a stack
    // of (kind, key, node_id) so aggregation can roll up at close.
    let mut idx = 0usize;
    while idx < rows.len() {
        let corpus = &rows[idx].source_corpus;
        let corpus_end = idx + count_run(&rows[idx..], |r| r.source_corpus == *corpus);
        let corpus_node = catalog.push_node(OutlineNode::leaf(
            OutlineNodeKind::Corpus,
            None,
            corpus.clone(),
            corpus.clone(),
            String::new(),
            String::new(),
        ));

        let mut j = idx;
        while j < corpus_end {
            let canon = &rows[j].canon;
            let canon_end = j + count_run(&rows[j..corpus_end], |r| r.canon == *canon);
            let canon_label = if rows[j].canon_name.is_empty() {
                if canon.is_empty() { "(no canon)".to_string() } else { canon.clone() }
            } else { rows[j].canon_name.clone() };
            let canon_node = catalog.push_node(OutlineNode::leaf(
                OutlineNodeKind::Canon,
                Some(corpus_node),
                rows[j].source_corpus.clone(),
                String::new(),
                String::new(),
                canon_label,
            ));
            catalog.add_child(corpus_node, canon_node);

            let mut k = j;
            while k < canon_end {
                let work_id = &rows[k].source_work_id;
                let work_end = k + count_run(&rows[k..canon_end], |r| r.source_work_id == *work_id);
                let work_label = if rows[k].main_title.is_empty() {
                    work_id.clone()
                } else { rows[k].main_title.clone() };
                let work_node = catalog.push_node(OutlineNode::leaf(
                    OutlineNodeKind::Work,
                    Some(canon_node),
                    rows[k].source_corpus.clone(),
                    work_id.clone(),
                    String::new(),
                    work_label,
                ));
                catalog.add_child(canon_node, work_node);

                // Record the WorkRecord (one per work_id).
                let traditions = rows[k].traditions.clone();
                let work_idx = catalog.works.len();
                catalog.work_id_map.insert(work_id.clone(), work_idx);

                // Collect rel-paths the work spans.
                let mut rel_paths: HashSet<String> = HashSet::new();
                for r in &rows[k..work_end] {
                    if !r.source_rel_path.is_empty() {
                        rel_paths.insert(r.source_rel_path.clone());
                    }
                }
                let mut rel_paths: Vec<String> = rel_paths.into_iter().collect();
                rel_paths.sort();

                // Build division subtree under each source_rel_path group, then
                // group by div_path within. Doc_ids in `rows` for this work are
                // already in doc_id order.
                let mut l = k;
                while l < work_end {
                    let rel = &rows[l].source_rel_path;
                    let rel_end = l + count_run(&rows[l..work_end], |r| r.source_rel_path == *rel);
                    build_div_subtree(&mut catalog, work_node, &rows[l..rel_end]);
                    l = rel_end;
                }

                // Aggregate counts from descendants up onto the work node.
                let (first, last, p_count, c_count) = aggregate_work(&rows[k..work_end]);
                let work_node_mut = &mut catalog.nodes[work_node as usize];
                work_node_mut.first_doc_id = first;
                work_node_mut.last_doc_id = last;
                work_node_mut.passage_count = p_count;
                work_node_mut.cjk_char_count = c_count;

                catalog.works.push(WorkRecord {
                    work_id: work_id.clone(),
                    source_corpus: rows[k].source_corpus.clone(),
                    canon: rows[k].canon.clone(),
                    canon_name: rows[k].canon_name.clone(),
                    main_title: rows[k].main_title.clone(),
                    author: rows[k].author.clone(),
                    period: rows[k].period.clone(),
                    period_rank: rows[k].period_rank,
                    origin: rows[k].origin.clone(),
                    traditions,
                    source_rel_paths: rel_paths,
                    root_node: work_node,
                    passage_count: p_count,
                    cjk_char_count: c_count,
                });

                k = work_end;
            }
            // Aggregate canon-level totals
            let (first, last, p_count, c_count) = aggregate_canon(&rows[j..canon_end]);
            let canon_node_mut = &mut catalog.nodes[canon_node as usize];
            canon_node_mut.first_doc_id = first;
            canon_node_mut.last_doc_id = last;
            canon_node_mut.passage_count = p_count;
            canon_node_mut.cjk_char_count = c_count;
            j = canon_end;
        }
        let (first, last, p_count, c_count) = aggregate_canon(&rows[idx..corpus_end]);
        let corpus_node_mut = &mut catalog.nodes[corpus_node as usize];
        corpus_node_mut.first_doc_id = first;
        corpus_node_mut.last_doc_id = last;
        corpus_node_mut.passage_count = p_count;
        corpus_node_mut.cjk_char_count = c_count;
        idx = corpus_end;
    }

    catalog
}

/// Within a (corpus, canon, work, source_rel_path) group, build a Division
/// subtree keyed off `div_path`. Components of `div_path` are split on " / ".
/// Contiguous runs of doc_ids sharing the same full div_path become
/// PassageRange leaves; doc_parent points each doc at its leaf.
fn build_div_subtree(catalog: &mut CorpusCatalogIndex, parent: NodeId, rows: &[PassageRow]) {
    if rows.is_empty() { return; }
    // Cache of `joined_path -> node_id` so sibling divisions share nodes
    // when a passage falls back into a previously-opened branch.
    let mut path_nodes: FxHashMap<String, NodeId> = FxHashMap::default();

    let rel_path_node_label = rows[0].source_rel_path.clone();
    // A subtree root specifically for this source_rel_path so a single
    // multi-file work doesn't flatten everything into a single div tree.
    let file_node = catalog.push_node(OutlineNode::leaf(
        OutlineNodeKind::Division,
        Some(parent),
        rows[0].source_corpus.clone(),
        rows[0].source_work_id.clone(),
        rows[0].source_rel_path.clone(),
        rel_path_node_label,
    ));
    catalog.add_child(parent, file_node);
    path_nodes.insert(String::new(), file_node);

    let mut i = 0usize;
    while i < rows.len() {
        let div_path = &rows[i].div_path;
        let run_end = i + count_run(&rows[i..], |r| r.div_path == *div_path);

        // Ensure all ancestor Division nodes exist along this div_path.
        let leaf_node = ensure_path(catalog, file_node, &mut path_nodes, div_path, &rows[i]);

        // Emit a PassageRange leaf under leaf_node for this contiguous run.
        let first = rows[i].doc_id;
        let last = rows[run_end - 1].doc_id;
        let passage_count = (run_end - i) as u32;
        let cjk: u32 = rows[i..run_end].iter().map(|r| r.cjk_char_count).sum();
        let label = if rows[i].heading.is_empty() {
            div_path.clone()
        } else { rows[i].heading.clone() };

        let mut range_node = OutlineNode::leaf(
            OutlineNodeKind::PassageRange,
            Some(leaf_node),
            rows[i].source_corpus.clone(),
            rows[i].source_work_id.clone(),
            rows[i].source_rel_path.clone(),
            label,
        );
        range_node.heading_path = div_path.clone();
        range_node.div_path = div_path.clone();
        range_node.first_doc_id = Some(first);
        range_node.last_doc_id = Some(last);
        range_node.passage_count = passage_count;
        range_node.cjk_char_count = cjk;
        range_node.from_lb = if rows[i].from_lb.is_empty() { None } else { Some(rows[i].from_lb.clone()) };
        range_node.to_lb   = if rows[run_end-1].to_lb.is_empty() { None } else { Some(rows[run_end-1].to_lb.clone()) };
        let range_id = catalog.push_node(range_node);
        catalog.add_child(leaf_node, range_id);

        // Point each doc at its leaf range node.
        for r in &rows[i..run_end] {
            catalog.doc_parent.insert(r.doc_id, range_id);
        }

        // Roll up doc range on each Division ancestor we touched.
        roll_up(catalog, range_id, first, last, passage_count, cjk);

        i = run_end;
    }
}

/// Walk `div_path` components, creating Division nodes as needed under
/// `file_node` (the source_rel_path subtree root). Returns the deepest
/// Division node id.
fn ensure_path(
    catalog: &mut CorpusCatalogIndex,
    file_node: NodeId,
    path_nodes: &mut FxHashMap<String, NodeId>,
    div_path: &str,
    sample_row: &PassageRow,
) -> NodeId {
    if div_path.is_empty() { return file_node; }
    let mut cur = file_node;
    let mut joined = String::new();
    for seg in div_path.split(" / ") {
        let seg = seg.trim();
        if seg.is_empty() { continue; }
        if !joined.is_empty() { joined.push_str(" / "); }
        joined.push_str(seg);
        if let Some(&existing) = path_nodes.get(&joined) {
            cur = existing;
            continue;
        }
        let mut node = OutlineNode::leaf(
            OutlineNodeKind::Division,
            Some(cur),
            sample_row.source_corpus.clone(),
            sample_row.source_work_id.clone(),
            sample_row.source_rel_path.clone(),
            seg.to_string(),
        );
        node.heading_path = joined.clone();
        node.div_path = joined.clone();
        let id = catalog.push_node(node);
        catalog.add_child(cur, id);
        path_nodes.insert(joined.clone(), id);
        cur = id;
    }
    cur
}

/// Roll first_doc_id/last_doc_id/passage_count/cjk_char_count from `start_node`
/// up to the root, expanding existing aggregates.
fn roll_up(
    catalog: &mut CorpusCatalogIndex,
    start_node: NodeId,
    first: DocId,
    last: DocId,
    passage_count: u32,
    cjk: u32,
) {
    let mut cur = catalog.nodes[start_node as usize].parent_id;
    while let Some(id) = cur {
        let node = &mut catalog.nodes[id as usize];
        node.first_doc_id = match node.first_doc_id {
            Some(v) => Some(v.min(first)),
            None => Some(first),
        };
        node.last_doc_id = match node.last_doc_id {
            Some(v) => Some(v.max(last)),
            None => Some(last),
        };
        node.passage_count += passage_count;
        node.cjk_char_count += cjk;
        cur = node.parent_id;
    }
}

// ---------------------------------------------------------------------------
// Aggregation helpers
// ---------------------------------------------------------------------------

fn aggregate_work(rows: &[PassageRow]) -> (Option<DocId>, Option<DocId>, u32, u32) {
    if rows.is_empty() { return (None, None, 0, 0); }
    let first = rows.iter().map(|r| r.doc_id).min();
    let last  = rows.iter().map(|r| r.doc_id).max();
    let passage_count = rows.len() as u32;
    let cjk: u32 = rows.iter().map(|r| r.cjk_char_count).sum();
    (first, last, passage_count, cjk)
}
fn aggregate_canon(rows: &[PassageRow]) -> (Option<DocId>, Option<DocId>, u32, u32) {
    aggregate_work(rows)
}

fn count_run<F: Fn(&PassageRow) -> bool>(rows: &[PassageRow], pred: F) -> usize {
    let mut n = 0;
    while n < rows.len() && pred(&rows[n]) { n += 1; }
    n
}

// ---------------------------------------------------------------------------
// Parquet column binding
// ---------------------------------------------------------------------------

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
    div_paths: &'a StringArray,
    headings: &'a StringArray,
    from_lbs: &'a StringArray,
    to_lbs: &'a StringArray,
}

impl<'a> CatalogColumns<'a> {
    fn bind(batch: &'a RecordBatch) -> Result<Self> {
        Ok(Self {
            passage_ids:      str_col(batch, "passage_id")?,
            zh_texts:         str_col(batch, "zh_text_normalized")?,
            source_corpuses:  str_col(batch, "source_corpus")?,
            source_work_ids:  str_col(batch, "source_work_id")?,
            source_rel_paths: str_col(batch, "source_rel_path")?,
            canons:           str_col(batch, "canon")?,
            canon_names:      str_col(batch, "canon_name")?,
            traditions:       str_col(batch, "traditions")?,
            periods:          str_col(batch, "period")?,
            period_ranks:     i32_col(batch, "period_rank")?,
            origins:          str_col(batch, "origin")?,
            authors:          str_col(batch, "author")?,
            main_titles:      str_col(batch, "main_title")?,
            div_paths:        str_col(batch, "div_path")?,
            headings:         str_col(batch, "heading")?,
            from_lbs:         str_col(batch, "from_lb")?,
            to_lbs:           str_col(batch, "to_lb")?,
        })
    }
}

fn str_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    let idx = batch.schema().column_with_name(name)
        .ok_or_else(|| anyhow!("Column '{name}' not found"))?.0;
    batch.column(idx).as_any().downcast_ref::<StringArray>()
        .ok_or_else(|| anyhow!("{name} is not StringArray"))
}
fn i32_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int32Array> {
    let idx = batch.schema().column_with_name(name)
        .ok_or_else(|| anyhow!("Column '{name}' not found"))?.0;
    batch.column(idx).as_any().downcast_ref::<Int32Array>()
        .ok_or_else(|| anyhow!("{name} is not Int32Array"))
}
fn opt_str(arr: &StringArray, i: usize) -> String {
    if arr.is_null(i) { String::new() } else { arr.value(i).to_string() }
}

// ---------------------------------------------------------------------------
// Misc
// ---------------------------------------------------------------------------

fn is_cjk(ch: char) -> bool {
    ('\u{3400}'..='\u{4dbf}').contains(&ch)
        || ('\u{4e00}'..='\u{9fff}').contains(&ch)
        || ('\u{f900}'..='\u{faff}').contains(&ch)
        || ('\u{20000}'..='\u{2a6df}').contains(&ch)
}

fn parse_traditions(s: &str) -> Vec<String> {
    s.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
}

// ---------------------------------------------------------------------------
// Outline / sections / scope / works / info — unchanged semantics
// ---------------------------------------------------------------------------

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
        filtered.retain(|w| w.traditions.iter().any(|tr| tr == &t));
    }
    if let Some(p) = period { filtered.retain(|w| w.period == p); }
    if let Some(c) = canon { filtered.retain(|w| w.canon == c); }
    if let Some(a) = author { filtered.retain(|w| w.author == a); }
    filtered.truncate(limit);
    let results: Vec<serde_json::Value> = filtered.iter().map(|w| {
        serde_json::json!({
            "work_id": w.work_id, "main_title": w.main_title, "author": w.author,
            "period": w.period, "canon": w.canon, "traditions": w.traditions,
            "passage_count": w.passage_count,
        })
    }).collect();
    println!("{}", serde_json::to_string_pretty(&results)?);
    Ok(())
}

pub fn outline(index_path: PathBuf, work: Option<String>, node: Option<u32>, max_depth: usize) -> Result<()> {
    let catalog = CorpusCatalogIndex::load(&index_path)?;
    let root_node_id = if let Some(work_id) = work {
        catalog.get_work(&work_id).map(|w| w.root_node)
            .ok_or_else(|| anyhow!("Work not found"))?
    } else if let Some(n) = node { n } else {
        anyhow::bail!("Must specify either --work or --node");
    };
    let tree = build_outline_tree(&catalog, root_node_id, max_depth, 0);
    println!("{}", serde_json::to_string_pretty(&tree)?);
    Ok(())
}

pub fn sections(index_path: PathBuf, work: Option<String>, max_depth: usize) -> Result<()> {
    let catalog = CorpusCatalogIndex::load(&index_path)?;
    let mut out = Vec::new();
    if let Some(work_id) = work {
        let w = catalog.get_work(&work_id).ok_or_else(|| anyhow!("Work not found"))?;
        out = collect_sections(&catalog, w.root_node, max_depth, 0);
    } else {
        for w in &catalog.works {
            out.extend(collect_sections(&catalog, w.root_node, max_depth, 0));
        }
    }
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

pub fn scope(index_path: PathBuf, node_id: u32) -> Result<()> {
    let catalog = CorpusCatalogIndex::load(&index_path)?;
    let node = catalog.get_node(node_id).ok_or_else(|| anyhow!("Node not found"))?;
    let scope = serde_json::json!({
        "node_id": node.node_id,
        "node_kind": format!("{:?}", node.node_kind),
        "label": node.label,
        "work_id": node.work_id,
        "passage_count": node.passage_count,
        "first_doc_id": node.first_doc_id,
        "last_doc_id": node.last_doc_id,
        "heading_path": node.heading_path,
        "source_rel_path": node.source_rel_path,
    });
    println!("{}", serde_json::to_string_pretty(&scope)?);
    Ok(())
}

fn build_outline_tree(catalog: &CorpusCatalogIndex, node_id: u32, max_depth: usize, depth: usize) -> serde_json::Value {
    let node = match catalog.get_node(node_id) {
        Some(n) => n, None => return serde_json::json!(null),
    };
    let children = if depth < max_depth {
        node.children.iter().map(|&c| build_outline_tree(catalog, c, max_depth, depth + 1)).collect::<Vec<_>>()
    } else { Vec::new() };
    serde_json::json!({
        "node_id": node.node_id,
        "node_kind": format!("{:?}", node.node_kind),
        "label": node.label,
        "passage_count": node.passage_count,
        "first_doc_id": node.first_doc_id,
        "last_doc_id": node.last_doc_id,
        "children": children,
    })
}

fn collect_sections(catalog: &CorpusCatalogIndex, node_id: u32, max_depth: usize, depth: usize) -> Vec<serde_json::Value> {
    let node = match catalog.get_node(node_id) {
        Some(n) => n, None => return Vec::new(),
    };
    let mut out = Vec::new();
    if depth > 0 {
        out.push(serde_json::json!({
            "node_id": node.node_id,
            "node_kind": format!("{:?}", node.node_kind),
            "label": node.label,
            "heading_path": node.heading_path,
            "passage_count": node.passage_count,
            "first_doc_id": node.first_doc_id,
            "last_doc_id": node.last_doc_id,
        }));
    }
    if depth < max_depth {
        for &c in &node.children {
            out.extend(collect_sections(catalog, c, max_depth, depth + 1));
        }
    }
    out
}
