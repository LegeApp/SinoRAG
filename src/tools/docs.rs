use serde_json::{json, Value};

pub fn docs_payload(tool: Option<&str>) -> Value {
    if let Some(name) = tool {
        if let Some(doc) = DOCS.iter().find(|doc| doc.name == name) {
            return doc.to_value();
        }
        return json!({
            "error": "unknown_tool",
            "tool": name,
            "available": DOCS.iter().map(|doc| doc.name).collect::<Vec<_>>(),
        });
    }

    json!({
        "overview": "SinoRAG tools are JSON commands intended for agents and scripts. Start with status, then use search or heading-search for discovery. Use cluster-hits and trace-term-usage when you need structured distribution evidence.",
        "workflow": [
            "status: verify which indexes are available in the pack.",
            "search: quick exact phrase lookup across loaded passage text.",
            "heading-search: find section or heading names before searching inside a scope.",
            "cluster-hits: answer where the phrase occurs by work or division.",
            "trace-term-usage: answer how usage is distributed by period, canon, author, or work.",
            "passage or expand-context-adaptive: retrieve full context for selected hits."
        ],
        "important_distinctions": {
            "trace-term-usage_vs_cluster-hits": "trace-term-usage is for analytical distribution by metadata buckets such as period, canon, author, or work. cluster-hits is for navigational clustering by catalog outline nodes such as work or division, with representative passages.",
            "search_vs_outline-search": "search is corpus-wide by default and returns direct passage hits. outline-search searches within a catalog node when scoped, or groups corpus-wide hits when no scope is supplied.",
            "heading-search_vs_search": "heading-search matches heading and heading_path metadata, plus normalized passage text as a fallback. search matches phrase text in passages."
        },
        "tools": DOCS.iter().map(ToolDoc::to_value).collect::<Vec<_>>(),
    })
}

pub fn doc_for_tool(name: &str) -> Option<Value> {
    DOCS.iter()
        .find(|doc| doc.name == name)
        .map(ToolDoc::to_value)
}

struct ToolDoc {
    name: &'static str,
    purpose: &'static str,
    use_when: &'static str,
    notes: &'static str,
}

impl ToolDoc {
    fn to_value(&self) -> Value {
        json!({
            "name": self.name,
            "purpose": self.purpose,
            "use_when": self.use_when,
            "notes": self.notes,
        })
    }
}

const DOCS: &[ToolDoc] = &[
    ToolDoc { name: "status", purpose: "Report available corpus resources and indexes.", use_when: "Run first to see whether passages, phrase index, catalog, doc table, TF-IDF, and registry exist.", notes: "Read-only and cheap." },
    ToolDoc { name: "tool-docs", purpose: "Return this built-in documentation.", use_when: "Use when choosing a tool or explaining command differences.", notes: "Pass {\"tool\":\"search\"} for one tool or {} for the full guide." },
    ToolDoc { name: "search", purpose: "Quick corpus-wide exact phrase lookup.", use_when: "Use for ordinary text search across every loaded passage. Add mode=clusters, trace, or all for grouped summaries.", notes: "Layered: phrase index when available, parquet verification, parquet scan fallback. brief=true suppresses verbose representative metadata." },
    ToolDoc { name: "heading-search", purpose: "Find headings, section names, and heading paths.", use_when: "Use when the query is a title, case heading, section label, or when you need a work/section scope before text search.", notes: "Works with passages.parquet alone; catalog indexes are not required." },
    ToolDoc { name: "passage", purpose: "Retrieve one passage by passage_id.", use_when: "Use after search or cluster tools identify an exact passage.", notes: "Returns compact passage text and basic work metadata." },
    ToolDoc { name: "canonical-source", purpose: "Find canon-side source passages for a phrase.", use_when: "Use for source verification, citation dependence, and sutra-side candidates.", notes: "Filters toward rows with canon metadata." },
    ToolDoc { name: "validate-adjudication", purpose: "Validate adjudication JSON structure.", use_when: "Use before graph/report building from adjudication files.", notes: "Checks structure, not scholarly correctness." },
    ToolDoc { name: "graph-build", purpose: "Build an evidence graph from adjudication JSON.", use_when: "Use after adjudication is validated and you need graph artifacts.", notes: "Writes output files." },
    ToolDoc { name: "report-build", purpose: "Build a markdown report from adjudication and graph files.", use_when: "Use when producing a dossier/report artifact from completed evidence.", notes: "Writes output files." },
    ToolDoc { name: "works", purpose: "List works from the catalog.", use_when: "Use to identify work IDs and filter corpus areas by metadata.", notes: "Requires catalog.index." },
    ToolDoc { name: "catalog-index-info", purpose: "Show catalog index metadata.", use_when: "Use to inspect catalog coverage and availability.", notes: "Requires catalog.index." },
    ToolDoc { name: "similar", purpose: "Find TF-IDF similar passages to a seed passage.", use_when: "Use for text reuse or thematic similarity starting from a known passage.", notes: "Requires TF-IDF index and doc table." },
    ToolDoc { name: "frontier", purpose: "Generate a discovery frontier packet for an agent session.", use_when: "Use to expand from a seed passage into promising leads.", notes: "Combines similarity and phrase extraction." },
    ToolDoc { name: "first-attestation", purpose: "Find earliest loaded-corpus occurrence of a phrase.", use_when: "Use for historical ordering claims inside the loaded corpus.", notes: "Earliest means earliest by corpus period_rank, not absolute historical origin." },
    ToolDoc { name: "phrase-history", purpose: "Analyze phrase distribution across periods/canons/traditions.", use_when: "Use for historical spread and timeline-style summaries.", notes: "Can include variants if requested." },
    ToolDoc { name: "phrase-index-search", purpose: "Force phrase-index lookup for exact phrase hits.", use_when: "Use when you specifically want to validate the phrase index path.", notes: "Unlike search, this errors when the phrase index is missing." },
    ToolDoc { name: "seed-pick", purpose: "Pick unworked seed passages for research.", use_when: "Use to start an exploratory research run from candidate passages.", notes: "Can filter by tradition and period." },
    ToolDoc { name: "expand-context-adaptive", purpose: "Expand passage context by climbing the catalog tree.", use_when: "Use after selecting a hit and needing surrounding context under a character budget.", notes: "Requires catalog and doc table." },
    ToolDoc { name: "trace-term-usage", purpose: "Group phrase hits by metadata buckets.", use_when: "Use to answer how a phrase is distributed by period, canon, author, or work.", notes: "This is analytical distribution. For navigational work/division clusters, use cluster-hits." },
    ToolDoc { name: "query-expand-terms", purpose: "Generate variants, orthographic flips, and aliases.", use_when: "Use before searching when exact wording may vary.", notes: "Does not search by itself." },
    ToolDoc { name: "compare-usage", purpose: "Compare two sub-corpora and score distinctive terms.", use_when: "Use for differential vocabulary between scopes.", notes: "Requires catalog and doc table." },
    ToolDoc { name: "collocation-search", purpose: "Find terms that co-occur near a seed phrase.", use_when: "Use to identify local semantic or formulaic companions.", notes: "Scores terms near occurrences against background terms." },
    ToolDoc { name: "outline-search", purpose: "Search within or across catalog outline nodes and group hits.", use_when: "Use for scoped work/node searches, or corpus-wide grouped outline search if no scope is supplied.", notes: "Falls back to metadata grouping when no catalog/doc table exists and no scope is requested." },
    ToolDoc { name: "cluster-hits", purpose: "Cluster phrase hits by work or division.", use_when: "Use to answer where hits are concentrated in the corpus outline.", notes: "This is navigational clustering. For period/canon/author distributions, use trace-term-usage." },
    ToolDoc { name: "absence-check", purpose: "Check whether a phrase appears within a specific scope.", use_when: "Use for negative evidence in a work, canon, period, or node.", notes: "Absence is only meaningful for the loaded corpus and selected scope." },
];
