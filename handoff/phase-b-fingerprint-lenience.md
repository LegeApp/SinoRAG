# Phase B — Pack-level fingerprint lenience

**Status:** queued
**Risk:** low. No format changes; only the validator logic changes.
**Estimated size:** ~80 LOC across 3 files.

## Why

After Phase A's terebess workflow becomes usable, you can append new corpora to `doc_table.bin` via `doc-table-build --append-to`. The append produces a *new* `source_fingerprint` (and writes the `<path>.bin.lineage.json` sidecar with `base_fingerprint`). The existing `phrase_v2.index` and `tfidf.index` files were built against the predecessor's fingerprint and now look "stale" to strict validators — even though they remain perfectly valid for every `doc_id < base_doc_count`.

Wiring the lineage into the validators turns this into a graceful "covers a prefix of the doc_table" state instead of a hard refusal.

## Behaviour after this phase

- `PhraseIndex::open` / `TfidfIndex::open` accept an index whose `doc_table_fingerprint` is either the current `DocumentTable::source_fingerprint` **or** the immediately preceding `base_fingerprint` (sidecar lineage).
- When the index matches `base_fingerprint`, query code is informed of `base_doc_count`: any `doc_id >= base_doc_count` returned from search is ignored (not possible — index can't emit them), and a single eprintln logs the situation: `index covers doc_ids 0..N (N < total); rebuild to extend.`
- `build-pack` validator honours the same rule and writes `index_sections.coverage = "base"` (vs `"full"`) so downstream agents can spot a partial coverage state.

## Touch list

```
src/document_table.rs          (already has `fingerprint_compatible`; expose `DocTableLineage::load_if_present`)
src/phrase_index.rs            (PhraseIndex::open_with_doc_table; current `open` keeps strict behaviour for callers that hold no doc_table)
src/tfidf/index.rs             (same)
src/commands/build_pack.rs     (accept lineage-matched indexes; record coverage)
src/templates/research_packet/ (gather: pass doc_table to PhraseIndex::open_with_doc_table)
```

## API delta

```rust
// phrase_index.rs and tfidf/index.rs
impl PhraseIndex {
    pub fn open_with_doc_table(path: &Path, doc_table: &DocumentTable) -> Result<Self>;
    pub fn coverage(&self) -> IndexCoverage;
}
pub enum IndexCoverage { Full, Base { base_doc_count: u32 } }
```

The existing `open(path)` remains as a strict-mode helper for callers that don't hold a doc_table (e.g. `*-info` commands).

## Validation checklist

- `build-pack --pack data` accepts both a current-fingerprint phrase index and a base-fingerprint phrase index, recording coverage correctly in `pack_index_sections`.
- After `doc-table-build --append-to`, `search`/`similar` still return CBETA hits; a one-line eprintln notes lineage coverage.
- Strict `phrase-index-info --index ...` (no doc_table) keeps working unchanged.

## Out of scope

- Phrase/tfidf merge code paths (those go away in favour of the resume-based incremental rebuilds anyway).
- Multi-step lineage chains (only one ancestor accepted; deeper chains require either re-export of older fingerprints or a re-design of the lineage sidecar).
