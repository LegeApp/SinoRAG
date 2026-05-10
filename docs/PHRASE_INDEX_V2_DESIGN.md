# PhraseIndex v2 Design Document

## Overview

PhraseIndex v2 is a redesigned phrase index format that is memory-mapped friendly and optimized for fast exact substring search. It uses doc_ids (u32) instead of passage_id strings, gram hashes for indexing, and compressed postings lists to minimize memory footprint.

## Design Goals

1. **Memory-mapped friendly**: The entire index should be mmap-able without deserialization overhead
2. **Compact storage**: Use compressed postings and avoid string duplication
3. **Fast lookup**: Hash-based gram indexing with efficient postings access
4. **Doc_id centric**: Reference DocumentTable for passage_id resolution
5. **Backward compatibility**: Keep v1 format for existing installations

## Data Structures

### Core Types

```rust
pub type DocId = u32;
pub type GramHash = u64;  // SHA-256 truncated or custom hash
pub type Offset = u32;    // Offset within mmap'd file
```

### Index Header

```rust
#[repr(C)]
pub struct PhraseIndexV2Header {
    pub magic: [u8; 8],           // "PHRASEV2"
    pub version: u32,             // Format version
    pub schema: StringOffset,     // Offset to schema string
    pub doc_table_fingerprint: StringOffset,  // Offset to fingerprint
    pub gram_len: u32,            // N-gram length (e.g., 4)
    pub num_grams: u32,           // Total unique grams
    pub num_docs: u32,           // Total documents
    pub total_postings: u32,      // Total postings across all grams
    pub gram_table_offset: Offset, // Offset to gram table
    pub postings_offset: Offset,  // Offset to compressed postings
    pub doc_table_offset: Offset, // Offset to doc_id list (optional)
}
```

### Gram Table Entry

```rust
#[repr(C)]
pub struct GramEntry {
    pub gram_hash: GramHash,      // Hash of the gram
    pub postings_offset: Offset,  // Offset to postings data
    pub postings_count: u32,      // Number of doc_ids in this posting list
}
```

### Postings Format

Postings are stored as compressed doc_id lists using delta-varint encoding:

```
[doc_id_0][delta_1][delta_2]...[delta_n]
```

Where:
- First value is the base doc_id (varint encoded)
- Subsequent values are deltas from previous doc_id (varint encoded)

This encoding is compact for sequential doc_ids and allows efficient range queries.

### Optional Doc List

For fast doc_id to passage_id resolution without loading DocumentTable:

```rust
#[repr(C)]
pub struct DocEntry {
    pub doc_id: DocId,
    pub passage_id_offset: StringOffset,
}
```

## Memory Layout

```
[Header]
[Schema String]
[Doc Table Fingerprint String]
[Gram Table] (sorted by gram_hash for binary search)
[Postings Data] (compressed)
[Optional Doc List]
```

## API Design

### Reading (mmap-based)

```rust
pub struct PhraseIndexV2 {
    mmap: memmap2::Mmap,
    header: &'static PhraseIndexV2Header,
    gram_table: &'static [GramEntry],
    postings_data: &'static [u8],
}

impl PhraseIndexV2 {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        let header = unsafe { &*(mmap.as_ptr() as *const PhraseIndexV2Header) };
        
        let gram_table = unsafe {
            std::slice::from_raw_parts(
                mmap.as_ptr().add(header.gram_table_offset as usize) as *const GramEntry,
                header.num_grams as usize,
            )
        };
        
        let postings_data = unsafe {
            std::slice::from_raw_parts(
                mmap.as_ptr().add(header.postings_offset as usize),
                (mmap.len() - header.postings_offset as usize),
            )
        };
        
        Ok(Self { mmap, header, gram_table, postings_data })
    }
    
    pub fn search(&self, phrase: &str) -> Result<Vec<DocId>> {
        let grams = ngrams(phrase, self.header.gram_len);
        let mut candidates: Option<Vec<DocId>> = None;
        
        for gram in grams {
            let gram_hash = hash_gram(&gram);
            let postings = self.get_postings(gram_hash)?;
            
            match candidates {
                None => candidates = Some(postings),
                Some(ref mut cands) => intersect_in_place(cands, &postings),
            }
        }
        
        candidates.ok_or_else(|| anyhow!("No grams found"))
    }
    
    fn get_postings(&self, gram_hash: GramHash) -> Result<Vec<DocId>> {
        // Binary search in gram table
        let entry = self.gram_table
            .binary_search_by_key(&gram_hash, |e| e.gram_hash)
            .ok_or_else(|| anyhow!("Gram not found"))?;
        
        let entry = &self.gram_table[entry];
        let postings_data = &self.postings_data[entry.postings_offset as usize..];
        
        decode_delta_varint_postings(postings_data, entry.postings_count as usize)
    }
}
```

### Building

```rust
pub struct PhraseIndexV2Builder {
    postings: FxHashMap<GramHash, Vec<DocId>>,
    gram_len: usize,
}

impl PhraseIndexV2Builder {
    pub fn new(gram_len: usize) -> Self {
        Self {
            postings: FxHashMap::default(),
            gram_len,
        }
    }
    
    pub fn add_document(&mut self, doc_id: DocId, text: &str) {
        let grams = ngrams(text, self.gram_len);
        for gram in grams {
            let hash = hash_gram(&gram);
            self.postings.entry(hash).or_default().push(doc_id);
        }
    }
    
    pub fn build(self, doc_table: &DocumentTable, out: &Path) -> Result<()> {
        // Sort and deduplicate postings
        for postings in self.postings.values_mut() {
            postings.sort();
            postings.dedup();
        }
        
        // Write to file
        let mut file = File::create(out)?;
        
        // Write header
        let header = PhraseIndexV2Header { /* ... */ };
        file.write_all(unsafe { std::slice::from_raw_parts(
            &header as *const _ as *const u8,
            std::mem::size_of::<PhraseIndexV2Header>(),
        ) })?;
        
        // Write gram table (sorted by hash)
        let mut gram_entries: Vec<_> = self.postings
            .into_iter()
            .map(|(hash, postings)| (hash, postings))
            .collect();
        gram_entries.sort_by_key(|(h, _)| *h);
        
        for (hash, postings) in &gram_entries {
            let entry = GramEntry {
                gram_hash: *hash,
                postings_offset: 0,  // Will fill in
                postings_count: postings.len() as u32,
            };
            file.write_all(unsafe { std::slice::from_raw_parts(
                &entry as *const _ as *const u8,
                std::mem::size_of::<GramEntry>(),
            ) })?;
        }
        
        // Write compressed postings
        for (_, postings) in gram_entries {
            let compressed = encode_delta_varint_postings(&postings);
            file.write_all(&compressed)?;
        }
        
        Ok(())
    }
}
```

## Compression Strategy

### Delta-Varint Encoding

1. Sort doc_ids in ascending order
2. First value is the base doc_id (varint encoded)
3. Subsequent values are deltas (doc_id[i] - doc_id[i-1])
4. Each delta is varint encoded

This is efficient because:
- Sequential doc_ids have small deltas (1, 2, 3...)
- Varint encoding uses 1-5 bytes per value
- No need to store the full 32-bit doc_id for each posting

Example:
```
Original: [100, 101, 105, 106, 200]
Deltas:   [100, 1, 4, 1, 94]
Varint:   [0x64, 0x01, 0x04, 0x01, 0x5E]
```

## Migration Path

1. Keep PhraseIndex v1 unchanged for backward compatibility
2. Add PhraseIndexV2 as a new module
3. CLI commands support both formats via schema detection
4. Tools to convert v1 to v2
5. Gradual migration of existing indexes

## Performance Considerations

### Memory Usage

- **v1**: Stores passage_id strings (high memory)
- **v2**: Stores doc_ids (4 bytes each) + compressed postings

Estimated reduction: 60-80% memory savings for large corpora

### Lookup Speed

- **v1**: HashMap lookup + string comparison
- **v2**: Binary search + integer comparison

Expected improvement: 2-3x faster for gram lookups

### Build Time

- **v1**: HashMap insertion with strings
- **v2**: HashMap insertion with integers + compression

Expected: Similar or slightly slower due to compression

## Future Enhancements

1. **Bloom filter**: Add bloom filter for fast "gram exists" checks
2. **Tiered postings**: Separate hot/cold postings for cache efficiency
3. **Compression**: Use zstd for additional compression on postings
4. **Index sharding**: Support for sharded phrase indexes
5. **Hybrid indexing**: Combine with TF-IDF for ranked results

## Compatibility Notes

- PhraseIndex v1 remains the default for existing installations
- PhraseIndex v2 is opt-in via CLI flag
- Conversion tool provided for migrating v1 to v2
- Both formats can coexist in the same codebase
