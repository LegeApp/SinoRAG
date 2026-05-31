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
- **Discovery from a seed passage (candidates, not evidence)**: prefer
  `frontier` after one or two good exact hits. It does not use the vector index;
  it combines TF-IDF similar passages with distinctive phrase frontiers and is
  usually the best bridge from known wording to unknown leads.
- **Semantic/vector discovery**: `vector-neighbors`, `hybrid-discover`. Use
  only when conceptual drift is worth the extra latency and the seed passage is
  representative. Vector hits are leads, not evidence; verify them with exact
  tools before making claims.
- **Fast lexical parallels**: `similar` for TF-IDF neighbors from a known
  passage, especially reuse/retelling candidates.
- **Scoped corpus reads**: `passage`, `source-read`, `expand-context-adaptive`,
  `heading-search`, `outline-search`.
- **Distinctive vocabulary / comparison**: `compare-usage`, `scope-profile`,
  `collocation-search`, `trace-term-usage`.
- **Absence / clustering**: `absence-check`, `cluster-hits`.
- **Variant expansion**: `query-expand-terms` (no corpus deps).
- **Person research**: `person-resolve` then `person-history`.
- **Term-pair co-occurrence**: `pair-appearance` (individual passage evidence);
  `pair-profile` (aggregate rates by period, canon, or work).
- **Citation verification**: `citation-verify` to check whether a claimed quote
  appears in the corpus.
- **Batch execution** (gated by `--writable`): `run-batch` — submit an inline
  job array or a JSONL plan file; all results are written to a JSONL output
  file; use `depends_on` for DAG ordering and `concurrency` for parallel steps.
- **Output / write** (gated; only available when MCP is launched with
  `--writable`): `graph-build`, `report-build`, `report-from-evidence`,
  `pdf-build`, `validate-adjudication`.

**Principle**: exact evidence before discovery. Start with `search`,
`evidence-search`, `works`, or `heading-search` when the user gives a phrase,
person, title, work, or doctrine. Read the best hit with `passage` or
`source-read`. Then, unless the user asked only for direct lookup, run
`frontier` on the best seed passage to discover phrases and nearby source
candidates the user did not know to search for. Use `cluster-hits` or
`trace-term-usage` once you have many hits and need distribution rather than
more snippets.

Treat every discovery result (`frontier`, `similar`, `vector-neighbors`,
`hybrid-discover`, `source-investigate`) as a candidate lead. Convert it into
evidence only after exact verification and close reading.

---

## Output discipline

- Every accepted claim must cite a `passage_id` returned by a tool. No
  invented citations.
- Tool errors with `kind = missing_artifact` mean the relevant index has
  not been built; surface this to the user rather than guessing.
- Prefer JSON-ready output when the next consumer is another tool; prefer
  prose when the next consumer is the user.
