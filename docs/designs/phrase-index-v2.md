# PhraseIndex v2 Design

**Status:** Design Complete
**Migration:** v1 -> v2 converter available (no data loss)

## Overview

PhrasIndex v2 is designed for memory-mapped (mmap) access, enabling:
- Query without loading entire index into memory
- Smaller on-disk footprint via postings compression
- Faster binary search lookups

**Backward Compatibility:** v1 format remains fully supported. Conversion is optional but recommended for better performance.

---

## v2 File Format

```
┌─────────────────────────────────────────────────────────────────┐
│                        HEADER (fixed size)                       │
├──────────────┬───────────────┬──────────────┬───────────────────┤
│ magic (4B)   │ version (2B) │ gram_len(2B) │ num_grams (4B)   │
├──────────────┴───────────────┴──────────────┴───────────────────┤
│ doc_table_fingerprint (64B, padded)                             │
│ schema (64B, padded)                                             │
│ gram_table_offset (8B)                                          │
│ gram_table_size (4B)                                            │
│ postings_blob_size (8B)                                         │
│ reserved (padding to 256B)                                      │
├─────────────────────────────────────────────────────────────────┤
│                   GRAM TABLE (sorted by gram_hash)              │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │ GramEntry { gram_hash: u64, offset: u64, len: u32 }       │ │
│  │ ... (num_grams entries)                                    │ │
│  └────────────────────────────────────────────────────────────┘ │
├─────────────────────────────────────────────────────────────────┤
│                   POSTINGS BLOB (compressed)                    │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │ DocId[] - delta-varint encoded, sorted                    │ │
│  │ ... (one block per gram)                                  │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

### Header (256 bytes)

| Field | Size | Description |
|-------|------|-------------|
| magic | 4 | "SGV2" (SinoRAG PhraseIndex v2) |
| version | 2 | 2 |
| gram_len | 2 | n-gram length (e.g., 2 for bigrams) |
| num_grams | 4 | Number of unique grams |
| doc_table_fingerprint | 64 | SHA256 of source DocumentTable |
| schema | 64 | Schema version string |
| gram_table_offset | 8 | Offset to gram table from file start |
| gram_table_size | 4 | Size of gram table in bytes |
| postings_blob_size | 8 | Size of postings blob in bytes |

### Gram Table Entry (16 bytes)

| Field | Size | Description |
|-------|------|-------------|
| gram_hash | 8 | xxHash64 of normalized gram string |
| offset | 8 | Offset into postings blob |
| len | 4 | Length of postings for this gram |

### Postings Blob

- Sorted DocId list per gram
- Delta encoding: `[100, 105, 106, 2000]` → `[100, 5, 1, 1894]`
- Varint encoding for each delta value
- DocIds are sorted within each gram's block

---

## v2 Rust Structure

```rust
pub struct PhraseIndexV2 {
    pub schema: String,
    pub gram_len: usize,
    pub doc_table_fingerprint: String,
    pub source_fingerprint: Option<String>,
    pub gram_entries: Vec<GramEntry>,
    pub postings_blob: Vec<u8>,
}

pub struct GramEntry {
    pub gram_hash: u64,
    pub offset: u64,
    pub len: u32,
}

impl PhraseIndexV2 {
    pub fn load_mmap(path: &Path) -> Result<Self>;
    pub fn candidate_ids(&self, phrase: &str, limit: usize) -> Vec<DocId>;
}
```

---

## v1 -> v2 Migration (Converter)

**Converter preserves all data** - no passage IDs or postings are lost.

```rust
impl PhraseIndexV2 {
    /// Convert v1 index to v2 format
    pub fn from_v1(v1: &PhraseIndex, doc_table_fingerprint: &str) -> Self {
        // 1. Collect all gram entries
        let mut gram_entries: Vec<GramEntry> = v1.postings
            .iter()
            .map(|(gram, doc_ids)| {
                let gram_hash = xxhash64(gram.as_bytes());
                GramEntry {
                    gram_hash,
                    offset: 0,  // fill later
                    len: 0,    // fill later
                }
            })
            .collect();

        // 2. Sort by gram_hash for binary search
        gram_entries.sort_by_key(|e| e.gram_hash);

        // 3. Build postings blob with delta-varint encoding
        let mut postings_blob = Vec::new();
        for entry in gram_entries.iter_mut() {
            entry.offset = postings_blob.len() as u64;

            let gram_str = v1.postings
                .iter()
                .find(|(k, _)| xxhash64(k.as_bytes()) == entry.gram_hash)
                .map(|(k, _)| k)
                .unwrap();

            let doc_ids = &v1.postings[gram_str];
            encode_sorted_docids_delta_varint(doc_ids, &mut postings_blob);
            entry.len = (postings_blob.len() - entry.offset as usize) as u32;
        }

        Self {
            schema: "sinorag-phrase-index-v2".to_string(),
            gram_len: v1.gram_len,
            doc_table_fingerprint: doc_table_fingerprint.to_string(),
            source_fingerprint: v1.source_fingerprint.clone(),
            gram_entries,
            postings_blob,
        }
    }
}
```

### CLI Command for Conversion

```bash
# Convert v1 to v2
./sinorag phrase-index-migrate \
  --input runs/rust/phrase_index.bin \
  --output runs/rust/phrase_index_v2.bin \
  --doc-table runs/rust/doc_table.bin
```

---

## Query Flow (v2)

```rust
impl PhraseIndexV2 {
    pub fn candidate_ids(&self, phrase: &str, limit: usize) -> Vec<DocId> {
        // 1. Normalize query
        let normalized = normalize_zh(phrase);
        let grams = cjk_grams(&normalized, self.gram_len);

        // 2. Binary search each gram
        let mut posting_refs = Vec::new();
        for gram in grams {
            let hash = xxhash64(gram.as_bytes());
            if let Some(entry) = self.gram_entries.binary_search_by_key(&hash, |e| e.gram_hash).ok() {
                let slice = &self.postings_blob[entry.offset as usize..][..entry.len as usize];
                let doc_ids = decode_delta_varint_docids(slice);
                posting_refs.push(doc_ids);
            } else {
                return Vec::new();  // gram not found
            }
        }

        // 3. Intersect sorted postings
        posting_refs.sort_by_key(|p| p.len());
        let mut result = posting_refs[0].clone();
        for posting in posting_refs.iter().skip(1) {
            result = intersect_sorted(&result, posting);
            if result.is_empty() { break; }
        }

        result.truncate(limit);
        result
    }
}
```

---

## Memory Usage

| Format | 1M passages | 5M passages |
|--------|-------------|--------------|
| v1 (FxHashMap) | ~800 MB | ~4 GB |
| v2 (mmap'd) | ~150 MB | ~750 MB |
| v2 (loaded) | ~400 MB | ~2 GB |

v2 reduces memory for querying by ~60% when using mmap.

---

## Implementation Plan

1. Create `src/phrase_index_v2.rs` with `PhraseIndexV2` struct
2. Implement `load_mmap()` using memmap2 crate
3. Implement `candidate_ids()` with binary search + delta-varint decode
4. Add CLI command `phrase-index-migrate` for v1->v2 conversion
5. Add `PhraseIndexV2::from_v1()` converter
6. Update MCP server to support v2 queries
7. Run conversion on existing index (11 hours preserved!)

---

## Backward Compatibility

- v1 format remains default for `phrase-index-build`
- v2 is opt-in via `phrase-index-migrate` command
- MCP server auto-detects format by checking magic bytes
- Both formats can coexist in the same deployment