# SinoRAG

**SinoRAG** is a local-first research backend for Chinese Buddhist and classical Chinese corpora.

It ingests TEI/XML, Kanripo, CEF, and HTML corpora into a searchable passage database, then builds compact indexes for exact phrase search, TF-IDF similarity, optional vector discovery, catalog browsing, and JSONL-based LLM-agent research workflows.

> Let an LLM agent research Chinese source texts with tools instead of guessing from memory.

---

## Quick start

```bash
# 1. Ingest a corpus (one-time, slow)
sinorag ingest cbeta /path/to/cbeta/xml-p5

# 2. Check what's built and what's next
sinorag status

# 3. Build optional heavy indexes when needed
sinorag optional-indexes  # phrase + TF-IDF indexes in one indexing run

# 4. Discover and call tools from an agent
sinorag tools-manifest --include-examples
sinorag tool-call search --json '{"phrase":"金剛經","limit":5}'
```

`sinorag --help` shows the full flow. Agents use `tools-manifest` to discover schemas, `tool-call` for one request, and `run-tools` for reproducible JSONL batches.

---

## What it does

- Ingests CBETA TEI/XML, Kanripo plain-text, CEF JSON-lines, and Terebess HTML corpora
- Builds a Parquet passage store partitioned by `source_corpus`
- Provides exact phrase search over normalized Chinese text (`phrase.index`)
- Builds a document table for stable `doc_id ↔ passage_id` mapping
- Builds catalog indexes for corpus/work/section navigation
- Builds TF-IDF indexes for similarity and textual reuse discovery
- Builds optional vector indexes from external embedding JSONL or local FastEmbed models for semantic discovery
- Exposes research tools through JSON schemas for LLM agents
- Produces structured JSON output for reports, graph generation, and downstream apps

---

## Supported corpora

| Source | Command | Input |
|---|---|---|
| CBETA TEI/XML | `ingest cbeta <PATH>` | CBETA root (containing `xml-p5/`) or `xml-p5/` directly |
| Kanripo | `ingest kanripo <PATH>` | Kanripo `texts/` root |
| CEF JSON-lines | `ingest cef <FILE>` | `.jsonl` file in Corpus Exchange Format |
| Terebess HTML | `ingest terebess <DIR>` | Directory of SingleFile-saved HTML pages |

Multiple corpora can be ingested into the same store — each lands in a separate `source_corpus=<name>` Parquet partition.

---

## User workflow

### Step 1 — Ingest

```bash
sinorag ingest cbeta /path/to/cbeta/xml-p5
sinorag ingest kanripo /path/to/kanripo        # optional, append
```

Use `--resume auto` to continue an interrupted run.

### Step 2 — Check status

```bash
sinorag status
```

Reports what's ingested, which indexes are present, estimated cost for the missing optional ones, and suggested next steps.

### Step 3 — Build optional indexes

These are not required for basic `search` and `passage` tools. Build them when the tools that depend on them are needed.

```bash
# Exact phrase search + similarity / frontier discovery
sinorag optional-indexes

# Include local semantic vector discovery, when built with local embeddings
sinorag optional-indexes --with-vector --embedding-model bge-small-zh-v1.5

# Incremental rebuilds remain available
sinorag index phrase
sinorag index tfidf

# Optional semantic discovery index: local cached embedding + vector build
sinorag index vector-update --model bge-small-zh-v1.5

# External embedding flow remains available for provider-managed batches
sinorag index vector-export --out data/derived/vector_input.jsonl
sinorag index vector-build --embeddings data/derived/embeddings.jsonl --model-id BAAI/bge-m3
```

Use `--temp-dir` pointing to a fast SSD for large builds. Avoid RAM-backed `/tmp`.
Local embedding commands require a binary built with `--features local-embeddings`.

### Step 4 — Use JSON tools

```bash
sinorag tools-manifest --include-examples
sinorag tool-call evidence-search --json '{"phrase":"金剛經","limit":5}'
sinorag tool-call hybrid-discover --json '{"seed_passage_id":"B/B13/B13n0079.xml#pB13p0047a0417","limit":10}'
sinorag run-tools --input jobs.jsonl --output results.jsonl
```

Agents should inspect the manifest, then submit schema-valid JSON tool calls. Batch mode writes one response envelope per input line for auditability and retry.

---

## Data model

```text
data/
  passages.parquet/
    source_corpus=cbeta/      ← partitioned by corpus
    source_corpus=kanripo/
  derived/
    doc_table.bin             stable doc_id / passage_id mapping
    catalog.index             corpus / work / outline navigation
    phrase.index           exact phrase candidate index  (optional)
    tfidf.index            similarity index              (optional)
    vector.index           semantic discovery index       (optional)
    registry.sqlite           mutable research state        (auto-created)
```

---

## Tool dependency map

Which artifacts each tool needs:

| Tool | parquet | doc_table | catalog | phrase_index | tfidf | vector | Notes |
|---|---|---|---|---|---|---|---|
| `passage` | ✓ | — | — | — | — | — | |
| `search` / `evidence-search` | ✓ | optional | optional | optional | — | — | phrase_index speeds exact match |
| `expand-context` | ✓ | — | — | — | — | — | pure parquet window |
| `expand-context-adaptive` | ✓ | ✓ | ✓ | — | — | — | catalog for node climbing |
| `find-first-mention` | ✓ | ✓ | — | optional | — | — | phrase_index greatly speeds it up |
| `trace-term-usage` | ✓ | ✓ | — | optional | — | — | |
| `phrase-history` | ✓ | ✓ | — | optional | — | — | |
| `first-attestation` | ✓ | ✓ | — | optional | — | — | |
| `outline-search` | ✓ | ✓ | ✓ | optional | — | — | |
| `cluster-hits` | ✓ | ✓ | ✓ | optional | — | — | |
| `absence-check` | ✓ | ✓ | ✓ | optional | — | — | |
| `collocation-search` | ✓ | ✓ | — | optional | — | — | no catalog needed |
| `compare-usage` / `scope-profile` | ✓ | ✓ | ✓ | — | — | — | |
| `similar` / `similar-batch` | ✓ | ✓ | — | — | ✓ | — | |
| `vector-info` / `vector-neighbors` | optional | ✓ | — | — | — | ✓ | semantic candidates, not evidence |
| `hybrid-discover` / `source-investigate` | ✓ | ✓ | optional | optional | optional | optional | orchestrates discovery tools |
| `frontier` / `research-packet` | ✓ | ✓ | optional | optional | optional | — | |
| `query-expand-terms` | — | — | — | — | — | — | zero corpus deps |

**Minimum viable agent workflow** (just `passage`, `search`, `expand-context`): ingest only, no heavy indexes required.

---

## Agent tool workflow

```bash
sinorag tools-manifest --include-examples
sinorag tool-call passage --json '{"id":"B/B13/B13n0079.xml#pB13p0047a0417"}'
sinorag run-tools --input jobs.jsonl --output results.jsonl --jobs 4
```

Agent pattern:

```text
agent receives user question
  -> agent reads tool schemas from tools-manifest
  -> agent calls SinoRAG tools with JSON
  -> SinoRAG returns exact passages + metadata
  -> agent writes answer/report with citations
```

Research tools that require indexes not yet built will return a clear error rather than silently failing. The `status` command tells you what's missing before you start.

---

## Custom corpora (CEF)

```bash
# Create a skeleton CEF directory
sinorag cef-init --out my-corpus
# Edit: corpus.toml, works.jsonl, passages.jsonl

# Validate before ingesting
sinorag cef-validate --input my-corpus

# Ingest
sinorag ingest cef my-corpus/passages.jsonl
```

---

## LLM-assisted research

SinoRAG is not meant to replace a scholar or translator — it makes an agent's work inspectable.

Generated artifacts should preserve:

- exact Chinese source text and passage IDs
- source work metadata (canon, period, author, title)
- search method and confidence
- candidate graph edges and rejected evidence

---

## ReadZen integration

```text
SinoRAG generates evidence.
ReadZen organizes and displays it beside source texts.
```

SinoRAG can export research bundles (`export-readzen`, `graph-build`, `report-build`) consumable by ReadZen to attach generated reports, phrase histories, and lineage diagrams to the CBETA passages they cite.

---

## Performance notes

- Use release builds: `cargo build --release`
- Put `--temp-dir` on a fast SSD; avoid RAM-backed `/tmp` for phrase or TF-IDF builds
- Indexes are mmap-backed — O(1) RAM at query time regardless of index size
- `doc_id` integer references are preferred over repeated passage ID strings in all index structures

---

## Install

```bash
git clone https://github.com/yourname/sinorag
cd sinorag
cargo install --path .
```

---

## Status

Experimental but usable. Expect index schemas to change while the project stabilizes. The main user-facing commands (`ingest`, `status`, `optional-indexes`, `tools-manifest`, `tool-call`, and `run-tools`) are stable.

**Note**: MCP server support has been deprecated. Use JSON Batching (`run-tools` command) for all research workflows. JSON Batching provides better reproducibility, debugging capabilities, and is better suited for academic research use cases requiring batch processing and audit trails.

---

## License

AGPL-3.0
