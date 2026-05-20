# Phase F1 — First batch of research tools

**Status:** in progress
**Risk:** low. Tools are additive; they compose existing primitives. No format changes.
**Estimated size:** ~1200 LOC.

## Why

The reviewer's tool list is the surface that turns the corpus + indexes into a real research assistant. F1 picks the four with highest leverage per dollar of code, all of which build on what exists today:

| Tool | Leans on |
|---|---|
| `expand-context-adaptive` | new catalog tree (D pt 1); shrinks to smallest coherent container that fits budget |
| `find-first-mention` | phrase index + doc_table.period_ranks |
| `trace-term-usage` | phrase index aggregated by period/canon/author/work |
| `query-expand-terms` | bundled variant dictionary + Unihan/CBETA seed list |

## Design rules

- Tools return **structured JSON**, never prose. The downstream LLM writes the prose.
- Every tool emits a `search_strategy` fragment in its output: filters used, expansion variants tried, candidate counts at each stage. Reproducibility / auditability.
- Tools never rely on heuristics that look at `passage_id` to recover work_id (Phase D pt 1 fixed this in the catalog; tools follow suit).
- Tools accept either `--pack` (resolved through `manifest.json`) or the existing per-path flags. `--pack` wins when both are set.
- New CLI commands are kebab-case: `expand-context-adaptive`, `find-first-mention`, `trace-term-usage`, `query-expand-terms`.

## Tool 1 — `expand-context-adaptive`

Inputs: `--passage-id <id>`, `--max-chars <budget>` (default 8000), `--mode auto|window|section|work` (default auto), optional `--before` / `--after` for window mode.

Auto mode picks the smallest coherent container that fits the budget by walking up the catalog tree:

1. Look up `doc_parent[passage_id]` → leaf `PassageRange` node.
2. While the node's `passage_count`-weighted byte estimate exceeds `max_chars`, climb to `parent_id`.
3. Stop at Work level (don't return whole corpora).
4. Return all passages in `[node.first_doc_id ..= node.last_doc_id]` from parquet.

Output:
```json
{
  "schema": "sinorag-expand-context-adaptive-v1",
  "seed_passage_id": "...",
  "selected_node_id": 1234,
  "selected_node_kind": "Division",
  "selected_label": "...",
  "passage_count": 42,
  "char_count": 7800,
  "passages": [ { "passage_id":..., "zh_text":..., "main_title":..., "from_lb":..., "to_lb":... }, ... ],
  "search_strategy": { "budget": 8000, "climbed_levels": 2, "mode": "auto" }
}
```

## Tool 2 — `find-first-mention`

Inputs: `--phrase <s>`, `--scope-canon`, `--scope-period`, `--scope-source-work-id` (optional filters), `--variants` (bool), `--limit` (default 50).

Algorithm:

1. Resolve phrase candidates via `PhraseIndex::candidate_ids`.
2. Verify by `strpos` in `zh_text_normalized` via DataFusion (existing pattern).
3. Join hits to `doc_table.period_ranks[doc_id]` and `doc_table.source_work_ids[doc_id]`.
4. Apply scope filters via catalog lookups (`work_id_map`, `period_rank` range).
5. Sort by `(period_rank, source_work_id, doc_id)` ascending; return the first `--limit`.

Output:
```json
{
  "schema": "sinorag-first-mention-v1",
  "phrase": "如是我聞",
  "variants_searched": ["如是我聞", "我聞如是"],
  "first": { "passage_id":..., "period":..., "period_rank":..., "source_work_id":..., "main_title":..., "from_lb":..., "zh_quote":... },
  "next_earlier": [...],  // up to limit-1 next-earliest
  "scope": { "canon":[], "period":[], "source_work_id":null },
  "search_strategy": { "candidates": N, "verified": M, "after_scope": K }
}
```

## Tool 3 — `trace-term-usage`

Inputs: `--phrase <s>`, `--group-by period|canon|author|work`, `--variants` (bool), `--scope-*` filters, `--limit-per-group` (default 5).

Algorithm:

1. Same phrase resolution as tool 2.
2. Group hits by the requested dimension.
3. Per group: hit count, work count, top-K representative passages (earliest by period_rank, then by doc_id).

Output:
```json
{
  "schema": "sinorag-term-usage-trace-v1",
  "phrase": "...",
  "group_by": "period",
  "groups": [
    { "key":"Tang", "hit_count":1234, "work_count":87, "top_works":[...], "representative_passages":[...] },
    ...
  ],
  "search_strategy": { ... }
}
```

## Tool 4 — `query-expand-terms`

Inputs: `--phrase <s>`, `--mode variants|orthographic|persons|all` (default all), `--max <N>` (default 10).

Algorithm (pragmatic, no LLM needed for the initial implementation):

1. Pull from a bundled small variant table (`src/templates/variants/` — JSON files keyed by base form).
2. For Han characters, also apply traditional ↔ simplified swap using a small handwritten table seeded with the most common ~200 mappings relevant to CBETA / kanripo (extend later if needed).
3. Optionally apply Unihan compatibility decomposition for known orthographic variants.
4. Person-name expansion uses the `aliases` field carried in the brief's `Seed::Person`.

Output:
```json
{
  "schema": "sinorag-query-expand-terms-v1",
  "input": "Amitabha",
  "expanded": ["阿彌陀", "阿弥陀", "無量壽", "無量光", "Amitābha"],
  "by_source": {
    "variants": ["阿弥陀"],
    "orthographic": ["阿彌陀"],
    "persons": ["無量壽", "無量光"]
  },
  "search_strategy": { "max": 10, "input_lang_guess": "en" }
}
```

The first-pass bundled variant table covers a few hundred high-frequency Buddhist terms (Amitabha, Bodhisattva, Buddha-nature, etc.) plus the common name pairs that appear across CBETA. Easy to extend over time.

## Touch list

```
src/templates/variants/buddhist_terms.json   (new, bundled via include_str!)
src/templates/variants/orthographic.json     (new, bundled via include_str!)
src/templates/variants/mod.rs                (new, loaders + lookup)
src/commands/expand_context_adaptive.rs      (new)
src/commands/find_first_mention.rs           (new)
src/commands/trace_term_usage.rs             (new)
src/commands/query_expand_terms.rs           (new)
src/commands/mod.rs                          (dispatch)
src/cli.rs                                   (4 new commands)
src/templates/mod.rs                         (declare variants mod)
```

## Validation checklist

- `sinorag expand-context-adaptive --passage-id <T48n2005-something> --max-chars 4000` returns a Division-level node when the budget can't fit the whole work.
- `sinorag find-first-mention --phrase "如是我聞"` returns a Tang-era or earlier hit with `period_rank` populated correctly.
- `sinorag trace-term-usage --phrase "頓悟" --group-by period` shows non-zero hit counts in at least 3 distinct periods.
- `sinorag query-expand-terms --phrase "Amitabha"` returns at least 3 variants.

## Out of scope (Phase F2)

`compare-usage`, `collocation-search`, `outline-search`, `cluster-hits`, `absence-check`. These get more sophisticated, especially `compare-usage` (TF-IDF distinctive-term ranking between two sub-corpora). Separate phase.
