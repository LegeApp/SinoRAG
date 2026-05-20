# Tool Usage Map & Dependency Reference

## Indexes / Artifacts

| Artifact | Path | Built by |
|---|---|---|
| `parquet` | `data/passages.parquet` | `ingest` |
| `doc_table` | `derived/doc_table.bin` | `doc-table-build` |
| `catalog` | `derived/catalog.index` | `catalog-index-build` |
| `phrase_index` | `derived/phrase_v2.index` | `phrase-index-build` |
| `tfidf` | `derived/tfidf.index` | `tfidf-build` |
| `registry` | `derived/registry.sqlite` | auto-created on first write |

---

## Per-Command Dependency Table

| Command | parquet | doc_table | catalog | phrase_index | tfidf | registry | Notes |
|---|---|---|---|---|---|---|---|
| `query-expand-terms` | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | **Zero corpus deps** — bundled tables only |
| `ingest` / `ingest-terebess` | writes | — | — | — | — | — | Produces parquet |
| `doc-table-build` | reads | writes | ✗ | ✗ | ✗ | ✗ | |
| `catalog-index-build` | reads | reads | writes | ✗ | ✗ | ✗ | |
| `phrase-index-build` | reads | reads | ✗ | writes | ✗ | ✗ | |
| `tfidf-build` | reads | reads | ✗ | ✗ | writes | ✗ | |
| `passage` | reads | ✗ | ✗ | ✗ | ✗ | ✗ | |
| `search` | reads | ✗ | ✗ | optional | ✗ | writes | |
| `expand-context` | reads | ✗ | ✗ | ✗ | ✗ | ✗ | pure parquet window |
| `expand-context-adaptive` | reads | ✗ | reads | ✗ | ✗ | ✗ | needs catalog for node climbing |
| `find-first-mention` | reads | reads | ✗ | optional | ✗ | ✗ | phrase_index speeds it up |
| `trace-term-usage` | reads | reads | ✗ | optional | ✗ | ✗ | |
| `timeline` | reads | ✗ | ✗ | optional | ✗ | writes | |
| `tfidf similar` / `similar-batch` | reads | reads | ✗ | ✗ | reads | writes | |
| `first-attestation` (MCP) | reads | reads | ✗ | optional | ✗ | ✗ | |
| `phrase-history` | reads | reads | ✗ | optional | ✗ | ✗ | |
| `outline-search` (F2) | reads | reads | reads | optional | ✗ | ✗ | |
| `cluster-hits` (F2) | reads | reads | reads | optional | ✗ | ✗ | |
| `absence-check` (F2) | reads | reads | reads | optional | ✗ | ✗ | |
| `collocation-search` (F2) | reads | reads | ✗ | optional | ✗ | ✗ | no catalog needed |
| `compare-usage` (F2) | reads | reads | reads | ✗ | ✗ | ✗ | catalog for doc range; falls back to full parquet scan on period/canon-only scope |
| `frontier` / `research-packet` | reads | reads | optional | optional | optional | writes | |

### Key insights

- `query-expand-terms` — works with **no pack files at all**
- `expand-context`, `passage`, `search` — only need **parquet**
- `find-first-mention`, `trace-term-usage`, `phrase-history` — need **parquet + doc_table**; phrase_index optional but greatly speeds them up
- All F2 tools except `collocation-search` — need **parquet + doc_table + catalog**; phrase_index optional
- `tfidf similar` — needs **parquet + doc_table + tfidf**
- `expand-context-adaptive` — needs **parquet + catalog**

---

## Folder Organization

| Folder | Purpose |
|---|---|
| `src/commands/` | CLI dispatch only — thin `run()` wrappers, one file per subcommand |
| `src/research/` | Shared corpus query logic — `mod.rs` (search/phrase helpers), `context_expand.rs` (window expansion) |
| `src/research_tools/` | Phase F2 shared types and helpers — `common`, `evidence`, `phrase`, `scopes`, `stats` |

`context_expand.rs` lives in `src/research/` because it is shared by both `commands/expand_context.rs` and `mcp/server.rs` — it is library logic, not a CLI command.

---

## TF-IDF (`tfidf similar`) — Capability Summary

Three usage points all go through `similar_passages_with_index()` in `commands/tfidf.rs`:

1. `sinorag tfidf similar` — direct CLI
2. `sinorag tfidf similar-batch` — batch JSONL output
3. MCP server — called in 3 tool handlers (frontier, seed expansion, first-attestation follow-up)

### Scoring rules applied on top of cosine

| Rule | Multiplier |
|---|---|
| Same file | ×0.60 |
| Cross file | ×1.10 |
| Short passage (< 15 chars) | ×0.70 |
| LCS shared phrase ≥ 8 chars | ×1.25 |
| LCS shared phrase ≥ 5 chars | ×1.10 |
| No shared phrase | ×0.75 |
| ≥ 3 distinctive long n-grams | ×1.15 |
| ≥ 1 long n-gram | ×1.05 |
| Cross-period | ×1.05 |

### Evidence returned per result

- `shared_ngrams` — overlapping n-gram strings (up to `shared_ngram_limit`)
- `shared_phrases` — longest common substrings (LCS) above `min_shared_phrase_len`
- `ring_hint` — `ring1_candidate` / `ring2_motif` / `ring3_weak` / downrank flags
- Full passage metadata: text, canon, period, author, traditions, heading

### Index properties

- mmap-backed, O(1) RAM regardless of file size
- Char n-gram based (configurable `min_n` / `max_n`)
- L2-normalized TF-IDF rows + smoothed IDF
- Postings-based: only docs sharing terms with seed are scored
- `doc_table_fingerprint` stored and validated at MCP startup

### Current limitations

- **No scope filtering** — always global top-K; no `--canon` / `--period` / `--node-id` filter
- **No vocabulary decode** — `shared_ngrams` output contains hashes, not original Chinese strings (index stores only xxh3 hashes)
- **Single seed only** — must supply a `passage_id` that exists in doc_table; no query-by-text mode

---

## Phase E / F2 Session Summary

### Compilation fixes
- Added `mod research_tools;` to `main.rs` (was only in `lib.rs`)
- Fixed `compare_usage` type inference — hoisted `Vec<Value>` before `json!()` macro
- Extracted `collect_scope_terms()` so `compare-usage` handles canon/period-only scopes via direct SQL instead of silently returning empty

### Logic fixes
- `outline-search` / `cluster-hits` groups now sorted by hit-count descending (were unsorted `HashMap` order)
- `compare-usage` `passage_count` was reporting unique n-gram count (`a_terms.len()`) instead of actual passages scanned
- All 5 F2 phrase-search commands unified through `phrase_rows_with_explicit_doc_table()` in `research_tools/phrase.rs` — scope filtering now happens *before* the parquet fetch when using the phrase index, eliminating false-negative risk of `LIMIT` on a global scan followed by scope filter

### Reorganization
- `context_expand.rs` moved to `src/research/context_expand.rs`
- `src/research.rs` promoted to `src/research/mod.rs` to allow the submodule

### Phase E tests
All 6 streaming intersection tests pass: `delta_doc_id_iter_empty`, `delta_doc_id_iter_single`, `delta_doc_id_iter_matches_vec_decoder`, `streaming_intersection_matches_old_vec_intersection`, `streaming_intersection_empty_candidates_short_circuits`, `duplicate_query_grams_deduped_by_hash_lookup`.