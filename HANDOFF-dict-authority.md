# Handoff: Dictionary & Authority Parquet System (Phases 2–4)

## What's done (Phase 1)

Dictionary parquet pipeline is complete and tested:

- `sinorag ingest-dict <dict-dir>` parses 8 JSON sources → `data/dict.parquet/source={name}/`
- 84,718 entries across: soothill (14.8k), dingfubao (32.7k), foguang (24k), agama (1k), fymyj (1k), pentaglot (851), ccc (4.4k), cyx (5.7k)
- `src/dict.rs` loads from parquet via DataFusion at first tool call, caches in `HashMap` for O(1) annotation
- `annotate_response()` is async, hooked into `call_tool()` in `src/tools/registry.rs:75`
- Soothill is fully disembedded — no more `include_bytes!` for dictionaries
- `src/pack.rs` has `DEFAULT_DICT`, `DEFAULT_PERSONS`, `DEFAULT_PLACES` constants ready

## What's next

### Phase 2: Person & place parquet ingest from DDBC XML

**Source files** (in `data/archives/cbeta/cbeta-reader/dict/`):
- `Buddhist_Studies_Person_Authority.xml` — 45MB, 46,668 `<person>` entries
- `Buddhist_Studies_Place_Authority.xml` — 15MB, 38,671 `<place>` entries

**Command:** `sinorag ingest-authority <dict-dir>` → writes `data/persons.parquet/` + `data/places.parquet/`

**Person schema** (see plan file for full spec):
```
person_id, primary_name, primary_name_lang, alt_names_json,
gender, dynasty, birth_year, death_year, occupation,
place_of_origin, concise_bio, teachers_json, students_json,
wikidata_id, cbdb_id
```

**Place schema:**
```
place_id, primary_name, primary_name_lang, alt_names_json,
latitude, longitude, geo_confidence, district, category,
description, parent_place_id
```

**Parser notes:**
- Both files use TEI XML without a default namespace (namespace is commented out in the files)
- Person entries are `<person xml:id="A000001">` inside `<listPerson>`
- Place entries are `<place xml:id="PL000000000001">` inside `<listPlace>`
- Names: multiple `<persName>`/`<placeName>` with `xml:lang` and optional `type="alternative"`
- Dates: `<birth>` and `<death>` with date ranges like `+0602-02-01 ~ +0603-02-18`
- Notes: `<note type="dynasty">`, `<note type="concise">`, `<note type="category">`
- Coordinates: `<geo cert="high">114.576442 38.142658</geo>` (lon lat format)
- Relationships: `<listRelation>` → `<relation type="teacher" active="A000295" n="玄奘">`
- Use `quick-xml` (already a dependency) — same parser as CBETA TEI ingest

**Files to create:**
- `src/commands/ingest_authority.rs` — TEI XML parser for both person and place
- `src/storage.rs` — add `PersonBatch`, `PlaceBatch`, `person_schema()`, `place_schema()`, write functions (follow `DictBatch` pattern exactly)

**Files to modify:**
- `src/cli.rs` — add `IngestAuthority { path, persons_out, places_out }` command
- `src/commands/mod.rs` — `pub mod ingest_authority;` + dispatch
- `src/tools/engine.rs` — add lazy-loaded person/place stores (follow `resolve_dict_path_static` pattern)

### Phase 3: Auto-annotate person/place in tool responses

Extend `src/dict.rs`:

1. Add `EntityStore` struct — `HashMap<String, EntityEntry>` where key is a name (primary + all alts), value is `{type: person|place, id, summary}`. Loaded from person/place parquet at first access, same `OnceCell` pattern as `DictStore`.

2. In `annotate_response()`, after the existing `_term_context` annotation, also scan for entity names and append `_entity_context`:
```json
"_entity_context": [
  {"type": "person", "name": "玄奘", "id": "A000294", "dynasty": "唐", "summary": "..."},
  {"type": "place", "name": "那爛陀", "id": "PL...", "category": "寺廟", "summary": "..."}
]
```

3. Ambiguity handling: if a string matches both a person AND a place (or a dict term), include ALL matches with their `type` labels. The model sees the disambiguation and can choose correctly.

4. Name length gating: only match names of 2+ characters. 2-char person names are common (慧能, 道元) but also have high false-positive rates as general vocabulary. Consider boosting 3+ char matches and demoting 2-char matches unless they appear in the `_term_context` too.

**Key function:** `set_dict_path()` in `dict.rs` already exists. Add parallel `set_person_path()` and `set_place_path()`, called from `ToolEngine::open()`.

### Phase 4: person-resolve overhaul + new place-resolve

**person-resolve overhaul** (`src/tools/engine.rs::person_resolve_impl`):
- Current implementation (line ~5947): searches passage text for name mentions, returns hit counts and first-hit samples. No authority data.
- New: query person parquet FIRST for exact match on `primary_name` and alt names (`alt_names_json LIKE '%{name}%'`). Return structured DDBC data (dates, dynasty, bio, teachers/students). THEN search passages for corpus evidence. Merge both into the response.
- Update `PersonResolveResponse` in `src/tools/responses.rs` to include DDBC fields.

**New place-resolve** (`src/tools/engine.rs::place_resolve_impl`):
- Same pattern as person-resolve: query place parquet for name match, return structured data (coordinates, category, description). Then search passages for contextual mentions.
- New types: `PlaceResolveRequest`, `PlaceResolveResponse` in requests/responses.
- Register `place-resolve` in `src/tools/registry.rs::tool_defs()` — follow the existing person-resolve registration pattern.

## Parquet compression note

The Parquet format supports exactly these codecs (from `parquet-58.2.0/src/basic.rs:818`):

```rust
pub enum Compression {
    UNCOMPRESSED,
    SNAPPY,
    GZIP(GzipLevel),
    LZO,
    BROTLI(BrotliLevel),
    ZSTD(ZstdLevel),
    LZ4,
    LZ4_RAW,
}
```

These are **page-level codecs baked into the Parquet spec** — every Parquet reader must support the codec used to write a file, so **PPMD-H and libbsc are not options for within-parquet compression**. The codec ID is a single integer in the page header (0=uncompressed, 1=snappy, ..., 6=zstd), and any non-standard value would make the files unreadable by DataFusion, DuckDB, Spark, Polars, or any other Parquet consumer.

However, arcmax can still add value as an **outer compression layer** for pack distribution:

1. **Parquet files use ZSTD internally** (our current setting) — this compresses individual column pages
2. **The pack tarball wraps all files** (parquet + indexes + dict) — arcmax PPMD-H compresses the tar
3. These are orthogonal: ZSTD handles page-level redundancy within parquet, arcmax handles cross-file and residual redundancy in the tar stream

If you want to maximize arcmax's effectiveness on the parquet files specifically, you could write parquet with `Compression::UNCOMPRESSED` and let arcmax handle all compression in the outer layer. This would give arcmax more data to work with (it sees raw column pages instead of ZSTD-compressed blobs). The tradeoff: uncompressed parquet is ~3-5x larger on disk before packing, so the working corpus takes more space until you build the pack. A `--pack-compression` flag on `build-pack` that optionally rewrites parquet to uncompressed before tarring would be the clean way to do this.

For now, ZSTD is the right default — it's fast, well-compressed, and every tool in the ecosystem reads it natively. The arcmax outer layer is additive.

## File inventory

Files created/modified in this session (for reference):

**New files:**
- `src/commands/merge_cbeta.rs` — three-way CBETA merge
- `src/commands/init.rs` — pack bootstrap
- `src/commands/ingest_dict.rs` — dictionary JSON → parquet
- `src/dict.rs` — parquet-backed dictionary annotation
- `THIRD_PARTY_NOTICES.md` — attribution for cbeta-reader + Soothill
- `HANDOFF-dict-authority.md` — this file
- `assets/cbeta/sutra_sch.lst` — embedded work catalog
- `assets/cbeta/cmp.lst` — embedded parallel works
- `assets/cbeta/STCharacters.txt`, `KXVariants.txt`, `JPVariants.txt` — normalization tables
- `assets/cbeta/soothill.json` — preprocessed Soothill-Hodous (for ingest, no longer embedded)

**Modified files:**
- `src/tei.rs` — `Merged` distribution, per-file detection, sidecar key normalization, `strip_fascicle_suffix` visibility
- `src/normalize.rs` — variant character normalization (4400+ mappings from 3 tables)
- `src/cbeta_sidecar.rs` — work catalog + parallel works sidecars
- `src/storage.rs` — `DictBatch` + dict parquet writing
- `src/tools/engine.rs` — dict path resolution in `ToolEngine::open()`
- `src/tools/registry.rs` — async `annotate_response` in `call_tool`
- `src/pack.rs` — `DEFAULT_DICT`, `DEFAULT_PERSONS`, `DEFAULT_PLACES`
- `src/cli.rs` — `MergeCbeta`, `Init`, `IngestDict` commands
- `src/commands/mod.rs` — module + dispatch wiring
- `src/lib.rs`, `src/main.rs` — `mod dict` registration
- `Cargo.toml` — `flate2` dependency
