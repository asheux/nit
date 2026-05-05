//! Pure parser + ranker for the per-pane directory-search overlay.
//!
//! Resolves operator text against a `(base, needle)` pair per the
//! table in `docs/MULTIPANE.md` ("Dir search modes"). Ranking
//! delegates to [`crate::fuzzy_search_runner::fuzzy_score_bytes`] so
//! the multipane dropdown shares the editor's subsequence-with-bonus
//! algorithm. Async filesystem walk lives in `dir_search_runner`.

use std::path::{Path, PathBuf};

use crate::fuzzy_search_runner::fuzzy_score_bytes;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedQuery {
    pub base: PathBuf,
    pub needle: String,
}

pub fn parse_query(input: &str, pane_cwd: &Path, home: Option<&Path>) -> ParsedQuery {
    let trimmed = input.trim_start();
    if trimmed.is_empty() {
        return parse_empty(pane_cwd);
    }
    if trimmed == "~" || trimmed.starts_with("~/") {
        let after = trimmed.strip_prefix('~').unwrap_or("");
        let after = after.strip_prefix('/').unwrap_or(after);
        return parse_tilde(after, home);
    }
    if let Some(rest) = trimmed.strip_prefix('/') {
        return descend_absolute(rest);
    }
    parse_relative(trimmed, pane_cwd)
}

pub fn rank(name: &str, needle: &str) -> Option<i64> {
    let name_lc = name.to_ascii_lowercase();
    let needle_lc = needle.to_ascii_lowercase();
    fuzzy_score_bytes(name_lc.as_bytes(), needle_lc.as_bytes()).map(|(score, _)| score)
}

fn parse_empty(pane_cwd: &Path) -> ParsedQuery {
    ParsedQuery {
        base: pane_cwd.to_path_buf(),
        needle: String::new(),
    }
}

fn parse_tilde(after: &str, home: Option<&Path>) -> ParsedQuery {
    match home {
        Some(home_path) => descend_into(home_path, after),
        None => ParsedQuery {
            base: PathBuf::from("~"),
            needle: after.to_string(),
        },
    }
}

fn parse_relative(trimmed: &str, pane_cwd: &Path) -> ParsedQuery {
    let (ups, after) = strip_dotdot_chain(trimmed);
    if ups == 0 {
        return ParsedQuery {
            base: pane_cwd.to_path_buf(),
            needle: after.to_string(),
        };
    }
    let mut base = pane_cwd.to_path_buf();
    for _ in 0..ups {
        if !base.pop() {
            break;
        }
    }
    if base.as_os_str().is_empty() {
        base = PathBuf::from("/");
    }
    ParsedQuery {
        base,
        needle: after.to_string(),
    }
}

fn strip_dotdot_chain(s: &str) -> (usize, &str) {
    let mut count = 0usize;
    let mut rest = s;
    while let Some(after) = rest.strip_prefix("../") {
        count += 1;
        rest = after;
    }
    if rest == ".." {
        count += 1;
        rest = "";
    }
    (count, rest)
}

fn descend_into(root: &Path, rest: &str) -> ParsedQuery {
    let trimmed = rest.trim_end_matches('/');
    if trimmed.is_empty() {
        return ParsedQuery {
            base: root.to_path_buf(),
            needle: String::new(),
        };
    }
    let Some(idx) = trimmed.rfind('/') else {
        return ParsedQuery {
            base: root.to_path_buf(),
            needle: trimmed.to_string(),
        };
    };
    let (parent, last) = trimmed.split_at(idx);
    ParsedQuery {
        base: root.join(parent),
        needle: last.trim_start_matches('/').to_string(),
    }
}

fn descend_absolute(rest: &str) -> ParsedQuery {
    let trimmed = rest.trim_end_matches('/');
    if trimmed.is_empty() {
        return ParsedQuery {
            base: PathBuf::from("/"),
            needle: String::new(),
        };
    }
    let Some(idx) = trimmed.rfind('/') else {
        return ParsedQuery {
            base: PathBuf::from("/"),
            needle: trimmed.to_string(),
        };
    };
    let (parent, last) = trimmed.split_at(idx);
    ParsedQuery {
        base: PathBuf::from("/").join(parent),
        needle: last.trim_start_matches('/').to_string(),
    }
}

#[cfg(test)]
#[path = "../tests/multipane_dir_search.rs"]
mod tests;
