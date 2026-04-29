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
    let base = pane_cwd
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| pane_cwd.to_path_buf());
    ParsedQuery {
        base,
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
mod tests {
    use super::*;

    #[test]
    fn empty_or_whitespace_input_resolves_to_parent_with_empty_needle() {
        let project = PathBuf::from("/Users/me/code/nit/crates");
        let from_empty = parse_query("", &project, None);
        let from_whitespace = parse_query("   ", &project, None);
        let from_root = parse_query("", Path::new("/"), None);

        assert_eq!(from_empty.base, PathBuf::from("/Users/me/code/nit"));
        assert_eq!(from_empty, from_whitespace);
        assert!(from_empty.needle.is_empty());
        assert_eq!(from_root.base, PathBuf::from("/"));
    }

    #[test]
    fn dotdot_chain_walks_up_each_level_independently() {
        let project = PathBuf::from("/a/b/c/d");
        let one = parse_query("../foo", &project, None);
        let two = parse_query("../../bar", &project, None);
        let three_blank = parse_query("../../../", &project, None);

        assert_eq!(one.base, PathBuf::from("/a/b/c"));
        assert_eq!(one.needle, "foo");
        assert_eq!(two.base, PathBuf::from("/a/b"));
        assert_eq!(two.needle, "bar");
        assert_eq!(three_blank.base, PathBuf::from("/a"));
        assert!(three_blank.needle.is_empty());
    }

    #[test]
    fn dotdot_underflow_clamps_at_root_and_keeps_needle() {
        let parsed = parse_query("../../../../foo", Path::new("/a/b"), None);
        assert_eq!(parsed.base, PathBuf::from("/"));
        assert_eq!(parsed.needle, "foo");

        let underflow_no_needle = parse_query("../../", Path::new("/single"), None);
        assert_eq!(underflow_no_needle.base, PathBuf::from("/"));
        assert!(underflow_no_needle.needle.is_empty());
    }

    #[test]
    fn lone_dotdot_or_inline_dots_are_treated_as_literal_needles() {
        let project = PathBuf::from("/repo");
        let dotdot_needle = parse_query("..foo", &project, None);
        let trailing_dots = parse_query("foo..bar", &project, None);
        let leading_dot = parse_query(".hidden", &project, None);

        assert_eq!(dotdot_needle.base, project);
        assert_eq!(dotdot_needle.needle, "..foo");
        assert_eq!(trailing_dots.needle, "foo..bar");
        assert_eq!(leading_dot.needle, ".hidden");
    }

    #[test]
    fn tilde_expansion_substitutes_home_then_descends() {
        let home = PathBuf::from("/Users/dev");
        let cwd = Path::new("/somewhere/else");
        let just_tilde = parse_query("~", cwd, Some(&home));
        let tilde_slash = parse_query("~/", cwd, Some(&home));
        let with_needle = parse_query("~/projects", cwd, Some(&home));
        let with_subpath = parse_query("~/Projects/nit", cwd, Some(&home));

        assert_eq!(just_tilde.base, home);
        assert_eq!(tilde_slash.base, home);
        assert!(tilde_slash.needle.is_empty());
        assert_eq!(with_needle.needle, "projects");
        assert_eq!(with_subpath.base, PathBuf::from("/Users/dev/Projects"));
        assert_eq!(with_subpath.needle, "nit");
    }

    #[test]
    fn tilde_without_known_home_uses_literal_marker_to_surface_failure() {
        let parsed = parse_query("~/foo", Path::new("/cwd"), None);
        assert_eq!(parsed.base, PathBuf::from("~"));
        assert_eq!(parsed.needle, "foo");

        let bare = parse_query("~", Path::new("/cwd"), None);
        assert_eq!(bare.base, PathBuf::from("~"));
        assert!(bare.needle.is_empty());
    }

    #[test]
    fn absolute_paths_split_at_the_last_separator() {
        let with_segment = parse_query("/abs/foo", Path::new("/cwd"), None);
        let trailing_slash = parse_query("/abs/foo/", Path::new("/cwd"), None);
        let single_segment = parse_query("/abs/", Path::new("/cwd"), None);
        let root_only = parse_query("/", Path::new("/cwd"), None);

        assert_eq!(with_segment.base, PathBuf::from("/abs"));
        assert_eq!(with_segment.needle, "foo");
        assert_eq!(trailing_slash, with_segment);
        assert_eq!(single_segment.base, PathBuf::from("/"));
        assert_eq!(single_segment.needle, "abs");
        assert_eq!(root_only.base, PathBuf::from("/"));
        assert!(root_only.needle.is_empty());
    }

    #[test]
    fn shell_metacharacters_pass_through_as_literal_needle_text() {
        let project = PathBuf::from("/repo");
        let glob = parse_query("foo*", &project, None);
        let bracket = parse_query("foo[bar]", &project, None);
        let tilde_mid = parse_query("foo~bar", &project, None);

        assert_eq!(glob.base, project);
        assert_eq!(glob.needle, "foo*");
        assert_eq!(bracket.needle, "foo[bar]");
        assert_eq!(tilde_mid.needle, "foo~bar");
    }

    #[test]
    fn rank_rewards_consecutive_matches_and_word_prefixes() {
        let consecutive = rank("foobar", "foo").expect("foo subsequence");
        let scattered = rank("fxxoxxoxxr", "foo").expect("foo subsequence");
        assert!(consecutive > scattered);

        let prefix = rank("test", "te").expect("te prefix");
        let mid_word = rank("untested", "te").expect("te mid-word");
        assert!(prefix > mid_word);
    }

    #[test]
    fn rank_penalises_haystack_length_when_match_quality_is_equal() {
        let short = rank("foo", "fo").expect("fo in foo");
        let long = rank("foooooooooooooo", "fo").expect("fo in long");
        let very_long = rank("fooooooooooooooooooooooooooo", "fo").expect("fo");

        assert!(short > long, "short ({short}) > long ({long})");
        assert!(long > very_long, "long ({long}) > very_long ({very_long})");
    }

    #[test]
    fn rank_is_case_insensitive_in_haystack_and_needle() {
        let lower = rank("crates", "cr").unwrap();
        let upper = rank("CRATES", "cr").unwrap();
        let mixed_needle = rank("crates", "CR").unwrap();
        let both_mixed = rank("CrAtEs", "Cr").unwrap();

        assert_eq!(lower, upper);
        assert_eq!(lower, mixed_needle);
        assert_eq!(lower, both_mixed);
    }

    #[test]
    fn rank_returns_none_for_non_subsequence_and_zero_for_empty_needle() {
        assert!(rank("alphabet", "z").is_none());
        assert!(rank("alphabet", "ba").is_none(), "out-of-order needle");
        assert!(
            rank("", "x").is_none(),
            "empty haystack with non-empty needle"
        );
        assert_eq!(rank("any-name", ""), Some(0));
        assert_eq!(rank("", ""), Some(0));
    }
}
