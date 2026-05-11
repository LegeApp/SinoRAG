# SinoRAG

**SinoRAG** is a local-first research backend for Chinese Buddhist and classical Chinese corpora.

It ingests TEI/XML, Kanripo, CEF, and HTML corpora into a searchable passage database, then builds compact indexes for exact phrase search, TF-IDF similarity, catalog browsing, and LLM-agent research workflows via MCP.

> Let an LLM agent research Chinese source texts with tools instead of guessing from memory.

---

## Quick start

```bash
# 1. Ingest a corpus (one-time, slow)
sinoragd ingest cbeta /path/to/cbeta/xml-p5

# 2. Check what's built and what's next
sinoragd status

# 3. Build optional heavy indexes when needed
sinoragd index phrase   # exact CJK phrase search (hours, several GB)
sinoragd index tfidf    # similarity / frontier discovery (hours, ~1–2 GB)

# 4. Start the MCP server for agent access
sinoragd mcp
```

`sinoragd --help` shows the full 4-step flow. All research and analysis tools are exposed as MCP tools — agents call them through the MCP server rather than directly.

---

## What it does

- Ingests CBETA TEI/XML, Kanripo plain-text, CEF JSON-lines, and Terebess HTML corpora
- Builds a Parquet passage store partitioned by `source_corpus`
- Provides exact phrase search over normalized Chinese text (`phrase_v2.index`)
- Builds a document table for stable `doc_id ↔ passage_id` mapping
- Builds catalog indexes for corpus/work/section navigation
- Builds TF-IDF indexes for similarity and textual reuse discovery
- Exposes all research tools through MCP for LLM agents
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
sinoragd ingest cbeta /path/to/cbeta/xml-p5
sinoragd ingest kanripo /path/to/kanripo        # optional, append
```

Use `--out <DIR>` to set a custom data root (default: `data/`).
Use `--resume auto` to continue an interrupted run.
Use `--sorting-data-dir` for CBETA period-rank ordering.

### Step 2 — Check status

```bash
sinoragd status
```

Reports what's ingested, which indexes are present, estimated cost for the missing optional ones, and suggested next steps.

### Step 3 — Build optional indexes

These are not required to start the MCP server. Build them when the tools that depend on them are needed.

```bash
# Exact phrase search (canonical-source, first-attestation, phrase-history, …)
sinoragd index phrase

# Similarity / frontier discovery (similar, frontier, …)
sinoragd index tfidf
```

Use `--temp-dir` pointing to a fast SSD for large builds. Avoid RAM-backed `/tmp`.

### Step 4 — Start the MCP server

```bash
sinoragd mcp
```

All research and analysis tools are exposed via MCP. Agents connect here rather than calling commands directly.

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
    phrase_v2.index           exact phrase candidate index  (optional)
    tfidf.index               similarity index              (optional)
    registry.sqlite           mutable research state        (auto-created)
```

---

## Tool dependency map

Which artifacts each tool needs:

| Tool | parquet | doc_table | catalog | phrase_index | tfidf | Notes |
|---|---|---|---|---|---|---|
| `passage` | ✓ | — | — | — | — | |
| `search` | ✓ | — | — | optional | — | phrase_index speeds exact match |
| `expand-context` | ✓ | — | — | — | — | pure parquet window |
| `expand-context-adaptive` | ✓ | — | ✓ | — | — | catalog for node climbing |
| `find-first-mention` | ✓ | ✓ | — | optional | — | phrase_index greatly speeds it up |
| `trace-term-usage` | ✓ | ✓ | — | optional | — | |
| `phrase-history` | ✓ | ✓ | — | optional | — | |
| `first-attestation` | ✓ | ✓ | — | optional | — | |
| `outline-search` | ✓ | ✓ | ✓ | optional | — | |
| `cluster-hits` | ✓ | ✓ | ✓ | optional | — | |
| `absence-check` | ✓ | ✓ | ✓ | optional | — | |
| `collocation-search` | ✓ | ✓ | — | optional | — | no catalog needed |
| `compare-usage` | ✓ | ✓ | ✓ | — | — | |
| `similar` / `similar-batch` | ✓ | ✓ | — | — | ✓ | |
| `frontier` / `research-packet` | ✓ | ✓ | optional | optional | optional | |
| `query-expand-terms` | — | — | — | — | — | zero corpus deps |

**Minimum viable MCP server** (just `passage`, `search`, `expand-context`): ingest only, no heavy indexes required.

---

## MCP server

```bash
sinoragd mcp
# or with explicit paths:
sinoragd mcp \
  --parquet data/passages.parquet \
  --tfidf-index data/derived/tfidf.index \
  --catalog-index data/derived/catalog.index
```

Agent pattern:

```text
agent receives user question
  → agent calls SinoRAG MCP tools
  → SinoRAG returns exact passages + metadata
  → agent writes answer/report with citations
```

Research tools that require indexes not yet built will return a clear error rather than silently failing. The `status` command tells you what's missing before you start.

---

## Custom corpora (CEF)

```bash
# Create a skeleton CEF directory
sinoragd cef-init --out my-corpus
# Edit: corpus.toml, works.jsonl, passages.jsonl

# Validate before ingesting
sinoragd cef-validate --input my-corpus

# Ingest
sinoragd ingest cef my-corpus/passages.jsonl
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
git clone https://github.com/yourname/sinoragd
cd sinoragd
cargo install --path .
```

---

## Status

Experimental but usable. Expect index schemas and hidden command names to change while the project stabilizes. The four user-facing commands (`ingest`, `status`, `index`, `mcp`) are stable.

---

## License

AGPL-3.0
