use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Source corpus type for `ingest`.
#[derive(Debug, Clone, ValueEnum)]
pub enum IngestSource {
    /// CBETA Buddhist canon TEI/XML corpus (xml-p5 layout).
    Cbeta,
    /// Kanripo plain-text classical Chinese repository.
    Kanripo,
    /// CEF (Corpus Exchange Format) JSON-lines file.
    Cef,
    /// Terebess.hu Zen biography pages (SingleFile-saved HTML).
    Terebess,
}

#[derive(Debug, Parser)]
#[command(name = "sinoragd")]
#[command(version)]
#[command(about = "SinoRAGD — Buddhist corpus research engine.\n\n\
User flow:\n  \
  1. sinoragd ingest <source> <path>   # build the corpus (one-time, slow)\n  \
  2. sinoragd status                   # see what's built / what's next\n  \
  3. sinoragd optional-indexes         # optional heavy indexes\n  \
  4. sinoragd tools-manifest           # discover JSON tool schemas\n\n\
Agents should use `tool-call` for one call or `run-tools` for JSONL batches.\n\
Run `sinoragd tools-manifest --include-examples` for available tools.")]
#[command(after_help = "Run `sinoragd help <COMMAND>` for command details.")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum IndexCommand {
    /// Build the phrase (n-gram) index for exact CJK phrase lookup.
    ///
    /// Required for canonical-anchor / first-attestation / phrase-history
    /// tools. Slow on large corpora — expect 1–3 hours on CBETA and
    /// multiple GB on disk. Not required for basic search/passage tools.
    Phrase {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/phrase_v3.index")]
        out: PathBuf,
        #[arg(long, default_value_t = 4)]
        gram_len: usize,
        #[arg(long, default_value_t = 2048)]
        buckets: usize,
        #[arg(long)]
        temp_dir: Option<PathBuf>,
    },

    /// Build the TF-IDF index for similarity / frontier discovery.
    ///
    /// Required for `similar`, `frontier`, and related tools. Slow on
    /// large corpora — expect 1–2 hours on CBETA and multiple GB on disk.
    /// Not required for basic search/passage tools.
    Tfidf {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/tfidf_v3.index")]
        out: PathBuf,
        #[arg(long, default_value_t = 5)]
        min_ngram: usize,
        #[arg(long, default_value_t = 8)]
        max_ngram: usize,
        #[arg(long, default_value_t = 5)]
        min_df: u32,
        #[arg(long, alias = "max-df", default_value_t = 0.05)]
        max_df_ratio: f32,
        #[arg(long, default_value_t = 200_000)]
        max_features: usize,
        #[arg(long, default_value_t = 2048)]
        buckets: usize,
        #[arg(long)]
        temp_dir: Option<PathBuf>,
    },

    /// Print phrase-index metadata.
    PhraseInfo {
        #[arg(long, default_value = "data/derived/phrase_v3.index")]
        index: PathBuf,
    },

    /// Print tf-idf-index metadata.
    TfidfInfo {
        #[arg(long, default_value = "data/derived/tfidf_v3.index")]
        index: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Show what's been ingested and which indexes are built under the data root.
    ///
    /// Cheap filesystem inspection — safe to run any time. Useful as the first
    /// command after `ingest` to verify state and see suggested next steps.
    Status {
        /// Data root (default: data/).
        #[arg(long, default_value = "data")]
        data: PathBuf,
    },

    /// Build one optional heavy index family.
    ///
    /// Prefer `optional-indexes` when both phrase and TF-IDF indexes are missing.
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },

    /// Build the optional phrase and TF-IDF indexes together.
    ///
    /// Run after ingest when you need exact phrase tools and similarity/frontier tools.
    OptionalIndexes {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/phrase_v3.index")]
        phrase_out: PathBuf,
        #[arg(long, default_value = "data/derived/tfidf_v3.index")]
        tfidf_out: PathBuf,
        #[arg(long, default_value_t = 4)]
        phrase_gram_len: usize,
        #[arg(long, default_value_t = 5)]
        min_ngram: usize,
        #[arg(long, default_value_t = 8)]
        max_ngram: usize,
        #[arg(long, default_value_t = 5)]
        min_df: u32,
        #[arg(long, alias = "max-df", default_value_t = 0.05)]
        max_df_ratio: f32,
        #[arg(long, default_value_t = 200_000)]
        max_features: usize,
        #[arg(long, default_value_t = 2048)]
        buckets: usize,
        #[arg(long)]
        temp_dir: Option<PathBuf>,
    },

    /// Ingest a corpus into the passage store (passages.parquet).
    ///
    /// Usage:
    ///   sinoragd ingest cbeta    <PATH>   # CBETA TEI xml-p5 root directory
    ///   sinoragd ingest kanripo  <PATH>   # Kanripo texts/ root directory
    ///   sinoragd ingest cef      <FILE>   # CEF .jsonl file
    ///   sinoragd ingest terebess <DIR>    # Terebess HTML directory
    ///
    /// Ingest appends to existing parquet partitions — running cbeta and
    /// kanripo separately is fine; they land in separate partitions.
    /// Use --resume auto to continue an interrupted run.
    Ingest {
        /// Corpus type: cbeta, kanripo, cef, or terebess.
        source: IngestSource,
        /// Path to the corpus root directory or file.
        path: PathBuf,
        /// Resume from a staging dir or "auto" to pick the freshest one.
        #[arg(long)]
        resume: Option<PathBuf>,
        /// Kanripo only: ingest Zen-lineage works only.
        #[arg(long)]
        zen_only: bool,
        /// Build phrase index after ingestion (slow: several hours on large corpora).
        #[arg(long, default_value = "false")]
        build_phrase_index: bool,
        /// Build TF-IDF index after ingestion (slow: several hours on large corpora).
        #[arg(long, default_value = "false")]
        build_tfidf: bool,
        /// Phrase index output path.
        #[arg(long, default_value = "data/derived/phrase_v3.index")]
        phrase_index_out: PathBuf,
        /// Phrase index gram length.
        #[arg(long, default_value = "4")]
        phrase_gram_len: usize,
        /// TF-IDF output path.
        #[arg(long, default_value = "data/derived/tfidf_v3.index")]
        tfidf_out: Option<PathBuf>,
        #[arg(long, default_value = "data/derived/catalog.index")]
        catalog_index_out: Option<PathBuf>,
        #[arg(long, value_parser = crate::memory::parse_memory_size, help = "Maximum memory for phrase index build (e.g. 4G, 800M; default: auto-detect)")]
        phrase_max_memory: Option<u64>,
        // Legacy/internal passthrough fields kept for compatibility (hidden from help).
        #[arg(long, default_value = "data/passages.jsonl", hide = true)]
        out_jsonl: PathBuf,
        #[arg(long, default_value = "data/passages.parquet", hide = true)]
        out_parquet: PathBuf,
    },

    /// Start the legacy MCP server (currently disabled).
    ///
    /// Use `tools-manifest`, `tool-call`, and `run-tools` for agent workflows.
    #[command(hide = true)] // Hidden until rmcp dependency is properly configured
    Mcp {
        /// Transport protocol: stdio (default) or sse.
        #[arg(long, default_value = "stdio")]
        transport: String,
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/tfidf_v3.index")]
        tfidf_index: PathBuf,
        #[arg(long, default_value = "data/derived/catalog.index")]
        catalog_index: PathBuf,
        #[arg(long)]
        registry: Option<PathBuf>,
        #[arg(long, default_value_t = true)]
        readonly: bool,
        #[arg(long)]
        allow_admin_tools: bool,
    },

    /// Stitch already-built index artifacts into a validated pack with manifest.
    ///
    /// Validates fingerprint consistency across doc_table.bin, catalog.index,
    /// phrase_v3.index, and tfidf_v3.index, then writes manifest.json.
    BuildPack {
        /// Pack root directory (default: data/).
        #[arg(long, default_value = "data")]
        pack: PathBuf,
        /// Optional pack id; defaults to the root directory name.
        #[arg(long)]
        pack_id: Option<String>,
    },

    // -----------------------------------------------------------------------
    // Index build commands — run by users but not shown in top-level help.
    // Use `sinoragd help <command>` for details.
    // -----------------------------------------------------------------------
    /// Build TF-IDF similarity index from passage parquet.
    #[command(hide = true)]
    TfidfBuild {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/tfidf_v3.index")]
        out: PathBuf,
        #[arg(long, default_value_t = 5)]
        min_ngram: usize,
        #[arg(long, default_value_t = 8)]
        max_ngram: usize,
        #[arg(long, default_value_t = 5)]
        min_df: u32,
        #[arg(long, alias = "max-df", default_value_t = 0.05)]
        max_df_ratio: f32,
        #[arg(long, default_value_t = 200_000)]
        max_features: usize,
        #[arg(long, default_value_t = 2048)]
        buckets: usize,
        #[arg(long)]
        temp_dir: Option<PathBuf>,
    },

    /// Show TF-IDF index metadata.
    #[command(hide = true)]
    TfidfInfo {
        #[arg(long, default_value = "data/derived/tfidf_v3.index")]
        index: PathBuf,
    },

    /// Build phrase (n-gram) index from passage parquet.
    #[command(hide = true)]
    PhraseIndexBuild {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/phrase_v3.index")]
        out: PathBuf,
        #[arg(long, default_value_t = 4)]
        gram_len: usize,
        #[arg(long, default_value_t = 2048)]
        buckets: usize,
        #[arg(long)]
        temp_dir: Option<PathBuf>,
    },

    /// Show phrase index metadata.
    #[command(hide = true)]
    PhraseIndexInfo {
        #[arg(long, default_value = "data/derived/phrase_v3.index")]
        index: PathBuf,
    },

    /// Search phrase index directly.
    ///
    /// Debug entrypoint. Agents usually invoke the JSON tool instead.
    /// Requires `sinoragd index phrase` to have been run.
    #[command(hide = true)]
    PhraseIndexSearch {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/phrase_v3.index")]
        index: PathBuf,
        #[arg(long)]
        phrase: String,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Build catalog (works/sections) index from passage parquet.
    #[command(hide = true)]
    CatalogIndexBuild {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/catalog.index")]
        out: PathBuf,
        #[arg(long)]
        debug_json: Option<PathBuf>,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: Option<PathBuf>,
    },

    /// Show catalog index metadata.
    #[command(hide = true)]
    CatalogIndexInfo {
        #[arg(long, default_value = "data/derived/catalog.index")]
        index: PathBuf,
    },

    /// Build document-ID table from passage parquet.
    #[command(hide = true)]
    DocTableBuild {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        out: PathBuf,
        /// Append mode: preserve existing doc_id assignments and add new ones.
        #[arg(long)]
        append_to: Option<PathBuf>,
    },

    // -----------------------------------------------------------------------
    // CEF / Kanripo format-conversion utilities (hidden from help).
    // -----------------------------------------------------------------------
    /// Validate a CEF JSON-lines file.
    #[command(hide = true)]
    CefValidate {
        #[arg(long)]
        input: PathBuf,
    },

    /// Create a skeleton CEF file.
    #[command(hide = true)]
    CefInit {
        #[arg(long)]
        out: PathBuf,
    },

    /// Print statistics for a CEF file.
    #[command(hide = true)]
    CefStats {
        #[arg(long)]
        input: PathBuf,
    },

    /// Ingest a CEF JSON-lines file into the passage parquet.
    #[command(hide = true)]
    IngestCef {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, default_value = "data/passages.parquet")]
        out_parquet: PathBuf,
    },

    /// Convert a Kanripo plain-text repository to TEI/XML.
    #[command(hide = true)]
    KanripoToTei {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, value_name = "DIR")]
        out_corpus: PathBuf,
        #[arg(long)]
        snapshot_id: Option<String>,
    },

    /// Generate a manifest JSON for a Kanripo repository.
    #[command(hide = true)]
    KanripoManifest {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },

    /// Ingest Terebess Zen biography HTML pages.
    #[command(hide = true)]
    IngestTerebess {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, default_value = "data/passages.parquet")]
        out_parquet: PathBuf,
        #[arg(long, default_value = "data/derived/terebess_images")]
        images_dir: PathBuf,
        #[arg(long, default_value_t = 500)]
        min_body_chars: usize,
    },

    // -----------------------------------------------------------------------
    // Research / analysis commands — exposed through JSON tool-call/batch.
    // Some low-level debug forms stay hidden; users generally should
    // not call them directly.
    // -----------------------------------------------------------------------
    /// Retrieve a single passage by ID.
    ///
    /// JSON tool. Normally invoked by agents via `sinoragd tool-call`;
    /// the direct CLI form is here for debugging.
    #[command(hide = true)]
    Passage {
        #[arg(long, value_name = "PASSAGE_ID")]
        id: String,
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Full-text / metadata search across the passage store.
    ///
    /// JSON tool. Normally invoked by agents via `sinoragd tool-call`;
    /// the direct CLI form is here for debugging.
    #[command(hide = true)]
    Search {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long)]
        phrase: Option<String>,
        #[arg(long)]
        tradition: Vec<String>,
        #[arg(long)]
        period: Vec<String>,
        #[arg(long)]
        origin: Vec<String>,
        #[arg(long)]
        canon: Vec<String>,
        #[arg(long)]
        author: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        source_work_id: Option<String>,
        #[arg(long)]
        heading_path_prefix: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value = "data/derived/registry.sqlite")]
        registry: PathBuf,
    },

    /// Expand context around a passage (preceding/following passages).
    #[command(hide = true)]
    ExpandContext {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, value_name = "PASSAGE_ID")]
        passage_id: Option<String>,
        #[arg(long)]
        session: Option<PathBuf>,
        #[arg(long)]
        hit: Option<String>,
        #[arg(long, default_value_t = 5)]
        before: usize,
        #[arg(long, default_value_t = 5)]
        after: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Adaptive context expansion using catalog outline boundaries.
    #[command(hide = true)]
    ExpandContextAdaptive {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/catalog.index")]
        catalog: PathBuf,
        #[arg(long)]
        passage_id: String,
        #[arg(long, default_value_t = 8000)]
        max_chars: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Find TF-IDF similar passages to a seed.
    ///
    /// JSON tool. Normally invoked by agents via `sinoragd tool-call`.
    /// Requires `sinoragd index tfidf` to have been run.
    #[command(hide = true)]
    Similar {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/tfidf_v3.index")]
        index: PathBuf,
        #[arg(long)]
        seed: String,
        #[arg(long, default_value_t = 25)]
        limit: usize,
        #[arg(long, default_value_t = 12)]
        shared_ngram_limit: usize,
        #[arg(long, default_value_t = 8)]
        shared_phrase_limit: usize,
        #[arg(long, default_value_t = 4)]
        min_shared_phrase_len: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Batch TF-IDF similarity for a list of seeds.
    #[command(hide = true)]
    SimilarBatch {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/tfidf_v3.index")]
        index: PathBuf,
        #[arg(long)]
        seeds: PathBuf,
        #[arg(long, default_value_t = 25)]
        limit: usize,
        #[arg(long, default_value_t = 12)]
        shared_ngram_limit: usize,
        #[arg(long, default_value_t = 8)]
        shared_phrase_limit: usize,
        #[arg(long, default_value_t = 4)]
        min_shared_phrase_len: usize,
        #[arg(long)]
        out: PathBuf,
    },

    /// Find similar passages for a standalone phrase (not a passage ID).
    #[command(hide = true)]
    SimilarPhrase {
        #[arg(long)]
        phrase: String,
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/tfidf_v3.index")]
        index: PathBuf,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Generate a discovery frontier packet for an agent session.
    ///
    /// JSON tool. Normally invoked by agents via `sinoragd tool-call`.
    /// Requires `sinoragd index tfidf` to have been run.
    #[command(hide = true)]
    Frontier {
        #[arg(long)]
        seed: String,
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/tfidf_v3.index")]
        index: PathBuf,
        #[arg(long)]
        corpus: Option<PathBuf>,
        #[arg(long, default_value_t = 25)]
        limit: usize,
        #[arg(long, default_value_t = 20)]
        phrase_limit: usize,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value = "data/derived/registry.sqlite")]
        registry: PathBuf,
    },

    /// Show all unique canon codes, periods, traditions, and origins with passage counts.
    ///
    /// Use the output values as filter arguments to `search` and `works`.
    /// Example: sinoragd taxonomy
    ///   → then: sinoragd tool-call search --json '{"phrase":"","canon":"X","limit":5}'
    Taxonomy {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
    },

    /// List works in the catalog, optionally filtered by tradition/period/canon/author.
    #[command(hide = true)]
    Works {
        #[arg(long, default_value = "data/derived/catalog.index")]
        index: PathBuf,
        #[arg(long)]
        tradition: Option<String>,
        #[arg(long)]
        period: Option<String>,
        #[arg(long)]
        canon: Option<String>,
        #[arg(long)]
        author: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },

    /// Show the outline tree for a work.
    #[command(hide = true)]
    Outline {
        #[arg(long, default_value = "data/derived/catalog.index")]
        index: PathBuf,
        #[arg(long)]
        work: Option<String>,
        #[arg(long)]
        node: Option<u32>,
        #[arg(long, default_value_t = 5)]
        max_depth: usize,
    },

    /// List top-level sections within a work.
    #[command(hide = true)]
    Sections {
        #[arg(long, default_value = "data/derived/catalog.index")]
        index: PathBuf,
        #[arg(long)]
        work: Option<String>,
        #[arg(long, default_value_t = 3)]
        max_depth: usize,
    },

    /// Return all passage IDs under a catalog outline node.
    #[command(hide = true)]
    Scope {
        #[arg(long, default_value = "data/derived/catalog.index")]
        index: PathBuf,
        #[arg(long)]
        node: u32,
    },

    /// Export a research packet to Markdown.
    #[command(hide = true)]
    ExportMarkdown {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        title: Option<String>,
    },

    /// Export a research packet to a ReadZen collection bundle.
    #[command(hide = true)]
    ExportReadzen {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
    },

    /// Build an evidence/timeline/lineage graph from a research packet.
    #[command(hide = true)]
    GraphBuild {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value = "evidence")]
        kind: String,
        #[arg(long)]
        name: Option<String>,
    },

    /// Assemble a multi-source report document.
    #[command(hide = true)]
    ReportBuild {
        #[arg(long, required = true)]
        input: Vec<PathBuf>,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, default_value_t = 3)]
        essay_max_pages: usize,
    },

    /// Render a Markdown file to PDF.
    #[command(hide = true)]
    ExportPdf {
        #[arg(long)]
        input_markdown: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        side_by_side: bool,
    },

    /// List prior research runs for a seed passage from the registry.
    #[command(hide = true)]
    Catalog {
        #[arg(long, default_value = "GraphDiscovery/Runs")]
        runs: PathBuf,
        #[arg(long, default_value = "data/derived/registry.sqlite")]
        registry: PathBuf,
    },

    /// Show prior agent work for a seed passage.
    #[command(hide = true)]
    PriorWork {
        #[arg(long)]
        seed: String,
        #[arg(long, default_value = "data/derived/registry.sqlite")]
        registry: PathBuf,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },

    /// Show research status for a phrase across all sessions.
    #[command(hide = true)]
    PhraseStatus {
        #[arg(long)]
        phrase: String,
        #[arg(long, default_value = "data/derived/registry.sqlite")]
        registry: PathBuf,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },

    /// Summarize work-level research activity from the registry.
    #[command(hide = true)]
    WorkSummary {
        #[arg(long, default_value = "data/derived/registry.sqlite")]
        registry: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },

    /// Validate an adjudication JSON file.
    #[command(hide = true)]
    Validate {
        #[arg(long)]
        adjudication: PathBuf,
    },

    /// Pick high-value seed passages for an agent to start from.
    ///
    /// JSON tool. Normally invoked by agents via `sinoragd tool-call`.
    #[command(hide = true)]
    SeedPick {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/registry.sqlite")]
        registry: PathBuf,
        #[arg(long)]
        tradition: Vec<String>,
        #[arg(long)]
        period: Vec<String>,
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },

    /// Trace the history and spread of a phrase across the corpus.
    #[command(hide = true)]
    PhraseHistory {
        #[arg(long)]
        phrase: String,
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long)]
        include_variants: bool,
        #[arg(long)]
        timeline: bool,
        #[arg(long)]
        phrase_index: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Find the earliest attested occurrence of a phrase.
    #[command(hide = true)]
    FirstAttestation {
        #[arg(long)]
        phrase: String,
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value_t = 200)]
        limit: usize,
        #[arg(long)]
        phrase_index: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Resolve a person's name to canonical form and known aliases.
    #[command(hide = true)]
    PersonResolve {
        #[arg(long)]
        name: String,
        #[arg(long)]
        alias: Vec<String>,
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Retrieve passages mentioning a person, ordered by period.
    #[command(hide = true)]
    PersonHistory {
        #[arg(long)]
        name: String,
        #[arg(long)]
        alias: Vec<String>,
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value_t = 200)]
        limit: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Find canonical-source passages for a phrase (sutra citations etc.).
    #[command(hide = true)]
    CanonicalSource {
        #[arg(long)]
        phrase: String,
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long)]
        canon: Vec<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        phrase_index: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Build a chronological timeline of phrase occurrences.
    #[command(hide = true)]
    Timeline {
        #[arg(long)]
        phrase: String,
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long)]
        include_variants: bool,
        #[arg(long, default_value_t = 200)]
        limit: usize,
        #[arg(long)]
        phrase_index: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Search for a phrase within a catalog outline node.
    #[command(hide = true)]
    OutlineSearch {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long)]
        phrase_index: Option<PathBuf>,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/catalog.index")]
        catalog: PathBuf,
        #[arg(long)]
        phrase: String,
        #[arg(long)]
        node_id: Option<u32>,
        #[arg(long)]
        work_id: Option<String>,
        #[arg(long, default_value = "division")]
        group_by: String,
        #[arg(long, default_value_t = 500)]
        limit_total: usize,
        #[arg(long, default_value_t = 20)]
        limit_per_group: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Search section headings and heading paths by name.
    #[command(hide = true)]
    HeadingSearch {
        #[arg(long)]
        query: String,
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long)]
        canon: Option<String>,
        #[arg(long)]
        source_work_id: Option<String>,
        #[arg(long)]
        period: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = false)]
        brief: bool,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Cluster phrase search hits by catalog outline (work/division).
    #[command(hide = true)]
    ClusterHits {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long)]
        phrase_index: Option<PathBuf>,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/catalog.index")]
        catalog: PathBuf,
        #[arg(long)]
        phrase: String,
        #[arg(long, default_value = "work")]
        cluster_by: String,
        #[arg(long, default_value_t = 500)]
        limit_total: usize,
        #[arg(long, default_value_t = 20)]
        limit_per_cluster: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Check whether a phrase is absent from a specific catalog scope.
    #[command(hide = true)]
    AbsenceCheck {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long)]
        phrase_index: Option<PathBuf>,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/catalog.index")]
        catalog: PathBuf,
        #[arg(long)]
        phrase: String,
        #[arg(long = "scope-work-id")]
        scope_work_id: Option<String>,
        #[arg(long = "scope-canon")]
        scope_canon: Option<String>,
        #[arg(long = "scope-period")]
        scope_period: Option<String>,
        #[arg(long = "scope-node-id")]
        scope_node_id: Option<u32>,
        #[arg(long, default_value_t = 200)]
        limit: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Find terms that co-occur near a seed phrase (log-odds scoring).
    #[command(hide = true)]
    CollocationSearch {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long)]
        phrase_index: Option<PathBuf>,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: PathBuf,
        #[arg(long)]
        phrase: String,
        #[arg(long, default_value_t = 20)]
        window_chars: usize,
        #[arg(long, default_value_t = 4)]
        gram_len: usize,
        #[arg(long, default_value_t = 200)]
        limit_total: usize,
        #[arg(long, default_value_t = 30)]
        limit_collocates: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Compare two catalog sub-corpora by distinctive term log-odds.
    #[command(hide = true)]
    CompareUsage {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/catalog.index")]
        catalog: PathBuf,
        #[arg(long = "scope-a-node-id")]
        scope_a_node_id: Option<u32>,
        #[arg(long = "scope-a-work-id")]
        scope_a_work_id: Option<String>,
        #[arg(long = "scope-a-canon")]
        scope_a_canon: Option<String>,
        #[arg(long = "scope-a-period")]
        scope_a_period: Option<String>,
        #[arg(long = "scope-b-node-id")]
        scope_b_node_id: Option<u32>,
        #[arg(long = "scope-b-work-id")]
        scope_b_work_id: Option<String>,
        #[arg(long = "scope-b-canon")]
        scope_b_canon: Option<String>,
        #[arg(long = "scope-b-period")]
        scope_b_period: Option<String>,
        #[arg(long, default_value_t = 4)]
        gram_len: usize,
        #[arg(long, default_value_t = 500)]
        limit_passages: usize,
        #[arg(long, default_value_t = 50)]
        limit_terms: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Earliest attestation of a phrase, ordered by period_rank.
    #[command(hide = true)]
    FindFirstMention {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long)]
        phrase_index: Option<PathBuf>,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: PathBuf,
        #[arg(long)]
        phrase: String,
        #[arg(long = "scope-canon")]
        scope_canon: Vec<String>,
        #[arg(long = "scope-period")]
        scope_period: Vec<String>,
        #[arg(long = "scope-source-work-id")]
        scope_source_work_id: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Phrase frequency aggregated by period/canon/author/work.
    #[command(hide = true)]
    TraceTermUsage {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long)]
        phrase_index: Option<PathBuf>,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: PathBuf,
        #[arg(long)]
        phrase: String,
        #[arg(long, default_value = "period")]
        group_by: String,
        #[arg(long, default_value_t = 2000)]
        limit_total: usize,
        #[arg(long, default_value_t = 5)]
        limit_per_group: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Variants / orthographic aliases for a seed phrase.
    #[command(hide = true)]
    QueryExpandTerms {
        #[arg(long)]
        phrase: String,
        #[arg(long, default_value = "all")]
        mode: String,
        #[arg(long = "person-alias")]
        person_alias: Vec<String>,
        #[arg(long, default_value_t = 10)]
        max: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Build a curated research packet zip for a downstream agent.
    #[command(hide = true)]
    ResearchPacketBuild {
        #[arg(long, default_value = "data")]
        pack: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value = "academic-default")]
        recipe: String,
        #[arg(long)]
        brief: Option<PathBuf>,
        #[arg(long)]
        keep_temp: bool,
        #[arg(long)]
        topic: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long)]
        phrase: Option<String>,
        #[arg(long)]
        seed_passage: Option<String>,
        #[arg(long)]
        person: Option<String>,
        #[arg(long = "person-alias")]
        person_alias: Vec<String>,
        #[arg(long)]
        work: Option<String>,
        #[arg(long)]
        canon: Option<String>,
        #[arg(long)]
        period: Option<String>,
    },

    // -----------------------------------------------------------------------
    // JSON batching / tool-call commands
    // -----------------------------------------------------------------------
    /// Print manifest of available tools with schemas and descriptions.
    ///
    /// Used by agents to discover available tools, their input/output schemas,
    /// required resources, and safety levels.
    ToolsManifest {
        #[arg(long)]
        pack: Option<PathBuf>,
        #[arg(long, default_value = "json")]
        format: String,
        #[arg(long, default_value_t = false)]
        include_examples: bool,
    },

    /// Print compiled-in documentation for tools.
    ToolDocs {
        #[arg(long)]
        tool: Option<String>,
    },

    /// Call a single tool with JSON arguments.
    ///
    /// Example: sinoragd tool-call search --json '{"phrase":"金剛經云","limit":5}'
    ToolCall {
        /// Tool name to call.
        tool: String,
        #[arg(long)]
        json: Option<String>,
        #[arg(long)]
        json_file: Option<PathBuf>,
        #[arg(long)]
        pack: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        readonly: bool,
        #[arg(long, default_value_t = false)]
        allow_admin_tools: bool,
        #[arg(long)]
        passages_parquet: Option<PathBuf>,
        #[arg(long)]
        phrase_index: Option<PathBuf>,
        #[arg(long)]
        tfidf_index: Option<PathBuf>,
        #[arg(long)]
        catalog_index: Option<PathBuf>,
        #[arg(long)]
        doc_table: Option<PathBuf>,
        #[arg(long)]
        registry: Option<PathBuf>,
        #[arg(long)]
        output_root: Option<PathBuf>,
    },

    /// Run a batch of tools from a JSONL file.
    ///
    /// Example: sinoragd run-tools --input jobs.jsonl --output results.jsonl
    RunTools {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        pack: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        readonly: bool,
        #[arg(long, default_value_t = false)]
        allow_admin_tools: bool,
        #[arg(long, default_value_t = true)]
        continue_on_error: bool,
        #[arg(long, default_value_t = 1)]
        jobs: usize,
        #[arg(long)]
        output_root: Option<PathBuf>,
        #[arg(long)]
        passages_parquet: Option<PathBuf>,
        #[arg(long)]
        phrase_index: Option<PathBuf>,
        #[arg(long)]
        tfidf_index: Option<PathBuf>,
        #[arg(long)]
        catalog_index: Option<PathBuf>,
        #[arg(long)]
        doc_table: Option<PathBuf>,
        #[arg(long)]
        registry: Option<PathBuf>,
    },
}
