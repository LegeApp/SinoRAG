# Phase C — Shared text analyzer + one SIMD kernel

**Status:** in progress
**Risk:** medium. Hot-path refactor touching both phrase and tfidf builders. **On-disk format unchanged** (v2 stays exact); a v2 index built before Phase C is byte-identical to one built after, modulo ordering of equal-priority records inside a bucket file (which doesn't affect the final index — Phase 2 sorts).
**Estimated size:** ~600 LOC across 5–6 files.

## Why

Reviewer's biggest performance complaint, repeated in three sections of the review:

1. **Per-gram `String` allocation** in `phrase_index.rs::build` Phase 1. Hot path, called billions of times.
2. **Duplicated normalization + n-gram logic** between phrase and tfidf builders. Two corpora-wide passes that do the same work.
3. **Per-doc `FxHashSet<u64>` / `FxHashMap<u64, u32>`** churn for dedup/term-count when sort-then-count over a reusable `Vec<u64>` is faster, deterministic, and allocation-free.

These compound: one allocation-conscious shared analyzer fixes all three.

Reviewer's narrow SIMD recommendation: ASCII fast-path detection inside normalization only. Everything else stays scalar.

## Behaviour after this phase

- One `text_analyzer` module owns: normalize → record CJK byte offsets → emit n-gram hashes by slicing into the normalized buffer → sort+dedup for phrase / sort+count-runs for tfidf.
- Phrase index Phase 1 and TF-IDF Phase 1 consume the analyzer instead of doing their own work.
- No per-gram `String` allocation. Hashes are computed from `&normalized.as_bytes()[start..end]` directly.
- Per-doc dedup/term-count use the analyzer's reusable `Vec<u64>` scratch buffer.
- The one SIMD kernel: `is_ascii_block_16(b: &[u8]) -> bool` (uses `wide::u8x16`) accelerates the ASCII fast-path inside `normalize_zh`. CJK paths stay scalar.

## API

```rust
// src/text_analyzer.rs (new)

pub struct AnalyzeOptions {
    pub min_n: usize,
    pub max_n: usize,
    pub dedup: bool,        // phrase wants true; tfidf wants false (count_tf)
    pub count_tf: bool,     // tfidf wants true
}

pub struct AnalyzeScratch {
    pub normalized: String,
    pub cjk_byte_offsets: Vec<u32>,  // byte offset into `normalized` per CJK char
    pub all_hashes: Vec<u64>,        // n-gram hashes (insertion order)
    pub unique: Vec<u64>,            // sorted+dedup; filled when opts.dedup
    pub counts: Vec<(u64, u32)>,     // sorted hash + tf; filled when opts.count_tf
}

impl AnalyzeScratch {
    pub fn new() -> Self;
    /// Clears in place; preserves allocated capacity. Hot-path safe.
    pub fn reset(&mut self);
}

/// Normalize `text`, compute n-gram hashes per opts, fill the chosen
/// auxiliary structures. No allocation in the steady state (scratch
/// vectors grow once, get reused).
pub fn analyze(text: &str, opts: &AnalyzeOptions, scratch: &mut AnalyzeScratch);
```

The analyzer becomes the single source of normalization + n-gram extraction. Reviewer's "shared analyzer / fewer rescans" item is satisfied by having both builders call it; the "one parquet scan feeds both" architectural form is a separate refactor (deferred to optional Phase C2).

## What replaces what

### `phrase_index.rs` Phase 1

Before (hot loop, per passage):

```rust
let normalized = normalize_zh(text);
let chars: Vec<char> = normalized.chars().filter(|c| !c.is_whitespace()).collect();
let mut seen: FxHashSet<u64> = FxHashSet::default();
for window in chars.windows(gram_len) {
    let gram: String = window.iter().collect();   // allocation
    let hash = xxh3_64(gram.as_bytes());
    if seen.insert(hash) {
        emit_phrase_record(hash, doc_id);
    }
}
```

After:

```rust
scratch.reset();
analyze(text, &phrase_opts, &mut scratch);
for &hash in &scratch.unique {
    emit_phrase_record(hash, doc_id);
}
```

### `tfidf/index.rs` Phase 1

Before — `char_ngram_hashes_into` already exists and is allocation-conscious for hashes, but doesn't share normalization with phrase. After Phase C, both call the same analyzer; tfidf reads `scratch.counts` for `(term_id, tf)`.

## The single SIMD kernel

Drop-in fast-path inside `normalize_zh`:

```rust
// hot path: if a 16-byte block is all ASCII, take a copy/strip branch
// that does no UTF-8 decoding. Falls back to scalar for CJK blocks.
fn is_ascii_block_16(b: &[u8]) -> bool {
    use wide::u8x16;
    if b.len() < 16 { return b.iter().all(|c| *c < 0x80); }
    let chunk = u8x16::new(b[..16].try_into().unwrap());
    let mask = chunk & u8x16::splat(0x80);
    mask == u8x16::splat(0)
}
```

That's it. No SIMD anywhere else. Per the reviewer: "the best first SIMD experiment is ASCII fast path in normalize_zh".

## Touch list

```
src/text_analyzer.rs       (new — analyzer module)
src/normalize.rs           (ASCII fast path; one SIMD helper)
src/phrase_index.rs        (Phase 1 calls analyzer; drops per-gram String allocation)
src/tfidf/index.rs         (Phase 1 calls analyzer; drops local n-gram routine)
src/tfidf/ngram.rs         (keep as thin re-exports for now, deprecate later)
Cargo.toml                 (+ wide dep, narrow features)
```

## Validation checklist

- `cargo build --release` clean.
- Unit test (new): given a known passage, analyzer output (unique hashes, counts) matches the previous code's output. Ensures byte-identity-of-postings between v2-old-builder and v2-new-builder for that passage. Run on a few corpus samples.
- Microbenchmark (new, `benches/analyzer.rs`): per-passage analyze throughput before vs after. Target ≥ 4× improvement.
- Phrase + tfidf index files built by Phase C on the same corpus as v2-old are byte-identical (or post-sort identical inside Phase 2 buckets).

## Out of scope for Phase C

- Roaring postings / quantization → Phase G.
- Streaming postings intersection → Phase E.
- One-pass-feeds-both-builders → optional Phase C2 (deferred).
- More SIMD kernels (TF-IDF dot products, varint decode) → only after profiling shows them as next-largest costs.

## Migration / rollback

- No on-disk format change. v2 files built before Phase C remain queryable. v2 files built after Phase C are byte-equivalent within the v2 contract.
- If a problem surfaces, `git revert` the Phase C commit and rebuild from buckets — Phase 1 outputs go to the work dir, Phase 2/3 are deterministic given those, so a partial rebuild is cheap.
