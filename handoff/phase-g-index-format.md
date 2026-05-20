# Phase G — index format (production target)

**Status:** deferred. Captured here so it can be picked up cleanly when production rebuild is convenient.
**Risk on adoption:** forces a one-time rebuild of `phrase_*.index` and `tfidf*.index`. Magic bytes change; older files are not readable by current code . Plan the rebuild against a cloud VM or a quiet local window.
**Order in plan:** after Phase E (streaming intersection lands first on v2 so the gains land independently). The same rebuild absorbs the doc_table alignment repair and terebess ingest.

The two wins drive everything in this phase:
1. **TF-IDF** weights drop from `f32` → 8-bit log-quantized — index ~3× smaller, faster I/O.
2. **Phrase** postings become a hybrid: dense lists use Roaring chunks, sparse lists stay delta-varint — common-gram intersection becomes 10–100× faster while size stays similar.

Everything else (mmap-first layout, sorted vocab, external-sort builder, doc_table-keyed identity) is retained as-is.

---

## What changes vs v2

### TF-IDF current format — quantize weights to u8

Header magic: `b"SRTF3VAA"` (v2 was `MAGIC_TFIDF_V2`). `version: u16 = 3`. All other header fields unchanged.

Row blob — was per-doc list of `(term_id: u32, weight: f32)` (8 B per entry); becomes `(term_id: u32, qweight: u8)` (5 B per entry, or 4 B if we pack `term_id:24 | qweight:8`). Row offsets shift accordingly.

Postings blob — same shape: `(doc_id: u32, qweight: u8)` (5 B per entry, or 4 B packed).

Quantization scheme: **8-bit logarithmic**.

```
qweight = clamp_u8(round(255 * log2(1 + w) / log2(1 + W_max)))
w_approx = (2 ** (qweight * log2(1 + W_max) / 255)) - 1
```

Where `W_max` is the per-column max weight, stored once per term in the vocab table (one extra `f32` per term — small, only `vocab_count` entries). So vocab entry grows from `(term_hash: u64, term_id: u32, df: u32, idf: f32, postings_off: u64, postings_len: u32)` to add `w_max: f32`. ~hundreds of MB added at the vocab level, but the row+postings blobs (the file's dominant cost) shrink ~3×.

Query path: dequantize on read using `w_max`. Final top-K reranked with the dequantized scores (good enough; coarse quantization affects ranks beyond ~top-50 only).

**Size estimate** for your 40 M-doc CBETA corpus:
- v2 expected: row blob ≈ 40M × avg-50 features × 8 B = 16 GB; postings ≈ similar.
- Current format expected: ~5–7 GB row blob + ~5–7 GB postings blob → **TF-IDF index ~30–40 GB → 10–15 GB**.

### Phrase current format — hybrid roaring/varint postings

Header magic: `b"SRPH3VAA"` (v2 was `b"SRPHV2AA"` or whatever the constant is). `version: u16 = 3`.

Gram-table entry was `(gram_hash: u64, postings_offset: u64, postings_len: u32)` (20 B). Becomes `(gram_hash: u64, postings_offset: u64, postings_len: u32, encoding: u8, padding: u3)` (24 B with padding). `encoding`: `0 = delta-varint`, `1 = roaring`.

Postings blob layout — per-gram:
- If `df ≤ density_threshold`: delta-varint bytes (same as v2).
- Else: Roaring serialization (chunked bitmaps, 16-bit-low + 32-bit-high doc_id split). Read with the `roaring` crate, which has a stable on-disk format.

Density threshold default: `df > num_docs / 64`. Empirically the crossover where Roaring becomes both smaller and faster for u32 set operations on doc_ids.

**Size estimate**: ~same total. Roaring is denser for high-DF grams; delta-varint stays denser for tail. The win is query speed, not file size.

### Skip pointers — implicit via Roaring

Roaring chunks give O(sqrt(N)) skip behavior for free. For the delta-varint tail (sparse grams), full decode of the smaller side stays optimal. No separate skip-pointer table needed.

### Doc-table v2 minor bump (optional)

While we're at it, consider adding `min_doc_id_per_work: Vec<u32>` to the doc_table so catalog can range-scope without scanning `doc_parent`. This stays backward compatible — older readers ignore the field. Decision can be made at rebuild time; no commitment now.

---

## What does NOT change

- Magic bytes header structure, just different values.
- Sorted gram/term tables.
- Per-doc row table (offset+length lookup).
- Mmap layout.
- External-sort builder pipeline.
- doc_table fingerprint discipline.
- CLI surface (`tfidf-build`, `phrase-index-build` keep the same flags; they emit the current format silently).

---

## Implementation plan

### G1. Quantizer + dequantizer helpers (~80 LOC, common code path)

```rust
// src/tfidf/quantize.rs
pub fn quantize_log_u8(w: f32, w_max: f32) -> u8;
pub fn dequantize_log_u8(q: u8, w_max: f32) -> f32;

#[cfg(test)] // round-trip + monotonicity tests
```

### G2. TF-IDF builder emits the current format (~150 LOC change in `tfidf/index.rs`)

- During Phase 3 (term grouping), compute `w_max` for each term while writing the postings blob.
- Pack `w_max` into vocab entries.
- Switch row blob + postings blob writers to emit u8.
- Bump header magic + version.

### G3. TF-IDF reader emits f32 (~80 LOC, `tfidf/index.rs::open` + scoring)

- Validate current magic.
- Load `w_max` per term into a parallel `Vec<f32>`.
- On posting-list scan, dequantize on-the-fly into accumulator.
- Final top-K rerank: optional; for most queries 8-bit precision suffices.

### G4. Phrase builder emits the current format hybrid (~200 LOC, `phrase_index.rs::build`)

- After Phase 2 (DF aggregation), compute `density_threshold` from `num_docs`.
- During Phase 3 (per-gram posting write), decide encoding per gram. If dense, write Roaring serialization; else varint as today.
- Per-gram encoding byte added to gram-table entry.

### G5. Phrase reader chooses decoder by encoding flag (~100 LOC, `phrase_index.rs`)

- Vocab entry now carries encoding byte.
- `candidate_ids_for_normalized` dispatches per-gram on encoding flag.
- Intersection algorithm: separate the dense and sparse halves; bitwise-AND dense, varint-decode sparse; merge at the end.

### G6. CLI + `*-info` output updates (~30 LOC)

- `tfidf-info`, `phrase-index-info` report current index metadata: quantization scheme, encoding mix.
- Reject v2 files with clear "rebuild required" message (cheap; the current format is forward-only).

### G7. Cargo dependencies

- `roaring = "0.10"` (or current).

---

## Rebuild workflow on index refresh day

```bash
cd /mnt/Samsung980_1TB/Rust-projects/SinoRAG

# 1. Ingest terebess if not already done.
./target/release/sinorag ingest-terebess --input terebess_zen_text_images

# 2. Rebuild doc_table from scratch (also fixes alignment per Phase A4).
./target/release/sinorag doc-table-build \
  --parquet data/passages.parquet \
  --out     data/derived/doc_table.bin
#    ≈ 30–60 min for 40M passages.

# 3. Build the indexes against the fresh doc_table.
./target/release/sinorag phrase-index-build  # → phrase.index
./target/release/sinorag tfidf-build          # → tfidf.index
./target/release/sinorag catalog-index-build \
  --doc-table data/derived/doc_table.bin

# 4. Pack.
./target/release/sinorag build-pack --pack data
```

Note: paths in default CLI may be `phrase_v2.index` / `tfidf.index`. On index refresh day either rename the defaults to `phrase.index` / `tfidf.index` in `cli.rs` *or* keep the names and rely on magic bytes to disambiguate. Renaming is cleaner; old v2 files stay on disk under their original names.

---

## Validation checklist (when the format lands)

- `cargo build --release` clean.
- Quantizer unit tests pass (round-trip + monotonicity within ε).
- `tfidf-info` and `phrase-index-info` report current format + encoding stats.
- A known top-K similarity query result differs from v2 in at most ~5% of positions beyond top-50.
- `build-pack` accepts only current index indexes; rejects v2 with a "rebuild against the current format" message.
- File sizes: TF-IDF index ≥ 2.5× smaller than the v2 equivalent for the same corpus.
- A common-gram phrase query (e.g. `如是我聞` or any 2-char prefix that hits >10M docs) returns in ≤100 ms wall-time vs current v2 minutes-scale.

---

## When to do this

Suggested triggers:
- Production deploy (cloud VM with ≥256 GB RAM + fast NVMe is the right environment for the rebuild).
- After Phase E lands so streaming intersection wins are decoupled and measured separately.
- When you're already going to rebuild the indexes for a corpus refresh (quarterly CBETA snapshot, etc.) — fold the format update into that same rebuild.

Until then, current v2 files stay valid and queryable. Phase C/D/E will keep emitting and reading v2.
