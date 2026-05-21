# SinoRAG MCP Server

This server exposes the SinoRAG corpus-research toolset over MCP. Each tool
returns a JSON envelope `{ ok, tool, result | error, meta }`. Read `error.kind`
when `ok` is false — common kinds include `unknown_tool`, `readonly_violation`,
`admin_tool_disabled`, and `missing_artifact`.

All corpus access goes through this server's tools — do not shell out to
`sinorag` directly when an MCP tool exists for the same operation.

Use `tool-docs` (or the MCP tool list) to discover available tools; their
input/output schemas are authoritative. The rest of this file is doctrine,
not a tool catalog.

---

## Tool-selection heuristics

- **Exact phrase evidence**: `search`, `evidence-search`, `phrase-index-search`,
  `canonical-source`, `first-attestation`, `phrase-history`.
- **Discovery (candidates, not evidence)**: `similar`, `vector-neighbors`,
  `hybrid-discover`, `frontier`, `source-investigate`.
- **Scoped corpus reads**: `passage`, `source-read`, `expand-context-adaptive`,
  `heading-search`, `outline-search`.
- **Distinctive vocabulary / comparison**: `compare-usage`, `scope-profile`,
  `collocation-search`, `trace-term-usage`.
- **Absence / clustering**: `absence-check`, `cluster-hits`.
- **Variant expansion**: `query-expand-terms` (no corpus deps).
- **Workflow planning**: `plan-tools` for "what should I do next" style tasks.
- **Person research**: `person-resolve` then `person-history`.
- **Term-pair co-occurrence**: `pair-appearance` (individual passage evidence);
  `pair-profile` (aggregate rates by period, canon, or work).
- **Citation verification**: `citation-verify` to check whether a claimed quote
  appears in the corpus.
- **Output / write** (gated; only available when MCP is launched with
  `--writable`): `graph-build`, `report-build`, `report-from-evidence`,
  `pdf-build`, `validate-adjudication`.

**Principle**: exact evidence before discovery. Confirm a phrase exists with
`search` / `evidence-search` first; only then chase neighbors with
`vector-neighbors` or `similar`. Treat candidate results as leads to verify,
never as direct evidence.

---

## Output discipline

- Every accepted claim must cite a `passage_id` returned by a tool. No
  invented citations.
- Tool errors with `kind = missing_artifact` mean the relevant index has
  not been built; surface this to the user rather than guessing.
- Prefer JSON-ready output when the next consumer is another tool; prefer
  prose when the next consumer is the user.
