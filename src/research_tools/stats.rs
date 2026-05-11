use rustc_hash::FxHashMap;

#[derive(Debug, Clone, serde::Serialize)]
pub struct DistinctiveTerm {
    pub term_hash: u64,
    pub term_display: Option<String>,
    pub score: f32,
    pub a_count: u32,
    pub b_count: u32,
}

pub fn log_odds_distinctive_terms(
    a: &FxHashMap<u64, u32>,
    b: &FxHashMap<u64, u32>,
    top_k: usize,
) -> (Vec<DistinctiveTerm>, Vec<DistinctiveTerm>) {
    let alpha = 0.01f32;
    let a_total: u32 = a.values().sum::<u32>().max(1);
    let b_total: u32 = b.values().sum::<u32>().max(1);
    let mut vocab = rustc_hash::FxHashSet::default();
    vocab.extend(a.keys().copied());
    vocab.extend(b.keys().copied());
    let vocab_n = vocab.len().max(1) as f32;
    let mut scored = Vec::with_capacity(vocab.len());
    for term in vocab {
        let ac = *a.get(&term).unwrap_or(&0) as f32;
        let bc = *b.get(&term).unwrap_or(&0) as f32;
        let ap = (ac + alpha) / (a_total as f32 + alpha * vocab_n);
        let bp = (bc + alpha) / (b_total as f32 + alpha * vocab_n);
        scored.push(DistinctiveTerm {
            term_hash: term, term_display: None,
            score: (ap / bp).ln(),
            a_count: ac as u32, b_count: bc as u32,
        });
    }
    scored.sort_by(|x, y| y.score.partial_cmp(&x.score).unwrap());
    let a_top = scored.iter().take(top_k).cloned().collect();
    scored.sort_by(|x, y| x.score.partial_cmp(&y.score).unwrap());
    let b_top = scored.iter().take(top_k).cloned().map(|mut t| { t.score = -t.score; t }).collect();
    (a_top, b_top)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Collocate {
    pub term_hash: u64,
    pub display: Option<String>,
    pub score: f32,
    pub near_count: u32,
    pub background_count: u32,
}

pub fn score_collocates(
    near: &FxHashMap<u64, u32>,
    background: &FxHashMap<u64, u32>,
    top_k: usize,
) -> Vec<Collocate> {
    let near_total: u32 = near.values().sum::<u32>().max(1);
    let bg_total: u32 = background.values().sum::<u32>().max(1);
    let mut out = Vec::new();
    for (&term, &near_count) in near {
        if near_count < 2 { continue; }
        let bg_count = *background.get(&term).unwrap_or(&0);
        let near_p = (near_count as f32 + 1.0) / (near_total as f32 + 1.0);
        let bg_p = (bg_count as f32 + 1.0) / (bg_total as f32 + 1.0);
        out.push(Collocate {
            term_hash: term, display: None,
            score: (near_p / bg_p).ln(),
            near_count, background_count: bg_count,
        });
    }
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    out.truncate(top_k);
    out
}
