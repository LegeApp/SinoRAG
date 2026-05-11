# Phase A — Correctness fixes

**Status:** in progress
**Risk:** none — no on-disk format changes, no effect on the running TF-IDF build or the existing `phrase_v2.index`.
**Estimated size:** ~100 LOC across 4 files.

Scope is the reviewer's "biggest stability problems" 1–3 + 5–6 plus a one-time data repair (item 4) that requires no code. Items here are mechanical, isolated, and shippable in one commit.

## A1. Honor `ingest` CLI args (`out_jsonl`, `out_parquet`, `zen_only`)

**Bug:** `src/cli.rs::Command::Ingest` declares `out_jsonl`, `out_parquet`, `zen_only` but `src/commands/mod.rs` discards them with `out_jsonl: _, out_parquet: _, zen_only: _`. `src/commands/ingest.rs` then synthesizes its own paths from `--out` and hardcodes `let zen_only = false;`. User-facing flags exist but do nothing.

**Fix:**
- Add the three parameters to `ingest::run`.
- `commands/mod.rs` passes them through.
- `commands/ingest.rs` uses the supplied paths directly; falls back to `out.join("passages.{jsonl,parquet}")` only when the flags are at their defaults.

**Files:** `src/commands/mod.rs`, `src/commands/ingest.rs`.

## A2. Repair ingest → catalog hand-off

**Bug:** I made `catalog-index-build --doc-table` required in an earlier commit, but `ingest::run` still passes `None` for the doc-table parameter when it triggers the catalog build internally (`commands/ingest.rs:297`). The default ingest path therefore fails at the catalog step after doing all the parquet work.

**Fix:** Pass `Some(doc_table_path.clone())` at the call site. The doc-table is built earlier in the same `run()` (`commands/ingest.rs:255–259`), so the path is in scope.

**Files:** `src/commands/ingest.rs`.

## A3. Disable unsafe ingest resume

**Bug:** `commands/ingest.rs` tracks resume state in `processed_files: HashSet<String>`, but on resume the per-corpus parquet part indices are reset to 0:
```
let mut cbeta_part_index = 0usize;
let mut kanripo_part_index = 0usize;
```
The writer (`storage::write_parquet_part_partitioned` → `File::create`) will then overwrite `part-000000.parquet` in existing partitions. This is silent data loss.

**Fix:** Until proper staging-directory resume is implemented (Phase D), if a `.ingest_checkpoint.json` is found, abort with a clear error:
> "Ingest checkpoint found at <path>. Resume is currently unsafe (would overwrite existing parquet parts). Delete the checkpoint and the parquet directory, then re-ingest from scratch — or wait for the staging-directory resume (Phase D)."

**Files:** `src/commands/ingest.rs`.

## A4. Repair existing `doc_table.bin` alignment *(operator action, no code)*

The current 5.3 GB `doc_table.bin` has a latent bug (reviewer point 4) where `source_work_ids[doc_id]` and `period_ranks[doc_id]` are misaligned because only `passage_ids` was sorted. The primary `passage_id ↔ doc_id` mapping is correct, so phrase + TF-IDF are unaffected; but any consumer that reads the auxiliary arrays (e.g. the new registry populator) gets wrong values.

My doc-table refactor in commit `4a7d206` builds all aligned arrays from a single sorted index list, fixing the bug going forward. The existing file still has it.

**Action (after the in-flight TF-IDF finishes):**
```
sinoragd doc-table-build \
  --parquet data/passages.parquet \
  --out     data/derived/doc_table.bin
```
≈30–60 min for 40M passages. Phrase + TF-IDF stay valid (primary fingerprint over `passage_ids` is unchanged).

## A5. Remove hardcoded `/mnt/...` temp paths

**Bug:** Two builders bake an absolute path into the source:
- `src/phrase_index.rs:476` — `"/mnt/Samsung980_1TB/Rust-projects/not-rust-projects/ReadZen/GraphDiscovery/tmp_phrase_v2"`
- `src/tfidf/index.rs:621` — `"/mnt/Samsung980_1TB/Rust-projects/SinoRAG/data/derived/_tmp_tfidf"`

Anyone running the binary on a different filesystem hits a write error. Defaults should be relative to the output index path.

**Fix:** Compute the default as `<out_path>.work/` if `temp_dir` is `None`. So `tfidf-build --out data/derived/tfidf.index` (no `--temp-dir`) writes to `data/derived/tfidf.index.work/`. Same for phrase.

**Files:** `src/phrase_index.rs`, `src/tfidf/index.rs`.

## A6. Compute phrase-index bucket count + honor `phrase_max_memory`

**Bug:** `commands/ingest.rs:263` is `let _ = phrase_max_memory;` — the flag is parsed and silently dropped. The bucket count is then hardcoded `2048` regardless of corpus size or memory budget. Small corpora get more file churn than necessary; very large corpora may underprovision.

**Fix:** Add `bucket_count_for_corpus(num_parquet_files: usize, memory_budget_bytes: Option<u64>) -> usize` to `crate::memory`. Heuristic:
- Estimate target records per bucket from `memory_budget / record_size / safety_factor`.
- Estimate total records as `num_parquet_files × records_per_file_estimate`.
- Snap to next power of two, clamped to `[64, 8192]`.

Apply it in `commands/ingest.rs` for both phrase and tfidf when no explicit `--buckets` flag is supplied. Defaults stay 2048 if `memory_budget` is unset.

**Files:** `src/memory.rs`, `src/commands/ingest.rs`.

## Files touched (Phase A)

```
src/cli.rs               (no change actually needed — CLI surface intact)
src/commands/mod.rs      (A1: thread out_jsonl/out_parquet/zen_only through)
src/commands/ingest.rs   (A1, A2, A3, A6)
src/phrase_index.rs      (A5)
src/tfidf/index.rs       (A5)
src/memory.rs            (A6 helper)
```

## Validation checklist

- `cargo build --release` clean.
- `sinoragd ingest --help` shows `out_jsonl`, `out_parquet`, `zen_only` flags.
- Invoking `sinoragd ingest` with a stale `.ingest_checkpoint.json` aborts with the error message.
- `sinoragd tfidf-build --out /tmp/test.index` (no `--temp-dir`) writes work files under `/tmp/test.index.work/`.
- `sinoragd ingest --phrase-max-memory 4G ...` produces an eprintln line showing the chosen bucket count.

## Out of scope for Phase A

- Real staging-directory resume → Phase D.
- Phrase/tfidf format changes → Phases C/E.
- New tools → Phase F.
