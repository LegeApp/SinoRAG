# SinoRAG MCP Server

This server exposes the SinoRAG corpus-research toolset over MCP. Each tool
returns a JSON envelope `{ ok, tool, result | error, meta }`. Read `error.kind`
when `ok` is false — common kinds include `unknown_tool`, `readonly_violation`,
`admin_tool_disabled`, and `missing_artifact`.

## When to reach for which tool (rough map)

- **Exact phrase evidence**: `search`, `evidence-search`, `phrase-index-search`,
  `canonical-source`, `first-attestation`, `phrase-history`.
- **Discovery / "what is near this passage"**: `similar`, `vector-neighbors`,
  `hybrid-discover`, `frontier`, `source-investigate`.
- **Scoped corpus reads**: `passage`, `source-read`, `expand-context-adaptive`,
  `heading-search`, `outline-search`.
- **Comparison / distinctive vocabulary**: `compare-usage`, `scope-profile`,
  `collocation-search`, `trace-term-usage`.
- **Absence and clustering**: `absence-check`, `cluster-hits`.
- **Variant expansion**: `query-expand-terms` (no corpus required).
- **Catalog metadata**: `works`, `catalog-index-info`, `vector-info`, `status`,
  `tool-docs`.
- **Workflow / planning**: `plan-tools` returns recommended next calls for a
  research task description.
- **Output / write tools** (only available when readonly is off):
  `graph-build`, `report-build`, `report-from-evidence`,
  `validate-adjudication`.

Prefer **exact evidence before discovery**: confirm a phrase exists with
`search` / `evidence-search`, then run discovery tools to find adjacent
material. `vector-neighbors` and `similar` return *candidates*, not evidence.

## Canonical-Dependence lens (when applicable)

The `canonical-dependence` research lens asks how a Chan/Zen passage depends
on, quotes, cites, or echoes the broader Buddhist canon — sutras, sastras,
vinaya, translation-era doctrinal phrases, named figures, distinctive
formulaic wording.

**In scope**: explicit citation of a sutra/sastra; quotation of a phrase
found in canonical text; distinctive doctrinal phrase traceable to a
canonical source; named canonical figure in a sourceable context.

**Out of scope**: Zen-to-Zen case genealogy; later koan retellings; Chan
phrase reuse without a canonical target; bare generic vocabulary
(菩提, 般若, 涅槃, 三昧) absent a distinctive co-text.

## Output discipline

Every accepted claim must cite a passage_id returned by a tool. Do not invent
citations. If a tool returns `missing_artifact`, surface that to the user
rather than guessing — the corresponding index has not been built.
