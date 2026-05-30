use crate::embedding::models::LocalEmbeddingProfile;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Source corpus type for `ingest`.
#[derive(Debug, Clone, ValueEnum)]
pub enum IngestSource {
    /// CBETA Buddhist canon TEI/XML corpus (xml-p5 layout, one file per work).
    Cbeta,
    /// CBETA Buddhist canon from the official ISO distribution (xml-iso layout, one file per fascicle).
    CbetaIso,
    /// CEF (Corpus Exchange Format) JSON-lines file.
    Cef,
    /// Terebess.hu Zen biography pages (SingleFile-saved HTML).
    Terebess,
}

#[derive(Debug, Parser)]
#[command(name = "sinorag")]
#[command(version)]
#[command(about = "SinoRAGD — Buddhist corpus research engine.\n\n\
User flow:\n  \
  1. sinorag init                       # Stage 1 (fast, <20 min): corpus + all fast indexes\n  \
  2. sinorag index vector-update \\      # Stage 2 (slow, 1–2 h): semantic search\n     \
     --model bge-small-zh-v1.5\n  \
  3. sinorag status                     # see what's built / what's next\n  \
  4. sinorag tools-manifest             # discover JSON tool schemas\n  \
  5. sinorag setup opencode && \\\n     \
     sinorag agent                      # talk to the corpus from a TUI\n\n\
Agents talk to SinoRAG one of two ways:\n  \
  - JSON CLI:  `tools-manifest`, `tool-call`, `run-tools` for scripts/batches\n  \
  - MCP:       `sinorag agent` for opencode, or `sinorag mcp` for other clients\n\n\
Run `sinorag tools-manifest --include-examples` for available tools.")]
#[command(after_help = "Run `sinorag help <COMMAND>` for command details.")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Which agent to onboard via `sinorag setup`.
#[derive(Debug, Subcommand)]
pub enum SetupAgent {
    /// Verify opencode is installed and show the supported MCP wrapper flow.
    ///
    /// Does not install or modify opencode itself.
    Opencode {
        /// Explicit path to opencode (overrides PATH / $OPENCODE_BIN).
        #[arg(long)]
        opencode: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum IndexCommand {
    /// Build the phrase (n-gram) index for exact CJK phrase lookup.
    ///
    /// Required for canonical-anchor / first-attestation / phrase-history
    /// tools.
    Phrase {
        #[arg(long, default_value = "data/passages.parquet", hide = true)]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/doc_table.bin", hide = true)]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/phrase.index", hide = true)]
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
    /// Required for `similar`, `frontier`, and related tools.
    /// Not required for basic search/passage tools.
    Tfidf {
        #[arg(long, default_value = "data/passages.parquet", hide = true)]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/doc_table.bin", hide = true)]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/tfidf.index", hide = true)]
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
        #[arg(long, default_value = "data/derived/phrase.index")]
        index: PathBuf,
    },

    /// Print tf-idf-index metadata.
    TfidfInfo {
        #[arg(long, default_value = "data/derived/tfidf.index")]
        index: PathBuf,
    },

    /// Build the catalog (works / sections / outline) index.
    ///
    /// Required for `works`, `outline`, `scope`, `outline-search`, and related
    /// tools. Much faster than phrase or TF-IDF — expect a few minutes on CBETA.
    /// Safe to re-run post-hoc on an existing corpus without re-ingesting.
    Catalog {
        #[arg(long, default_value = "data/passages.parquet", hide = true)]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/doc_table.bin", hide = true)]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/catalog.index", hide = true)]
        out: PathBuf,
    },

    /// Export passage records for external embedding generation.
    VectorExport {
        #[arg(long, default_value = "data/passages.parquet", hide = true)]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/doc_table.bin", hide = true)]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/vector_input.jsonl")]
        out: PathBuf,
        #[arg(long)]
        limit: Option<usize>,
    },

    /// Build the vector index from external embedding JSONL.
    VectorBuild {
        #[arg(long, default_value = "data/derived/doc_table.bin", hide = true)]
        doc_table: PathBuf,
        #[arg(long)]
        embeddings: PathBuf,
        #[arg(long, default_value = "data/derived/vector.index")]
        out: PathBuf,
        #[arg(long)]
        model_id: String,
        #[arg(long, default_value = "unknown")]
        model_revision: String,
        #[arg(long)]
        source_fingerprint: Option<String>,
        #[arg(
            long,
            default_value = "Work: {main_title}\\nSection: {heading}\\nPeriod: {period}\\nText:\\n{text}"
        )]
        embedding_text_template: String,
        #[arg(long, default_value = "vector-export embedding_text field")]
        input_text_field_policy: String,
        #[arg(long, default_value = "external_provider_policy")]
        truncation_policy: String,
        #[arg(long)]
        max_input_chars: Option<u32>,
        #[arg(long)]
        pooling: Option<String>,
        #[arg(long)]
        instruction: Option<String>,
        #[arg(long, default_value_t = 32)]
        max_nb_connection: usize,
        #[arg(long, default_value_t = 200)]
        ef_construction: usize,
        #[arg(long, default_value_t = 16)]
        nb_layer: usize,
    },

    /// Print vector-index metadata.
    VectorInfo {
        #[arg(long, default_value = "data/derived/vector.index")]
        index: PathBuf,
    },

    /// Build a direct TensorRT engine with trtexec for local embeddings.
    VectorEngineBuild {
        /// ONNX model file to compile.
        #[arg(long)]
        onnx: PathBuf,
        /// TensorRT engine directory; writes engine.plan and build.json.
        #[arg(long, default_value = "data/derived/tensorrt/bge-small-zh-v1.5")]
        engine_dir: PathBuf,
        /// Embedding model profile.
        #[arg(long, value_enum, default_value = "bge-small-zh-v1.5")]
        model: LocalEmbeddingProfile,
        /// TensorRT optimization/max batch size.
        #[arg(long)]
        batch_size: Option<usize>,
        /// Path to trtexec.
        #[arg(long, default_value = "trtexec")]
        trtexec: PathBuf,
        /// Rebuild even when engine.plan and build.json already exist.
        #[arg(long, default_value_t = false)]
        force: bool,
    },

    /// Embed passages with a local model and build the vector index.
    ///
    /// Build or update the vector (HNSW) index used for semantic search.
    ///
    /// Uses fastembed-rs to embed passages directly from the parquet corpus.
    /// Maintains an append-only embedding cache so only new/changed passages
    /// are re-embedded on subsequent runs.
    ///
    /// REQUIREMENTS
    ///
    /// Build flags:
    ///   cargo build --release --features tensorrt
    ///
    /// Runtime libraries:
    ///   TensorRT + CUDA libs     — visible via --tensorrt-root, SINORAG_TENSORRT_ROOT, or linker path
    ///   TensorRT engine.plan     — build explicitly before indexing
    ///
    /// Linux examples include libnvinfer.so, libnvinfer_plugin.so,
    /// libnvonnxparser.so, libcudart.so, libcublas.so, and libcublasLt.so.
    ///
    /// Environment variables:
    ///   SINORAG_TENSORRT_ROOT    — TensorRT install root (bin/ and lib/ added to PATH)
    ///
    /// Build TensorRT engines first with `index vector-engine-build`. Embedding
    /// runs load engine.plan from --tensorrt-cache-dir (default: data/derived/tensorrt/).
    VectorUpdate {
        /// Passages parquet directory.
        #[arg(long, default_value = "data/passages.parquet", hide = true)]
        parquet: PathBuf,
        /// DocumentTable path.
        #[arg(long, default_value = "data/derived/doc_table.bin", hide = true)]
        doc_table: PathBuf,
        /// Embedding model profile.
        #[arg(long, value_enum)]
        model: LocalEmbeddingProfile,
        /// Embedding cache JSONL path (defaults to derived/<model-slug>.jsonl).
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Vector index output path.
        #[arg(long, default_value = "data/derived/vector.index")]
        out: PathBuf,
        /// Embedding batch size (default: model-specific).
        #[arg(long)]
        batch_size: Option<usize>,
        /// Directory for fastembed model weight downloads.
        #[arg(long)]
        model_cache_dir: Option<PathBuf>,
        /// TensorRT install root. Defaults to SINORAG_TENSORRT_ROOT, TENSORRT_ROOT,
        /// TRT_ROOT, or D:\TensorRT on Windows when it exists.
        #[arg(long)]
        tensorrt_root: Option<PathBuf>,
        /// TensorRT engine/timing cache directory. Defaults beside the embedding
        /// cache so optimized engines are reused across runs.
        #[arg(long)]
        tensorrt_cache_dir: Option<PathBuf>,
        /// Use the explicit CPU backend instead of TensorRT.
        #[arg(long, default_value_t = false)]
        cpu: bool,
        /// Show download progress bar when fetching model weights.
        #[arg(long, default_value_t = true)]
        show_download_progress: bool,
        /// Allow building an index when doc_table contains passages absent from parquet.
        #[arg(long, default_value_t = false)]
        allow_partial_vector_index: bool,
        /// HNSW max_nb_connection (graph connectivity).
        #[arg(long, default_value_t = 32)]
        max_nb_connection: usize,
        /// HNSW ef_construction (index quality / build time trade-off).
        #[arg(long, default_value_t = 200)]
        ef_construction: usize,
        /// HNSW nb_layer.
        #[arg(long, default_value_t = 16)]
        nb_layer: usize,
    },

    /// Build the semantic vector index with the default Chinese model.
    ///
    /// Alias for: sinorag index vector-update --model bge-small-zh-v1.5
    ///
    /// Expects a direct TensorRT engine built in data/derived/tensorrt/.
    Semantic {
        /// Allow building an index when doc_table contains passages absent from parquet.
        #[arg(long, default_value_t = false)]
        allow_partial_vector_index: bool,
    },
}

#[derive(Debug, Args)]
pub struct LexicalIndexArgs {
    #[arg(long, default_value = "data/passages.parquet", hide = true)]
    pub parquet: PathBuf,
    #[arg(long, default_value = "data/derived/doc_table.bin", hide = true)]
    pub doc_table: PathBuf,
    #[arg(long, default_value = "data/derived/phrase.index", hide = true)]
    pub phrase_out: PathBuf,
    #[arg(long, default_value = "data/derived/tfidf.index", hide = true)]
    pub tfidf_out: PathBuf,
    #[arg(long, default_value_t = 4)]
    pub phrase_gram_len: usize,
    #[arg(long, default_value_t = 5)]
    pub min_ngram: usize,
    #[arg(long, default_value_t = 8)]
    pub max_ngram: usize,
    #[arg(long, default_value_t = 5)]
    pub min_df: u32,
    #[arg(long, alias = "max-df", default_value_t = 0.05)]
    pub max_df_ratio: f32,
    #[arg(long, default_value_t = 200_000)]
    pub max_features: usize,
    #[arg(long, default_value_t = 2048)]
    pub buckets: usize,
    #[arg(long)]
    pub temp_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct SemanticIndexArgs {
    #[arg(long, default_value = "data/passages.parquet", hide = true)]
    pub parquet: PathBuf,
    #[arg(long, default_value = "data/derived/doc_table.bin", hide = true)]
    pub doc_table: PathBuf,
    #[arg(long, value_enum, default_value = "bge-small-zh-v1.5")]
    pub model: LocalEmbeddingProfile,
    #[arg(long)]
    pub cache: Option<PathBuf>,
    #[arg(long, default_value = "data/derived/vector.index")]
    pub out: PathBuf,
    #[arg(long)]
    pub batch_size: Option<usize>,
    #[arg(long)]
    pub model_cache_dir: Option<PathBuf>,
    #[arg(long)]
    pub tensorrt_root: Option<PathBuf>,
    #[arg(long)]
    pub tensorrt_cache_dir: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    pub cpu: bool,
    #[arg(long, default_value_t = true)]
    pub show_download_progress: bool,
    /// Allow building an index when doc_table contains passages absent from parquet.
    #[arg(long, default_value_t = false)]
    pub allow_partial_vector_index: bool,
    #[arg(long, default_value_t = 32)]
    pub max_nb_connection: usize,
    #[arg(long, default_value_t = 200)]
    pub ef_construction: usize,
    #[arg(long, default_value_t = 16)]
    pub nb_layer: usize,
}

#[derive(Debug, Subcommand)]
pub enum IndexesCommand {
    /// Build phrase + TF-IDF lexical indexes.
    Lexical {
        #[command(flatten)]
        args: LexicalIndexArgs,
    },
    /// Build the semantic vector index.
    ///
    /// This can be much slower than lexical indexing on large corpora.
    Semantic {
        #[command(flatten)]
        args: SemanticIndexArgs,
    },
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Three-way merge of CBETA distributions into one canonical directory.
    ///
    /// Combines GitHub xml-p5, ISO xml-iso, and optionally tei-extra (CC, LC,
    /// TX, YP canons) into `<out>/xml-merged/`. For each work, the source with
    /// the most bytes wins; works unique to any source are always included.
    ///
    ///   sinorag merge-cbeta --github /path/to/xml-p5 \
    ///                       --iso    /path/to/xml-iso \
    ///                       --extra  /path/to/tei-extra \
    ///                       --out    /path/to/merged
    ///   sinorag ingest cbeta /path/to/merged
    #[command(name = "merge-cbeta")]
    MergeCbeta {
        /// Path to the GitHub xml-p5 CBETA corpus root (contains `xml-p5/`).
        #[arg(long)]
        github: PathBuf,
        /// Path to the ISO xml-iso CBETA corpus root (contains `xml-iso/`).
        #[arg(long)]
        iso: PathBuf,
        /// Path to supplementary TEI canons (CC, LC, TX, YP — typically `tei-extra/`).
        #[arg(long)]
        extra: Option<PathBuf>,
        /// Output directory. Will contain `xml-merged/` and `merge-manifest.json`.
        #[arg(long)]
        out: PathBuf,
        /// Scan and report without copying any files.
        #[arg(long)]
        dry_run: bool,
    },

    /// Bootstrap or re-index the CBETA corpus.
    ///
    /// If the corpus is not yet downloaded, fetches cbeta-pack.7z from GitHub
    /// Releases and decompresses it. Then builds all fast indexes:
    /// doc_table, catalog, phrase index, and TF-IDF index (total: under 20 min).
    ///
    /// Run this once after the corpus is available. Afterward, add semantic
    /// search with:
    ///
    ///   sinorag index vector-update --model bge-small-zh-v1.5
    ///
    /// To build from your own CBETA source files instead:
    ///
    ///   sinorag init --from-raw /path/to/cbeta
    Init {
        /// Override the download URL. Accepts https:// or file:// URLs.
        /// Useful for testing with a locally built pack.
        #[arg(long)]
        url: Option<String>,
        /// Re-initialize even if a CBETA corpus is already present.
        #[arg(long)]
        force: bool,
        /// Skip download: ingest from a local CBETA corpus directory instead.
        /// Accepts the same paths as `sinorag ingest cbeta` / `cbeta-iso`.
        #[arg(long, value_name = "PATH")]
        from_raw: Option<PathBuf>,
        #[arg(long, default_value = "data/passages.parquet", hide = true)]
        out_parquet: PathBuf,
    },

    /// Show what's been ingested and which indexes are built under the data root.
    ///
    /// Cheap filesystem inspection — safe to run any time. Useful as the first
    /// command after `init` or `ingest` to verify state and see suggested next steps.
    Status {
        /// Data root (default: data/).
        #[arg(long, default_value = "data")]
        data: PathBuf,
    },

    /// Build one index family.
    ///
    /// Prefer `indexes lexical` when both phrase and TF-IDF indexes are missing.
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },

    /// Build grouped index sets.
    Indexes {
        #[command(subcommand)]
        command: IndexesCommand,
    },

    /// Build phrase + TF-IDF lexical indexes.
    ///
    /// `optional-indexes` is kept as an alias for older scripts, but no longer
    /// builds the slow semantic vector index.
    #[command(name = "lexical-indexes", alias = "optional-indexes")]
    LexicalIndexes {
        #[command(flatten)]
        args: LexicalIndexArgs,
    },

    /// Ingest a corpus into the passage store (passages.parquet).
    ///
    /// Usage:
    ///   sinorag ingest cbeta      <PATH>   # CBETA GitHub TEI (xml-p5, one file per work)
    ///   sinorag ingest cbeta-iso  <PATH>   # CBETA ISO (xml-iso, one file per fascicle)
    ///   sinorag ingest cef        <FILE>   # CEF .jsonl file
    ///   sinorag ingest terebess   <DIR>    # Terebess HTML directory
    ///
    /// Both cbeta and cbeta-iso write to the same 'cbeta' partition.
    /// Use --resume auto to continue an interrupted run.
    Ingest {
        /// Corpus type: cbeta, cbeta-iso, cef, or terebess.
        source: IngestSource,
        /// Path to the corpus root directory or file.
        path: PathBuf,
        /// Resume from a staging dir or "auto" to pick the freshest one.
        #[arg(long)]
        resume: Option<PathBuf>,
        /// Build phrase index after ingestion.
        #[arg(long, default_value = "false")]
        build_phrase_index: bool,
        /// Build TF-IDF index after ingestion.
        #[arg(long, default_value = "false")]
        build_tfidf: bool,
        /// Phrase index output path.
        #[arg(long, default_value = "data/derived/phrase.index", hide = true)]
        phrase_index_out: PathBuf,
        /// Phrase index gram length.
        #[arg(long, default_value = "4")]
        phrase_gram_len: usize,
        /// TF-IDF output path.
        #[arg(long, default_value = "data/derived/tfidf.index", hide = true)]
        tfidf_out: Option<PathBuf>,
        #[arg(long, default_value = "data/derived/catalog.index", hide = true)]
        catalog_index_out: Option<PathBuf>,
        #[arg(long, value_parser = crate::memory::parse_memory_size, help = "Maximum memory for phrase index build (e.g. 4G, 800M; default: auto-detect)")]
        phrase_max_memory: Option<u64>,
        /// Write parquet with no compression. Useful when applying an external
        /// compressor (e.g. 7z/LZMA2). Default is ZSTD.
        #[arg(long)]
        no_parquet_compression: bool,
        // Legacy/internal passthrough fields kept for compatibility (hidden from help).
        #[arg(long, default_value = "data/passages.jsonl", hide = true)]
        out_jsonl: PathBuf,
        #[arg(long, default_value = "data/passages.parquet", hide = true)]
        out_parquet: PathBuf,
    },

    /// Ingest CBETA corpus, then build doc_table, catalog,
    /// and (by default) the phrase + TF-IDF lexical indexes in one run.
    ///
    /// Runs the corpus through a single atomic staging dir so doc_table and the
    /// catalog index are built once over the combined passage store.
    ///
    /// Use `--no-lexical` to skip both phrase and TF-IDF, `--no-phrase` /
    /// `--no-tfidf` to skip one. `--parallel-lexical` runs phrase + TF-IDF
    /// concurrently after doc_table is built — faster on multi-core boxes
    /// but ~doubles peak memory.
    #[command(name = "ingest-all")]
    IngestAll {
        /// CBETA TEI xml-p5 root directory.
        #[arg(long)]
        cbeta: PathBuf,
        /// Resume from a staging dir or "auto" to pick the freshest one.
        #[arg(long)]
        resume: Option<PathBuf>,
        /// Skip both phrase and TF-IDF index builds.
        #[arg(long)]
        no_lexical: bool,
        /// Skip the phrase index build.
        #[arg(long)]
        no_phrase: bool,
        /// Skip the TF-IDF index build.
        #[arg(long)]
        no_tfidf: bool,
        /// Build phrase + TF-IDF in parallel after doc_table. Roughly
        /// 1.5–1.7× faster but ~doubles peak memory.
        #[arg(long)]
        parallel_lexical: bool,
        /// Phrase index gram length.
        #[arg(long, default_value_t = 4)]
        phrase_gram_len: usize,
        /// Maximum memory for phrase index build (e.g. 4G, 800M;
        /// default: auto-detect).
        #[arg(long, value_parser = crate::memory::parse_memory_size)]
        phrase_max_memory: Option<u64>,
        /// Phrase index output path.
        #[arg(long, default_value = "data/derived/phrase.index", hide = true)]
        phrase_index_out: PathBuf,
        /// TF-IDF index output path.
        #[arg(long, default_value = "data/derived/tfidf.index", hide = true)]
        tfidf_out: PathBuf,
        /// Catalog index output path.
        #[arg(long, default_value = "data/derived/catalog.index", hide = true)]
        catalog_index_out: PathBuf,
        /// Write parquet with no compression. Useful when applying an external
        /// compressor (e.g. 7z/LZMA2). Default is ZSTD.
        #[arg(long)]
        no_parquet_compression: bool,
        #[arg(long, default_value = "data/passages.jsonl", hide = true)]
        out_jsonl: PathBuf,
        #[arg(long, default_value = "data/passages.parquet", hide = true)]
        out_parquet: PathBuf,
    },

    /// Start the stdio MCP server exposing the SinoRAG tool registry.
    ///
    /// This is the supported transport for MCP-capable clients. For opencode,
    /// prefer `sinorag agent`, which writes the MCP config and doctrine before
    /// launching the TUI. Other clients can spawn this command directly.
    /// All logging goes to stderr; stdout is reserved for JSON-RPC framing.
    Mcp {
        #[arg(long)]
        pack: Option<PathBuf>,
        /// Start in read-only mode. This is the default for MCP.
        #[arg(long, default_value_t = true)]
        readonly: bool,
        /// Allow tools with safety=writes_output, such as pdf-build.
        #[arg(long, default_value_t = false)]
        writable: bool,
        #[arg(long)]
        allow_admin_tools: bool,
        #[arg(long)]
        passages_parquet: Option<PathBuf>,
        #[arg(long)]
        phrase_index: Option<PathBuf>,
        #[arg(long)]
        tfidf_index: Option<PathBuf>,
        #[arg(long)]
        vector_index: Option<PathBuf>,
        #[arg(long)]
        catalog_index: Option<PathBuf>,
        #[arg(long)]
        doc_table: Option<PathBuf>,
        #[arg(long)]
        registry: Option<PathBuf>,
        #[arg(long)]
        output_root: Option<PathBuf>,
    },

    /// One-time onboarding checks for an agent SinoRAG can wrap.
    ///
    /// Currently only `opencode` has a managed wrapper. Other MCP clients can
    /// still use `sinorag mcp` directly.
    Setup {
        #[command(subcommand)]
        agent: SetupAgent,
    },

    /// Launch the opencode TUI with the SinoRAG MCP server pre-wired.
    ///
    /// Writes `<workdir>/.opencode/opencode.json` pointing opencode at
    /// `<this exe> mcp ...`, refreshes `<workdir>/AGENTS.md` from the
    /// embedded doctrine, then execs opencode. This is the recommended
    /// interactive-agent workflow; JSON CLI remains recommended for scripts
    /// and reproducible batches.
    Agent {
        /// Path to the opencode executable. Resolution order:
        /// this flag → `$OPENCODE_BIN` → `opencode` on PATH.
        #[arg(long)]
        opencode: Option<PathBuf>,

        /// Pack root passed through to the MCP server.
        #[arg(long)]
        pack: Option<PathBuf>,

        /// Pass `--allow-admin-tools` through to the MCP server.
        #[arg(long)]
        allow_admin_tools: bool,

        /// Pass `--writable` through to the MCP server so output tools can write files.
        #[arg(long, default_value_t = false)]
        writable: bool,

        /// Restrict MCP output tools to this directory.
        #[arg(long)]
        output_root: Option<PathBuf>,

        /// Write the launcher artifacts and exit without spawning
        /// opencode. Useful for inspecting the generated configuration.
        #[arg(long)]
        dry_run: bool,
    },

    /// Build the CBETA distribution pack (.7z) for GitHub Releases.
    ///
    /// Compresses passages.parquet, dict.parquet, persons.parquet, and
    /// places.parquet into a single LZMA2 .7z file using the system 7z binary.
    /// The resulting file can be distributed and unpacked by `sinorag init`.
    ///
    /// Requires 7z (p7zip-full on Linux, 7-Zip on Windows/macOS) on PATH.
    ///
    /// Required sources (run before this command):
    ///   sinorag ingest cbeta   ... --no-parquet-compression
    ///   sinorag ingest-dict    ...
    ///   sinorag ingest-authority ...
    ///
    /// Example:
    ///   sinorag pack-create --out cbeta-pack.7z
    #[command(name = "pack-create")]
    PackCreate {
        /// Data root directory containing the parquet datasets.
        #[arg(long, default_value = "data")]
        data: PathBuf,
        /// Output .7z file path.
        #[arg(long, default_value = "cbeta-pack.7z")]
        out: PathBuf,
    },

    /// Stitch already-built index artifacts into a validated pack with manifest.
    ///
    /// Validates fingerprint consistency across doc_table.bin, catalog.index,
    /// phrase.index, and tfidf.index, then writes manifest.json.
    BuildPack {
        /// Pack root directory (default: data/).
        #[arg(long, default_value = "data")]
        pack: PathBuf,
        /// Optional pack id; defaults to the root directory name.
        #[arg(long)]
        pack_id: Option<String>,
    },

    /// Fill blank author/period/main_title in passages-raw.parquet from the
    /// embedded sutra_sch.lst catalog. One-time step before pack-create.
    ///
    /// Only overwrites values that are empty or "Unknown Period" — existing
    /// non-blank data is never changed.  Run with --dry-run first to preview.
    #[command(name = "patch-metadata", hide = true)]
    PatchMetadata {
        /// Parquet directory to patch (default: data/passages-raw.parquet).
        #[arg(long, default_value = "data/passages-raw.parquet")]
        parquet: PathBuf,
        /// Preview what would change without writing any files.
        #[arg(long)]
        dry_run: bool,
    },

    // -----------------------------------------------------------------------
    // Index build commands — run by users but not shown in top-level help.
    // Use `sinorag help <command>` for details.
    // -----------------------------------------------------------------------
    /// Build TF-IDF similarity index from passage parquet.
    #[command(hide = true)]
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

    /// Show TF-IDF index metadata.
    #[command(hide = true)]
    TfidfInfo {
        #[arg(long, default_value = "data/derived/tfidf.index")]
        index: PathBuf,
    },

    /// Build phrase (n-gram) index from passage parquet.
    #[command(hide = true)]
    PhraseIndexBuild {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/doc_table.bin")]
        doc_table: PathBuf,
        #[arg(long, default_value = "data/derived/phrase.index")]
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
        #[arg(long, default_value = "data/derived/phrase.index")]
        index: PathBuf,
    },

    /// Search phrase index directly.
    ///
    /// Debug entrypoint. Agents usually invoke the JSON tool instead.
    /// Requires `sinorag index phrase` to have been run.
    #[command(hide = true)]
    PhraseIndexSearch {
        #[arg(long, default_value = "data/passages.parquet")]
        parquet: PathBuf,
        #[arg(long, default_value = "data/derived/phrase.index")]
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
    // CEF format-conversion utilities (hidden from help).
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

    /// Ingest Buddhist dictionaries into dict.parquet.
    ///
    /// Reads JSON dictionary files from the cbeta-reader dict/ directory and
    /// produces a Hive-partitioned parquet at data/dict.parquet/source={name}/.
    /// Includes Soothill-Hodous, 丁福保, 佛光, 阿含, 翻譯名義集, and others.
    #[command(name = "ingest-dict")]
    IngestDict {
        /// Path to the cbeta-reader dict/ directory.
        path: PathBuf,
        #[arg(long, default_value = "data/dict.parquet")]
        out_parquet: PathBuf,
        /// Write parquet with no compression. Useful when applying an external
        /// compressor (e.g. 7z/LZMA2). Default is ZSTD.
        #[arg(long)]
        no_parquet_compression: bool,
    },

    /// Ingest DDBC person and place authority XML into parquet.
    ///
    /// Reads Buddhist_Studies_Person_Authority.xml and Buddhist_Studies_Place_Authority.xml
    /// from the cbeta-reader dict/ directory and produces:
    ///   data/persons.parquet/   — 46k persons with names, dates, bios, relationships
    ///   data/places.parquet/    — 38k places with names, coordinates, categories
    #[command(name = "ingest-authority")]
    IngestAuthority {
        /// Path to the cbeta-reader dict/ directory containing the authority XML files.
        path: PathBuf,
        #[arg(long, default_value = "data/persons.parquet")]
        persons_out: PathBuf,
        #[arg(long, default_value = "data/places.parquet")]
        places_out: PathBuf,
        /// Write parquet with no compression. Useful when applying an external
        /// compressor (e.g. 7z/LZMA2). Default is ZSTD.
        #[arg(long)]
        no_parquet_compression: bool,
    },

    /// Ingest a CEF JSON-lines file into the passage parquet.
    #[command(hide = true)]
    IngestCef {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, default_value = "data/passages.parquet")]
        out_parquet: PathBuf,
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
    /// JSON tool. Normally invoked by agents via `sinorag tool-call`;
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
    /// JSON tool. Normally invoked by agents via `sinorag tool-call`;
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
    /// JSON tool. Normally invoked by agents via `sinorag tool-call`.
    /// Requires `sinorag index tfidf` to have been run.
    #[command(hide = true)]
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

    /// Batch TF-IDF similarity for a list of seeds.
    #[command(hide = true)]
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

    /// Find similar passages for a standalone phrase (not a passage ID).
    #[command(hide = true)]
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

    /// Generate a discovery frontier packet for an agent session.
    ///
    /// JSON tool. Normally invoked by agents via `sinorag tool-call`.
    /// Requires `sinorag index tfidf` to have been run.
    #[command(hide = true)]
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

    /// Show all unique canon codes, periods, traditions, and origins with passage counts.
    ///
    /// Use the output values as filter arguments to `search` and `works`.
    /// Example: sinorag taxonomy
    ///   → then: sinorag tool-call search --json '{"phrase":"","canon":"X","limit":5}'
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

    /// Render a Markdown file or structured report JSON to bilingual PDF.
    ///
    /// Intended as a user-side sink for model-authored dossiers: pipe a
    /// Markdown report through this command to produce a publication-ready
    /// PDF. The model can include sidecar Chinese passages by wrapping them
    /// in ```` ``` ```` fences; everything outside fences is treated as
    /// the English/translation body.
    #[command(name = "pdf-build", alias = "export-pdf")]
    ExportPdf {
        /// Markdown file produced by `report-build`, `report-from-evidence`,
        /// or hand-edited prose.
        #[arg(
            long,
            conflicts_with = "input_json",
            required_unless_present = "input_json"
        )]
        input_markdown: Option<PathBuf>,
        /// Structured JSON report/evidence artifact rendered through the
        /// built-in basic PDF template before Lopdf generation.
        #[arg(long, conflicts_with = "input_markdown")]
        input_json: Option<PathBuf>,
        /// Optional report title override for `--input-json`.
        #[arg(long)]
        title: Option<String>,
        /// Maximum essay length hint used by the basic report template.
        #[arg(long, default_value_t = 3)]
        essay_max_pages: usize,
        /// Destination PDF path. Parent directories are created if missing.
        #[arg(long)]
        out: PathBuf,
        /// Render Chinese and English in two columns per page instead of
        /// stacking them vertically.
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
    /// JSON tool. Normally invoked by agents via `sinorag tool-call`.
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
        /// Include full JSON input/output schemas. Omitted by default to keep the manifest agent-readable.
        #[arg(long, default_value_t = false)]
        include_schemas: bool,
        /// Include internal debug/forced-path tools in the manifest.
        #[arg(long, default_value_t = false)]
        include_internal: bool,
    },

    /// Print compiled-in documentation for tools.
    ToolDocs {
        #[arg(long)]
        tool: Option<String>,
    },

    /// Call a single tool with JSON arguments.
    ///
    /// Example: sinorag tool-call search --json '{"phrase":"金剛經云","limit":5}'
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
        vector_index: Option<PathBuf>,
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
    /// Example: sinorag run-tools --input jobs.jsonl --output results.jsonl
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
        vector_index: Option<PathBuf>,
        #[arg(long)]
        catalog_index: Option<PathBuf>,
        #[arg(long)]
        doc_table: Option<PathBuf>,
        #[arg(long)]
        registry: Option<PathBuf>,
    },

    /// Run fixture-based tool evaluations and write a compact pass/fail report.
    ///
    /// Fixture schema:
    /// {"schema":"sinorag-tool-eval-v1","cases":[{"id":"status","tool":"status","args":{},"expect":{"ok":true}}]}
    ToolEval {
        #[arg(long)]
        cases: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        pack: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        readonly: bool,
        #[arg(long, default_value_t = false)]
        allow_admin_tools: bool,
        #[arg(long)]
        output_root: Option<PathBuf>,
        #[arg(long)]
        passages_parquet: Option<PathBuf>,
        #[arg(long)]
        phrase_index: Option<PathBuf>,
        #[arg(long)]
        tfidf_index: Option<PathBuf>,
        #[arg(long)]
        vector_index: Option<PathBuf>,
        #[arg(long)]
        catalog_index: Option<PathBuf>,
        #[arg(long)]
        doc_table: Option<PathBuf>,
        #[arg(long)]
        registry: Option<PathBuf>,
    },
}
