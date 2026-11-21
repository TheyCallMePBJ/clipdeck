//! Fuzzy filtering with nucleo's matcher (the engine behind the `nucleo`
//! crate, without its background threads, which a list of a few hundred
//! entries doesn't need).
//!
//! Queries use fzf syntax: space-separated terms must all match, and
//! `'exact`, `^prefix`, `suffix$` and `!not` work as in fzf.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// One entry that matched the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Index into the list that was searched.
    pub index: usize,
    /// Positions of the matched characters, for highlighting. Sorted and
    /// without duplicates. Empty when the query is empty.
    pub highlights: Vec<u32>,
}

pub struct Searcher {
    matcher: Matcher,
    /// Scratch space reused between calls, so filtering doesn't allocate
    /// for every entry.
    buf: Vec<char>,
}

impl Searcher {
    pub fn new() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT),
            buf: Vec::new(),
        }
    }

    /// Returns the haystacks that match `query`, best match first. Entries
    /// with equal scores keep their original (newest first) order. An empty
    /// query matches everything in the original order.
    ///
    /// `impl IntoIterator<Item = &str>` accepts any sequence of strings, so
    /// callers don't need to build a `Vec` first.
    pub fn filter<'a>(
        &mut self,
        query: &str,
        haystacks: impl IntoIterator<Item = &'a str>,
    ) -> Vec<Match> {
        // Smart case: case-insensitive unless the query has an uppercase letter.
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);

        let mut scored: Vec<(u32, Match)> = Vec::new();
        for (index, haystack) in haystacks.into_iter().enumerate() {
            let mut highlights = Vec::new();
            let text = Utf32Str::new(haystack, &mut self.buf);
            if let Some(score) = pattern.indices(text, &mut self.matcher, &mut highlights) {
                highlights.sort_unstable();
                highlights.dedup();
                scored.push((score, Match { index, highlights }));
            }
        }
        // `sort_by_key` is stable, so ties stay in recency order.
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        scored.into_iter().map(|(_, m)| m).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HISTORY: [&str; 5] = [
        "git commit -m 'fix'",
        "https://github.com/sentriz/cliphist",
        "cargo install --path .",
        "Hello World",
        "git checkout main",
    ];

    fn indices(query: &str) -> Vec<usize> {
        Searcher::new()
            .filter(query, HISTORY)
            .into_iter()
            .map(|m| m.index)
            .collect()
    }

    #[test]
    fn empty_query_matches_everything_in_order() {
        assert_eq!(indices(""), [0, 1, 2, 3, 4]);
        assert_eq!(indices("   "), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn fuzzy_matches_out_of_order_gaps() {
        // "gco" matches "git checkout", "git commit" and "github.com".
        let mut found = indices("gco");
        found.sort();
        assert_eq!(found, [0, 1, 4]);
    }

    #[test]
    fn all_terms_must_match() {
        assert_eq!(indices("git main"), [4]);
    }

    #[test]
    fn smart_case() {
        assert_eq!(indices("hello"), [3]);
        assert_eq!(indices("HELLO"), Vec::<usize>::new());
        assert_eq!(indices("Hello"), [3]);
    }

    #[test]
    fn fzf_syntax_works() {
        assert_eq!(indices("^cargo"), [2]);
        assert_eq!(indices("'cliphist"), [1]);
        // "github" contains "git", so the URL is excluded too.
        let mut not_git = indices("!git");
        not_git.sort();
        assert_eq!(not_git, [2, 3]);
    }

    #[test]
    fn highlights_point_at_matched_chars() {
        let m = &Searcher::new().filter("wrld", ["Hello World"])[0];
        // W=6, r=8, l=9, d=10 (the "o" at 7 isn't in the query).
        assert_eq!(m.highlights, [6, 8, 9, 10]);
    }

    #[test]
    fn ties_keep_recency_order() {
        let found: Vec<_> = Searcher::new()
            .filter("abc", ["abc", "abc", "abc"])
            .into_iter()
            .map(|m| m.index)
            .collect();
        assert_eq!(found, [0, 1, 2]);
    }
}
