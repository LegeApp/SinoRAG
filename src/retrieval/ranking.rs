use crate::retrieval::CandidateSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HybridRankProfile {
    Discovery,
}

pub fn rank_score(rank: Option<usize>) -> Option<f32> {
    rank.map(|r| 1.0 / r.max(1) as f32)
}

pub fn final_score(
    semantic_score: Option<f32>,
    lexical_score: Option<f32>,
    _profile: HybridRankProfile,
) -> f32 {
    match (semantic_score, lexical_score) {
        (Some(semantic), Some(lexical)) => 3.0 + semantic * 0.25 + lexical * 0.5,
        (None, Some(lexical)) => 2.0 + lexical * 0.5,
        (Some(semantic), None) => 1.0 + semantic * 0.25,
        (None, None) => 0.0,
    }
}

pub fn candidate_sources(vector_rank: Option<usize>, tfidf_rank: Option<usize>) -> Vec<CandidateSource> {
    let mut sources = Vec::new();
    if vector_rank.is_some() {
        sources.push(CandidateSource::Vector);
    }
    if tfidf_rank.is_some() {
        sources.push(CandidateSource::Tfidf);
    }
    sources
}

pub fn refresh_hybrid_scores(
    vector_rank: Option<usize>,
    tfidf_rank: Option<usize>,
) -> (Option<f32>, Option<f32>, f32, Vec<CandidateSource>) {
    let semantic_score = rank_score(vector_rank);
    let lexical_score = rank_score(tfidf_rank);
    let final_score = final_score(semantic_score, lexical_score, HybridRankProfile::Discovery);
    let sources = candidate_sources(vector_rank, tfidf_rank);
    (semantic_score, lexical_score, final_score, sources)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hybrid_scores_rank_overlap_above_lexical_above_semantic() {
        let overlap = final_score(
            rank_score(Some(10)),
            rank_score(Some(10)),
            HybridRankProfile::Discovery,
        );
        let lexical = final_score(None, rank_score(Some(1)), HybridRankProfile::Discovery);
        let semantic = final_score(rank_score(Some(1)), None, HybridRankProfile::Discovery);

        assert!(overlap > lexical);
        assert!(lexical > semantic);
    }

    #[test]
    fn hybrid_candidate_sources_are_explicit() {
        assert_eq!(
            candidate_sources(Some(1), Some(2)),
            vec![CandidateSource::Vector, CandidateSource::Tfidf]
        );
        assert_eq!(candidate_sources(None, Some(1)), vec![CandidateSource::Tfidf]);
    }
}
