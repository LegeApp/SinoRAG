use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "graphdiscovery")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Ingest {
        /// CBETA TEI corpus root (the directory that contains `xml-p5/`).
        #[arg(long)]
        corpus: Option<PathBuf>,
        /// Kanripo plain-text repository clone (the directory that contains `texts/KR*/KR*/`).
        #[arg(long)]
        kanripo_input: Option<PathBuf>,
        #[arg(long)]
        sorting_data_dir: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value = "data/passages.jsonl")]
        out_jsonl: PathBuf,
        #[arg(long, default_value = "data/passages.parquet")]
        out_parquet: PathBuf,
        #[arg(long)]
        zen_only: bool,
        /// Resume from a staging dir (`<out>/.staging/ingest-...`) or "auto"
        /// to pick the freshest one. Without this flag, ingest always starts
        /// a fresh staged run.
        #[arg(long)]
        resume: Option<PathBuf>,
        /// Build phrase index after ingestion
        #[arg(long, default_value = "true")]
        build_phrase_index: bool,
        /// Build TF-IDF index after ingestion
        #[arg(long, default_value = "true")]
        build_tfidf: bool,
        /// Phrase index output path
        #[arg(long, default_value = "data/derived/phrase_v2.index")]
        phrase_index_out: PathBuf,
        /// Phrase index gram length
        #[arg(long, default_value = "4")]
        phrase_gram_len: usize,
        /// TF-IDF output path
        #[arg(long, default_value = "data/derived/tfidf")]
        tfidf_out: Option<PathBuf>,
        #[arg(long, default_value = "data/derived/catalog.index")]
        catalog_index_out: Option<PathBuf>,
        #[arg(long, value_parser = crate::memory::parse_memory_size, help = "Maximum memory to use for phrase index (e.g., 4G, 800M, default: auto-detect)")]
        phrase_max_memory: Option<u64>,
    },
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
    CefValidate {
        #[arg(long)]
        input: PathBuf,
    },
    CefInit {
        #[arg(long)]
        out: PathBuf,
    },
    CefStats {
        #[arg(long)]
        input: PathBuf,
    },
    IngestCef {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, default_value = "data/passages.parquet")]
        out_parquet: PathBuf,
    },
    KanripoToTei {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, value_name = "DIR")]
        out_corpus: PathBuf,
        #[arg(long)]
        snapshot_id: Option<String>,
    },
    KanripoManifest {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
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
    TfidfBuild {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/tfidf.index")]
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
    TfidfInfo {
        #[arg(long, default_value = "data/derived/tfidf.index")]
        index: PathBuf,
    },
    PhraseIndexBuild {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/phrase_v2.index")]
        out: PathBuf,
        #[arg(long, default_value_t = 4)]
        gram_len: usize,
        #[arg(long, default_value_t = 2048)]
        buckets: usize,
        #[arg(long)]
        temp_dir: Option<PathBuf>,
    },
    PhraseIndexInfo {
        #[arg(long, default_value = "data/derived/phrase_v2.index")]
        index: PathBuf,
    },
    PhraseIndexSearch {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/phrase_v2.index")]
        index: PathBuf,
        #[arg(long)]
        phrase: String,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    CatalogIndexBuild {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/catalog.index")]
        out: PathBuf,
        #[arg(long)]
        debug_json: Option<PathBuf>,
        #[arg(long)]
        doc_table: Option<PathBuf>,
    },
    CatalogIndexInfo {
        #[arg(long, default_value = "data/derived/catalog.index")]
        index: PathBuf,
    },
    DocTableBuild {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        out: PathBuf,
        /// Append mode: load this existing doc_table, preserve all its
        /// doc_id assignments, and append any passage_ids in `--parquet`
        /// not already present. Writes a sidecar `<out>.lineage.json`.
        #[arg(long)]
        append_to: Option<PathBuf>,
    },
    /// Ingest terebess.hu Zen biography pages (SingleFile-saved HTML).
    /// Filters 403/404 placeholders, strips site chrome, extracts body text +
    /// main image (written to images_dir), writes parquet under
    /// `source_corpus=terebess/`.
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
    /// Stitch already-built artifacts (doc_table.bin, catalog.index,
    /// phrase_v2.index, tfidf.index) into a pack: validate fingerprints,
    /// populate the registry identity tables, write manifest.json.
    BuildPack {
        /// Pack root (defaults to `data` directory layout).
        #[arg(long, default_value = "data")]
        pack: PathBuf,
        /// Optional pack id; defaults to the root directory name.
        #[arg(long)]
        pack_id: Option<String>,
    },
    /// Build a research packet: a zip of curated primary-source material
    /// (tool outputs + cited passages) for a downstream agent. SinoRAG does
    /// not write the report itself; it assembles the dossier.
    ResearchPacketBuild {
        /// Pack root that owns the corpus + indexes.
        #[arg(long, default_value = "data")]
        pack: PathBuf,
        /// Output zip path. Default: `data/research_packets/<topic>-<utc>.researchpacket.zip`.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Bundled name (`academic-default`, `phrase-focused`, `full-genealogy`)
        /// or a path to a custom recipe JSON.
        #[arg(long, default_value = "academic-default")]
        recipe: String,
        /// Path to a brief JSON. Mutually exclusive with the per-seed flags below.
        #[arg(long)]
        brief: Option<PathBuf>,
        /// Keep the staging directory for inspection.
        #[arg(long)]
        keep_temp: bool,
        // ---- brief-from-flags (used when --brief is omitted) ----
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
    Sections {
        #[arg(long, default_value = "data/derived/catalog.index")]
        index: PathBuf,
        #[arg(long)]
        work: Option<String>,
        #[arg(long, default_value_t = 3)]
        max_depth: usize,
    },
    Scope {
        #[arg(long, default_value = "data/derived/catalog.index")]
        index: PathBuf,
        #[arg(long)]
        node: u32,
    },
    ExportMarkdown {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        title: Option<String>,
    },
    ExportReadzen {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
    },
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
    ExportPdf {
        #[arg(long)]
        input_markdown: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        side_by_side: bool,
    },
    Similar {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/tfidf.index")]
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
    SimilarBatch {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/tfidf.index")]
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
    Frontier {
        #[arg(long)]
        seed: String,
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/tfidf.index")]
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
    Catalog {
        #[arg(long, default_value = "GraphDiscovery/Runs")]
        runs: PathBuf,
        #[arg(long, default_value = "data/derived/registry.sqlite")]
        registry: PathBuf,
    },
    PriorWork {
        #[arg(long)]
        seed: String,
        #[arg(long, default_value = "data/derived/registry.sqlite")]
        registry: PathBuf,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    PhraseStatus {
        #[arg(long)]
        phrase: String,
        #[arg(long, default_value = "data/derived/registry.sqlite")]
        registry: PathBuf,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    WorkSummary {
        #[arg(long, default_value = "data/derived/registry.sqlite")]
        registry: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Passage {
        #[arg(long, value_name = "PASSAGE_ID")]
        id: String,
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Validate {
        #[arg(long)]
        adjudication: PathBuf,
    },
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
    SimilarPhrase {
        #[arg(long)]
        phrase: String,
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/tfidf.index")]
        index: PathBuf,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Mcp {
        #[arg(long, default_value = "stdio")]
        transport: String,

        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,

        #[arg(long, default_value = "data/derived/tfidf.index")]
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
}
