//! Static legend for the three classifier-assigned metadata fields:
//! tradition, period, and origin.
//!
//! Every value produced by `tei::classify_tradition / classify_period /
//! classify_origin` has a stable integer ID.  Callers may supply either the
//! exact string name ("Chan/Zen") **or** the integer ID ("1") wherever a
//! filter accepts one of these fields.  Use `resolve_id` to convert.

pub struct TraditionEntry {
    pub id: u8,
    pub name: &'static str,
    pub description: &'static str,
}

pub struct PeriodEntry {
    pub id: u8,
    pub name: &'static str,
    pub approx_dates: &'static str,
    pub period_rank: i32,
}

pub struct OriginEntry {
    pub id: u8,
    pub name: &'static str,
}

// ---------------------------------------------------------------------------
// Tradition IDs  (must match every string pushed in tei::classify_tradition)
// ---------------------------------------------------------------------------
pub const TRADITIONS: &[TraditionEntry] = &[
    TraditionEntry {
        id: 1,
        name: "Chan/Zen",
        description: "Chan/Zen school texts; contains 禪/禅/chan/zen",
    },
    TraditionEntry {
        id: 2,
        name: "Pure Land",
        description: "Pure Land / Amitabha devotion",
    },
    TraditionEntry {
        id: 3,
        name: "Tiantai",
        description: "Tiantai school; Fahua-based",
    },
    TraditionEntry {
        id: 4,
        name: "Huayan",
        description: "Huayan / Avatamsaka school",
    },
    TraditionEntry {
        id: 5,
        name: "Vinaya",
        description: "Monastic discipline / Pratimoksha",
    },
    TraditionEntry {
        id: 6,
        name: "Madhyamaka",
        description: "Madhyamaka / Zhongguan philosophy",
    },
    TraditionEntry {
        id: 7,
        name: "Yogacara",
        description: "Yogacara / Consciousness-Only (唯識)",
    },
    TraditionEntry {
        id: 8,
        name: "Esoteric",
        description: "Esoteric / Tantric / Mijiao",
    },
    TraditionEntry {
        id: 9,
        name: "Commentarial",
        description: "Commentaries, sub-commentaries, treatises",
    },
    TraditionEntry {
        id: 10,
        name: "Historical",
        description: "Historical / biographical literature (史傳)",
    },
    TraditionEntry {
        id: 11,
        name: "General/Unspecified",
        description: "No dominant tradition detected",
    },
];

// ---------------------------------------------------------------------------
// Period IDs  (must match every string returned by tei::classify_period)
// ---------------------------------------------------------------------------
pub const PERIODS: &[PeriodEntry] = &[
    PeriodEntry {
        id: 1,
        name: "Pre-Tang",
        approx_dates: "before 618",
        period_rank: 4,
    },
    PeriodEntry {
        id: 2,
        name: "Sui",
        approx_dates: "581–618",
        period_rank: 5,
    },
    PeriodEntry {
        id: 3,
        name: "Tang",
        approx_dates: "618–907",
        period_rank: 6,
    },
    PeriodEntry {
        id: 4,
        name: "Song",
        approx_dates: "960–1279",
        period_rank: 8,
    },
    PeriodEntry {
        id: 5,
        name: "Yuan",
        approx_dates: "1271–1368",
        period_rank: 9,
    },
    PeriodEntry {
        id: 6,
        name: "Ming",
        approx_dates: "1368–1644",
        period_rank: 10,
    },
    PeriodEntry {
        id: 7,
        name: "Qing",
        approx_dates: "1644–1912",
        period_rank: 11,
    },
    PeriodEntry {
        id: 8,
        name: "Modern",
        approx_dates: "post-1912",
        period_rank: 99,
    },
    PeriodEntry {
        id: 9,
        name: "Unknown Period",
        approx_dates: "unclassified",
        period_rank: 99,
    },
];

// ---------------------------------------------------------------------------
// Origin IDs  (must match every string returned by tei::classify_origin)
// ---------------------------------------------------------------------------
pub const ORIGINS: &[OriginEntry] = &[
    OriginEntry {
        id: 1,
        name: "India",
    },
    OriginEntry {
        id: 2,
        name: "Central Asia",
    },
    OriginEntry {
        id: 3,
        name: "China",
    },
    OriginEntry {
        id: 4,
        name: "Korea",
    },
    OriginEntry {
        id: 5,
        name: "Japan",
    },
    OriginEntry {
        id: 6,
        name: "Unknown Origin",
    },
];

// ---------------------------------------------------------------------------
// ID resolution helpers
// ---------------------------------------------------------------------------

/// If `token` is a decimal integer, look it up in `TRADITIONS` and return
/// the canonical name; otherwise return the token unchanged.
pub fn resolve_tradition(token: &str) -> &str {
    if let Ok(id) = token.parse::<u8>() {
        if let Some(e) = TRADITIONS.iter().find(|e| e.id == id) {
            return e.name;
        }
    }
    token
}

/// If `token` is a decimal integer, look it up in `PERIODS`.
pub fn resolve_period(token: &str) -> &str {
    if let Ok(id) = token.parse::<u8>() {
        if let Some(e) = PERIODS.iter().find(|e| e.id == id) {
            return e.name;
        }
    }
    token
}

/// If `token` is a decimal integer, look it up in `ORIGINS`.
pub fn resolve_origin(token: &str) -> &str {
    if let Ok(id) = token.parse::<u8>() {
        if let Some(e) = ORIGINS.iter().find(|e| e.id == id) {
            return e.name;
        }
    }
    token
}

// ---------------------------------------------------------------------------
// JSON serialisation helpers (for taxonomy command output)
// ---------------------------------------------------------------------------

pub fn traditions_json() -> serde_json::Value {
    serde_json::Value::Array(
        TRADITIONS
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "name": e.name,
                    "description": e.description,
                })
            })
            .collect(),
    )
}

pub fn periods_json() -> serde_json::Value {
    serde_json::Value::Array(
        PERIODS
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "name": e.name,
                    "approx_dates": e.approx_dates,
                    "period_rank": e.period_rank,
                })
            })
            .collect(),
    )
}

pub fn origins_json() -> serde_json::Value {
    serde_json::Value::Array(
        ORIGINS
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "name": e.name,
                })
            })
            .collect(),
    )
}
