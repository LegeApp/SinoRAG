# Phase D part 1 — Real nested catalog

**Status:** in progress
**Risk:** low. Touches catalog format only. Phrase + TF-IDF unaffected.
**Estimated size:** ~400 LOC across 2 files (`catalog_index.rs`, `commands/catalog_index.rs`).
**Rebuild required:** `catalog.index` only (~6 MB; seconds).

## Why

The current catalog is one-level-deep and structurally wrong. From the reviewer's review:

1. It builds **one node per work**. The `OutlineNodeKind` enum declares `Corpus`, `Canon`, `Work`, `Volume`, `Fascicle`, `Chapter`, `Section`, `Division`, `PassageRange` — but only `Work` is ever emitted. Outline / sections / scope queries therefore can't usefully descend below the work level.
2. Work IDs are **parsed out of `passage_id` strings** with a fragile filename-stripping routine that doesn't survive most non-CBETA passage shapes. `doc_table` now carries `source_work_id` properly, but even simpler: the catalog already reads `source_work_id` from parquet during its scan — that's the canonical value. There's no excuse to ever reconstruct it from `passage_id`.

The downstream cost is that every scope-limited or outline-aware tool (`outline_search`, `expand_context_adaptive`, `summarize_work_for_query`, `cluster_hits by="section"`, `find_first_mention scope=work/chapter`) has nothing useful to scope against.

## Behaviour after this phase

A single parquet scan groups passages by `(source_corpus, canon, source_work_id, source_rel_path, div_path)` and emits a real tree:

```
Corpus (cbeta | kanripo | terebess | …)
└── Canon (T | X | … | empty for kanripo/terebess)
    └── Work (T09n0262 = Lotus Sutra)
        └── Division (from `div_path`, split on " / "; one node per segment, deduplicated)
            └── PassageRange (contiguous doc_id runs with the same parent)
```

Per node:
- `node_id`, `parent_id`, `children: Vec<NodeId>`.
- `node_kind: OutlineNodeKind` (Corpus | Canon | Work | Division | PassageRange).
- `label` — for Division nodes this is the most recent `heading` text seen in that range; for Works it's `main_title`; for Canons it's `canon_name`.
- `first_doc_id`, `last_doc_id`, `passage_count`, `cjk_char_count` — aggregated up.
- `source_rel_path` retained on Work nodes and below.

The catalog also writes a fully-populated `doc_parent: FxHashMap<DocId, NodeId>` that points each doc at its deepest containing node (the PassageRange leaf if present, else the lowest Division, else the Work). This makes `scope(node_id)` an O(1) lookup and lets downstream tools filter posting lists by node membership.

## What changes vs the current code

### Removed

- `extract_work_id_from_passage_id` (gone — the work_id comes from the `source_work_id` column).
- `build_catalog_from_work_data` (replaced by `build_catalog_from_passages`, which sorts and walks groups instead of iterating an unordered HashMap).

### Added

- `PassageRow`: a flattened parquet row carrying the columns the builder needs.
- `build_catalog_from_passages`: the new entry point. Single sorted walk; emits nested nodes.
- A schema update: the corpus catalog schema changed. The existing on-disk `catalog.index` won't load with the new code (cheap rebuild).

### Preserved

- `OutlineCatalog` (renamed in struct sense but on-disk shape stays bincode-compatible — fields added are at the end, defaulted with `#[serde(default)]`).
- `OutlineNodeKind` enum (already has the kinds we need).
- `WorkRecord` and per-work metadata (canon, period, author, traditions, etc.).
- CLI surface (`catalog-index-build`, `outline`, `sections`, `scope`, `works`).

## Validation

After the rebuild:

```
catalog-index-info --index data/derived/catalog.index
```

should report:

- `schema == "readzen-corpus-catalog"`
- `nodes > works × 5` (every work has at least a few divisions and a passage range)
- `doc_parent.len() == doc_table.passage_ids.len()` (every doc has a parent)

A focused sanity command:

```
outline --work T48n2005 --max-depth 5
```

should now show real chapter/section structure for the Platform Sutra, not just one root node.

## Out of scope (Phase D part 2 — separate doc)

- `CorpusSource` trait + atomic staging-directory ingest.
- Scope query improvements (`scope --within` semantics, range-based filters that posting lists honor at search time).
