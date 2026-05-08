//! Tokenization, IDF weighting, and similarity scoring against the
//! indexed mission corpus. Pure keyword retrieval — no embeddings.

use std::collections::{HashMap, HashSet};

use super::{IndexedMission, MissionHit, MissionMemoryIndex};

pub(crate) const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "of", "in", "to", "for", "with", "on", "at", "by",
    "from", "as", "is", "are", "was", "were", "be", "been", "this", "that", "these", "those", "it",
    "its", "not", "no", "i", "you", "we", "they", "he", "she", "do", "does", "did", "have", "has",
    "had", "will", "would", "should", "could", "can", "may", "might", "must", "so", "if", "then",
    "than", "also", "only", "just", "like", "into", "out", "up", "down", "over", "under", "off",
    "per", "via",
];

/// Lowercased, stopword-filtered tokens (length ≥ 2). Splits on Unicode
/// non-word boundaries (keeping `_`/`-`) and additionally emits each
/// snake_case sub-token so `some_thing` matches `thing` queries.
pub(crate) fn tokenize(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    for raw in lower.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-') {
        if raw.is_empty() {
            continue;
        }
        let mut pieces: Vec<String> = vec![raw.to_string()];
        let sub: Vec<&str> = raw.split('_').collect();
        if sub.len() > 1 {
            for s in &sub {
                pieces.push((*s).to_string());
            }
        }
        for piece in pieces {
            if piece.len() < 2 {
                continue;
            }
            if STOPWORDS.contains(&piece.as_str()) {
                continue;
            }
            out.push(piece);
        }
    }
    out
}

pub(crate) fn tokens_from_paths(paths: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for p in paths {
        for part in p.split(['/', '\\', '.']) {
            if part.is_empty() {
                continue;
            }
            for tok in tokenize(part) {
                out.push(tok);
            }
            // Also keep the full part when non-trivial (e.g. "nit-gol").
            let lower = part.to_lowercase();
            if lower.len() >= 2 && !STOPWORDS.contains(&lower.as_str()) {
                out.push(lower);
            }
        }
    }
    out
}

/// Public wrapper around the internal path tokenizer, used at planner
/// call sites to derive scope-file tokens without exposing crate
/// internals.
pub fn path_tokens(paths: &[String]) -> Vec<String> {
    tokens_from_paths(paths)
}

/// Smoothed inverse document frequency: `ln((N + 1) / (df + 1)) + 1`.
/// Smoothing avoids `log(0)`/`log(inf)` singularities when `df` is 0 or
/// matches the corpus size. The `+ 1` offset keeps weights strictly
/// positive so matches always contribute.
pub(crate) fn idf_weight(df: usize, total: usize) -> f32 {
    if total == 0 || df == 0 {
        return 1.0;
    }
    ((total as f32 + 1.0) / (df as f32 + 1.0)).ln() + 1.0
}

/// For each unique term across all mission `tags`, count how many
/// missions contain it. Per-query work is `O(N * avg_tags)`, negligible
/// at typical corpus sizes.
fn document_frequencies(missions: &[IndexedMission]) -> HashMap<String, usize> {
    let mut df: HashMap<String, usize> = HashMap::new();
    for m in missions {
        let seen: HashSet<&String> = m.tags.iter().collect();
        for t in seen {
            *df.entry(t.clone()).or_insert(0) += 1;
        }
    }
    df
}

pub fn retrieve_similar(
    index: &MissionMemoryIndex,
    query: &str,
    scope_file_tokens: &[String],
    exclude: &[&str],
    k: usize,
) -> Vec<MissionHit> {
    let query_terms: HashSet<String> = tokenize(query)
        .into_iter()
        .chain(scope_file_tokens.iter().cloned())
        .collect();
    if query_terms.is_empty() {
        return Vec::new();
    }
    let query_path_stems: HashSet<String> = scope_file_tokens.iter().cloned().collect();

    // IDF-weighted Jaccard: rare terms count more than common ones. df is
    // computed lazily over the full corpus (not the filtered subset) so
    // weights are stable across calls with different excludes.
    let total = index.missions.len();
    let df = document_frequencies(&index.missions);

    let mut hits: Vec<MissionHit> = index
        .missions
        .iter()
        .filter(|m| !exclude.contains(&m.mission_id.as_str()))
        .filter_map(|m| score_mission(m, &query_terms, &query_path_stems, &df, total))
        .collect();

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.mission.updated_at.cmp(&a.mission.updated_at))
    });
    hits.truncate(k);
    hits
}

fn score_mission(
    m: &IndexedMission,
    query_terms: &HashSet<String>,
    query_path_stems: &HashSet<String>,
    df: &HashMap<String, usize>,
    total: usize,
) -> Option<MissionHit> {
    let mission_terms: HashSet<String> = m.tags.iter().cloned().collect();
    if mission_terms.is_empty() {
        return None;
    }
    let overlap_terms: Vec<&String> = query_terms.intersection(&mission_terms).collect();
    if overlap_terms.is_empty() {
        return None;
    }
    let weighted_overlap: f32 = overlap_terms
        .iter()
        .map(|t| idf_weight(*df.get(*t).unwrap_or(&0), total))
        .sum();
    let weighted_union: f32 = query_terms
        .union(&mission_terms)
        .map(|t| idf_weight(*df.get(t).unwrap_or(&0), total))
        .sum();
    let jaccard = if weighted_union > 0.0 {
        weighted_overlap / weighted_union
    } else {
        0.0
    };

    let title_terms: HashSet<String> = tokenize(&m.title).into_iter().collect();
    let title_overlap = query_terms.intersection(&title_terms).count() as f32;
    let title_boost = (title_overlap * 0.1).min(0.3);

    let mission_path_stems: HashSet<String> =
        tokens_from_paths(&m.files_touched).into_iter().collect();
    let path_overlap = query_path_stems.intersection(&mission_path_stems).count() as f32;
    let path_bonus = (path_overlap * 0.05).min(0.2);

    Some(MissionHit {
        mission: m.clone(),
        score: jaccard + title_boost + path_bonus,
    })
}
