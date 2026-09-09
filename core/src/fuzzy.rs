//! Fuzzy matching and ranking for command-palette style searches, built on
//! the `code-fuzzy-match` crate. The matcher only accepts candidates that
//! contain every character of the query in order, and scores matches at the
//! start of words (as words appear in code and paths) and runs of
//! consecutive matches higher.

use code_fuzzy_match::FuzzyMatcher;
use std::cmp::Reverse;

/// One ranked search result: the index of the candidate and its score
/// (higher is better).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ranked {
    pub index: usize,
    pub score: usize,
}

/// Rank `candidates` against `query`. Candidates that don't match are
/// dropped; the rest are returned best first, with ties keeping their
/// original order so callers can pass candidates in a sensible default
/// order (most recently used, alphabetical, ...).
///
/// An empty query matches everything with a score of zero, preserving the
/// candidates' order.
pub fn rank<S: AsRef<str>>(candidates: impl IntoIterator<Item = S>, query: &str) -> Vec<Ranked> {
    let mut matcher = FuzzyMatcher::new();
    let mut results: Vec<Ranked> = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            let score = if query.is_empty() {
                0
            } else {
                matcher.fuzzy_match(candidate.as_ref(), query)?
            };
            Some(Ranked { index, score })
        })
        .collect();
    results.sort_by_key(|r| Reverse(r.score));
    results
}

/// Added to the score of a candidate whose label matches, so that any label
/// match outranks any match found only in the full text.
const LABEL_MATCH_BONUS: usize = 1 << 20;

/// Rank candidates that each have a short label inside a longer text, such
/// as a file name within its path. A query that matches the label ranks
/// above one that only matches somewhere in the full text, so searching
/// for `function` lists `core/function.cpp` before `tests/test_function.py`
/// and both before `functions/other.rs`. Among equal scores, shorter texts
/// come first, then the original order.
///
/// The full text is matched when the label isn't, so a query containing a
/// directory (`core/func`) still works. As with [`rank`], an empty query
/// matches everything in the original order.
pub fn rank_labeled<S: AsRef<str>>(
    candidates: impl IntoIterator<Item = (S, S)>,
    query: &str,
) -> Vec<Ranked> {
    let mut matcher = FuzzyMatcher::new();
    let mut results: Vec<(Ranked, usize)> = candidates
        .into_iter()
        .enumerate()
        .filter_map(|(index, (label, text))| {
            let text = text.as_ref();
            let score = if query.is_empty() {
                0
            } else if let Some(score) = matcher.fuzzy_match(label.as_ref(), query) {
                score + LABEL_MATCH_BONUS
            } else {
                matcher.fuzzy_match(text, query)?
            };
            Some((Ranked { index, score }, text.len()))
        })
        .collect();
    results.sort_by_key(|(r, len)| (Reverse(r.score), *len));
    results.into_iter().map(|(r, _)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indices(candidates: &[&str], query: &str) -> Vec<usize> {
        rank(candidates, query)
            .into_iter()
            .map(|r| r.index)
            .collect()
    }

    fn labeled<'a>(paths: &[&'a str], query: &str) -> Vec<&'a str> {
        let candidates = paths.iter().map(|p| (p.rsplit('/').next().unwrap(), *p));
        rank_labeled(candidates, query)
            .into_iter()
            .map(|r| paths[r.index])
            .collect()
    }

    #[test]
    fn empty_query_keeps_order() {
        assert_eq!(indices(&["b", "a", "c"], ""), vec![0, 1, 2]);
    }

    #[test]
    fn drops_non_matches_and_ranks_word_starts_first() {
        let files = ["src/main.rs", "docs/notes.txt", "core/src/editor.rs"];
        let ranked = indices(&files, "ed");
        assert_eq!(ranked[0], 2, "the word start in editor.rs wins");
        assert!(!ranked.contains(&1));
    }

    #[test]
    fn ties_keep_original_order() {
        assert_eq!(indices(&["foo.rs", "foo.rs"], "foo"), vec![0, 1]);
    }

    #[test]
    fn labeled_prefers_label_matches_then_shorter_paths() {
        let files = [
            "arch/tricore/src/function.rs",
            "core/function.cpp",
            "core/function/undoactions.cpp",
            "core/functiongraph.cpp",
            "tests/python/test_function.py",
            "ui/other.cpp",
        ];
        assert_eq!(
            labeled(&files, "function"),
            vec![
                "core/function.cpp",
                "core/functiongraph.cpp",
                "arch/tricore/src/function.rs",
                "tests/python/test_function.py",
                "core/function/undoactions.cpp",
            ]
        );
        assert_eq!(labeled(&files, "function.cpp")[0], "core/function.cpp");
        // A directory in the query matches against the whole path.
        assert_eq!(
            labeled(&files, "core/undo"),
            vec!["core/function/undoactions.cpp"]
        );
        assert_eq!(labeled(&files, "").len(), files.len());
    }
}
