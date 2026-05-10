# SinoRAGD

**SinoRAGD** is a local-first research tool for Chinese Buddhist and classical Chinese corpora.

It ingests TEI/XML and corpus exchange files into a searchable passage database, then builds compact indexes for exact phrase search, TF-IDF similarity, catalog browsing, and LLM-agent research workflows.

The goal is simple:

> Let an LLM agent research Chinese source texts with tools instead of guessing from memory.

SinoRAGD is built for evidence-backed workflows: exact Chinese citations, passage IDs, source metadata, search results, similarity candidates, and generated research artifacts that can be reviewed later.

---

## What it does

- Ingests CBETA-style TEI/XML into Parquet passage data
- Supports optional Kanripo and custom corpus ingestion
- Provides exact phrase search over normalized Chinese text
- Builds a document table for stable `doc_id <-> passage_id` mapping
- Builds catalog indexes for corpus/work navigation
- Builds TF-IDF indexes for similarity and reuse discovery
- Supports memory-mapped phrase index experiments for large corpora
- Exposes tools through MCP for LLM agents
- Produces structured JSON output for reports, graph generation, and downstream apps

---

## Why this exists

General LLMs are useful, but they should not be trusted to “remember” the Buddhist canon or infer textual history without evidence.

SinoRAGD gives agents a local research backend:

```text
user question
  -> agent calls SinoRAGD tools
  -> SinoRAGD searches the corpus
  -> agent receives exact passages + metadata
  -> report cites the source text
````

This is especially useful for questions like:

* “Where is the first loaded-corpus occurrence of this phrase?”
* “What passages are textually similar to this one?”
* “Where does this Chan/Zen saying appear?”
* “Which works cite or reuse this formula?”
* “What source passages support this graph edge?”

---

## Data model

SinoRAGD separates source text from derived indexes:

```text
passages.parquet      canonical passage database
doc_table.bin         stable doc_id / passage_id mapping
catalog.index         corpus/work/outline navigation
phrase_v2.index       exact phrase candidate index
tfidf.index           similarity index
registry.sqlite       optional mutable research state
```

The intended long-term shape is:

```text
one canonical text store
many compact reference indexes
no repeated full-text copies
```

---

## Supported corpora

### CBETA TEI/XML

CBETA-style TEI/XML is the primary target.

```bash
sinoragd ingest \
  --corpus /path/to/cbeta/xml-p5 \
  --out data/passages.parquet
```

### Kanripo

Kanripo support is experimental and can require a large amount of disk space.

```bash
sinoragd ingest \
  --kanripo-input /path/to/kanripo \
  --out data/passages.parquet
```

### Custom corpora

Custom Chinese corpora can be converted into the SinoRAGD Corpus Exchange Format.

```bash
sinoragd cef-init --out my-corpus
# edit corpus.toml, works.jsonl, passages.jsonl

sinoragd ingest-cef \
  --input my-corpus \
  --out data/passages.parquet
```

Minimum custom format:

```text
corpus.toml
works.jsonl
passages.jsonl
```

---

## Basic workflow

### 1. Build passage database

```bash
sinoragd ingest \
  --corpus /path/to/cbeta/xml-p5 \
  --out data/passages.parquet
```

### 2. Build document table

```bash
sinoragd doc-table-build \
  --parquet data/passages.parquet \
  --out data/doc_table.bin
```

### 3. Build catalog index

```bash
sinoragd catalog-index-build \
  --parquet data/passages.parquet \
  --out derived/catalog.index
```

### 4. Build phrase index

```bash
sinoragd phrase-index-build-v2 \
  --parquet data/passages.parquet \
  --doc-table data/doc_table.bin \
  --out derived/phrase_v2.index \
  --gram-len 4 \
  --buckets 2048 \
  --temp-dir /path/to/large/temp
```

Use a real disk-backed temp directory for large builds. Avoid `/tmp` if it is RAM-backed.

### 5. Build TF-IDF index

```bash
sinoragd tfidf-build \
  --parquet data/passages.parquet \
  --doc-table data/doc_table.bin \
  --out derived/tfidf.index \
  --min-ngram 5 \
  --max-ngram 8 \
  --min-df 5 \
  --max-features 200000
```

For very large corpora, prefer sharded builds.

---

## Search examples

### Exact phrase search

```bash
sinoragd phrase-index-search \
  --phrase "如是我聞" \
  --parquet data/passages.parquet \
  --doc-table data/doc_table.bin \
  --phrase-index derived/phrase_v2.index
```

### SQL-style passage search

```bash
sinoragd search \
  --parquet data/passages.parquet \
  --phrase "平常心是道" \
  --limit 20
```

### Similar passages

```bash
sinoragd similar \
  --parquet data/passages.parquet \
  --index derived/tfidf.index \
  --seed "T/T48/T48n2005.xml#p001" \
  --limit 20
```

---

## MCP server

SinoRAGD includes an MCP server so LLM agents can call research tools directly.

```bash
sinoragd mcp \
  --parquet data/passages.parquet \
  --doc-table data/doc_table.bin \
  --phrase-index derived/phrase_v2.index \
  --tfidf-index derived/tfidf.index \
  --catalog-index derived/catalog.index
```

The intended agent pattern is:

```text
agent receives user question
agent calls SinoRAGD tools
SinoRAGD returns structured evidence
agent writes answer/report with citations
```

---

## LLM-assisted research

SinoRAGD is not meant to replace a scholar or translator. It is meant to make an agent’s work inspectable.

Generated reports should preserve:

* exact Chinese source text
* passage IDs
* source work metadata
* search method
* confidence / review state
* candidate graph edges
* rejected or weak evidence where relevant

---

## ReadZen integration idea

SinoRAGD can generate research artifacts that other tools organize.

A clean division is:

```text
SinoRAGD generates evidence.
ReadZen organizes and displays it beside source texts.
```

Possible outputs:

```text
research_bundle/
  artifact_index.jsonl
  source_attachments.jsonl
  reports/
  graphs/
  dossiers/
```

This allows generated reports, diagrams, phrase histories, and lineage outputs to be attached to the CBETA documents and passages they cite.

---

## Performance notes

Large corpora need disk-aware indexing.

Recommendations:

* Use release builds.
* Put temp files on a fast SSD.
* Avoid RAM-backed `/tmp` for phrase or TF-IDF builds.
* Use sharded TF-IDF for very large corpora.
* Prefer `doc_id` references over repeated passage ID strings.
* Memory-mapped indexes are preferred for large read-only retrieval structures.

Build with:

```bash
cargo build --release
```

---

## Install

From source:

```bash
git clone https://github.com/yourname/sinoragd
cd sinoragd
cargo install --path .
```

Or run directly:

```bash
cargo run --release -- --help
```

---

## Status

SinoRAGD is experimental but usable.

Current focus:

* scalable phrase index v2
* memory-conscious TF-IDF indexing
* stable corpus exchange format
* MCP agent integration
* ReadZen-compatible research bundles
* better translation/glossing workflows for Classical Chinese

Expect schema and command names to change while the project stabilizes.

---

## License

AGPL-3.0

````


