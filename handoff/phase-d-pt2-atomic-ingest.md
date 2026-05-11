# Phase D part 2 — Atomic ingest staging + safe resume

**Status:** in progress
**Risk:** low. Touches `commands/ingest.rs` only. No format changes.
**Estimated size:** ~250 LOC.

## Why

Phase A item A3 disabled ingest resume with an outright abort because the existing resume logic was unsafe: per-corpus part indices reset to `0` and `File::create` would overwrite `part-000000.parquet`. Phase D part 2 makes resume actually safe and makes the whole ingest atomic.

The reviewer's recommended pattern: stage output in a temporary directory, only promote to the canonical path after the run validates. Implements points 2 and 4 of the reviewer's resume-fix list: a staging directory and a checkpoint that stores `next_part_index` per corpus.

## Behaviour after this phase

Ingest writes to:

```
data/.staging/ingest-<utc_id>/
  passages.jsonl
  passages.parquet/
    source_corpus=cbeta/
    source_corpus=kanripo/
  .ingest_checkpoint.json
```

On success it atomically moves:

```
.staging/ingest-<id>/passages.jsonl       → out/passages.jsonl
.staging/ingest-<id>/passages.parquet/    → out/passages.parquet (per-partition mv)
```

The `.staging/ingest-<id>/` directory is deleted on success and preserved on failure for inspection.

The checkpoint records:

```json
{
  "schema": "sinoragd-ingest-checkpoint-v1",
  "run_id": "ingest-2026-05-12T03-15-00Z",
  "started_utc": "2026-05-12T03:15:00Z",
  "processed_files": ["..."],
  "next_part_index": { "cbeta": 42, "kanripo": 18 },
  "stats": { "cbeta": 1000000, "kanripo": 250000, "total": 1250000 }
}
```

A subsequent invocation passes `--resume <staging-path>` (or auto-detects the freshest staging dir under `data/.staging/` if `--resume auto`) and continues from where the checkpoint left off without resetting part indices.

If a partition with the same `source_corpus=...` already exists in `out/passages.parquet/`, the promotion step refuses with a clear error: *"partition source_corpus=cbeta already exists at <path>. Delete it or ingest into a different `--out` directory."* No silent overwrite.

## Touch list

```
src/commands/ingest.rs    — staging dir, atomic promote, safe resume
```

`tei`/`ingest::kanripo` extraction code is unchanged.

## API delta

```rust
// commands/mod.rs dispatch adds:
//   Command::Ingest { ..., resume: Option<PathBuf> }
// where resume is None for fresh runs, Some(path) to continue.

// commands/ingest.rs:
pub async fn run(
    corpus: Option<PathBuf>,
    kanripo_input: Option<PathBuf>,
    sorting_data_dir: Option<PathBuf>,
    out: Option<PathBuf>,
    out_jsonl: PathBuf,
    out_parquet: PathBuf,
    zen_only: bool,
    resume: Option<PathBuf>,   // new
    ...
) -> Result<()>
```

## Validation checklist

- Fresh ingest: produces `data/passages.parquet/source_corpus=...` partitions; staging dir is gone after success.
- Kill mid-ingest, then `sinoragd ingest --resume data/.staging/ingest-<id>`: continues from the last checkpoint, part indices advance from saved values, no partition file overwrites.
- Try to ingest into an output that already has `source_corpus=cbeta`: fails with the clear error before doing any work.
- On exception inside the run, staging dir is left intact for diagnosis.

## Out of scope (future)

- **CorpusSource trait + DirectorySource / ZipSource / TarZstSource.** The existing CBETA/kanripo/terebess ingest paths read from disk directly via corpus-specific walkers; introducing a unified `CorpusSource` abstraction is architecturally clean but doesn't unblock any user-visible feature. Deferred to a later cleanup. When it lands, `Source` will be a thin wrapper that opens a file by logical path (filesystem path or zip-entry name) so the per-corpus parsers don't need to know which form the corpus is in.
- **Streaming archive ingest** (don't extract to temp; stream zip/tar entries through the parser). Pairs with `CorpusSource` above.
- **Cross-run dedup** by content hash (so repeated identical XML files don't re-emit duplicate passages). Cheap once `SourceEntry` carries a content_hash; not needed yet.
