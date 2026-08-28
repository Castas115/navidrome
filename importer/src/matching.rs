//! Resolving a playlist entry to a file on disk.
//!
//! Three tiers, best first:
//!   1. ISRC — an exact recording identifier, immune to spelling
//!   2. exact normalised `artist|title`
//!   3. fuzzy `artist|title`, but only when the artist half also agrees
//!
//! Tier 3 without the artist guard matches every cover and every same-named
//! song in the library, which is worse than not matching at all.

use std::sync::LazyLock;

use rapidfuzz::distance::indel;
use regex::Regex;
use unicode_normalization::UnicodeNormalization;

/// Present in Spotify's titles, absent from most local tags.
static PAREN_NOISE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[\(\[](feat|ft|with|con)\.?[^\)\]]*[\)\]]").unwrap());

static SUFFIX_NOISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\s*-\s*[^-]*\b(remaster(ed)?|remix|version|edit|mix|mono|stereo|\
deluxe|bonus|radio|single|album|anniversary)\b.*$",
    )
    .unwrap()
});

/// Fold to a comparable ASCII key. Lossy on purpose: "Beyoncé" and "Beyonce"
/// have to collide, and so do "Don't" and "Dont".
pub fn norm(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    // Spotify writes " – 2011 Remaster" with an en dash, which NFKD does not
    // fold to "-". Mapping the dashes first is what lets SUFFIX_NOISE below
    // actually fire on real Spotify titles.
    let dashed: String = text
        .chars()
        .map(|c| match c {
            '\u{2010}'..='\u{2015}' | '\u{2212}' | '\u{FE58}' | '\u{FF0D}' => '-',
            other => other,
        })
        .collect();

    // NFKD splits "é" into "e" + combining accent, so dropping non-ASCII leaves
    // the bare letter instead of deleting the character outright.
    let folded: String = dashed.nfkd().filter(char::is_ascii).collect();
    let folded = folded.to_ascii_lowercase();

    let stripped = PAREN_NOISE.replace_all(&folded, " ");
    let stripped = SUFFIX_NOISE.replace_all(&stripped, " ");

    stripped
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|tok| !tok.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn key(artist: &str, title: &str) -> String {
    format!("{}|{}", norm(artist), norm(title))
}

fn token_sort(text: &str) -> String {
    let mut tokens: Vec<&str> = text.split_whitespace().collect();
    tokens.sort_unstable();
    tokens.join(" ")
}

/// rapidfuzz's `fuzz.ratio`: normalised indel similarity, 0-100.
fn ratio(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 100.0;
    }
    indel::normalized_similarity(a.chars(), b.chars()) * 100.0
}

/// rapidfuzz's `fuzz.token_sort_ratio`: word order stops mattering, so
/// "Simon & Garfunkel" and "Garfunkel & Simon" score the same.
pub fn token_sort_ratio(a: &str, b: &str) -> f64 {
    ratio(&token_sort(a), &token_sort(b))
}

/// Upper bound on `ratio(a, b)` from the lengths alone.
///
/// ratio = 2·LCS / (len_a + len_b) and LCS can never exceed the shorter string,
/// so a wildly different length can be rejected without touching the DP. This
/// is what keeps the fuzzy sweep over thousands of keys cheap.
fn max_possible_ratio(len_a: usize, len_b: usize) -> f64 {
    let total = len_a + len_b;
    if total == 0 {
        return 100.0;
    }
    200.0 * len_a.min(len_b) as f64 / total as f64
}

pub fn best_match<'a>(
    query: &str,
    candidates: impl IntoIterator<Item = &'a String>,
    cutoff: f64,
) -> Option<(&'a String, f64)> {
    let sorted_query = token_sort(query);
    let qlen = sorted_query.chars().count();

    let mut best: Option<(&'a String, f64)> = None;
    for cand in candidates {
        if max_possible_ratio(qlen, cand.chars().count()) < cutoff {
            continue;
        }
        let score = ratio(&sorted_query, &token_sort(cand));
        if score >= cutoff && best.is_none_or(|(_, b)| score > b) {
            best = Some((cand, score));
        }
    }
    best
}

/// The artist half of an `artist|title` key.
pub fn artist_of(key: &str) -> &str {
    key.split('|').next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_folds_accents_and_case() {
        assert_eq!(norm("Beyoncé"), "beyonce");
        assert_eq!(norm("Don't Stop"), "don t stop");
    }

    #[test]
    fn norm_strips_feat_and_remaster_noise() {
        assert_eq!(norm("Empire State of Mind (feat. Alicia Keys)"), "empire state of mind");
        assert_eq!(norm("Come Together - 2019 Remaster"), "come together");
    }

    #[test]
    fn norm_keeps_hyphens_that_are_not_suffixes() {
        // No noise keyword after the dash, so the title survives intact.
        assert_eq!(norm("Jack-in-the-Box"), "jack in the box");
    }

    #[test]
    fn token_sort_ratio_ignores_word_order() {
        assert_eq!(token_sort_ratio("simon garfunkel", "garfunkel simon"), 100.0);
    }

    #[test]
    fn length_bound_never_rejects_a_real_match() {
        // The prefilter must be conservative: whenever it prunes, the true
        // score really was below the cutoff.
        let pairs = [("hello world", "hello world!"), ("abc", "abcdefghij")];
        for (a, b) in pairs {
            let bound = max_possible_ratio(a.chars().count(), b.chars().count());
            assert!(ratio(a, b) <= bound + 1e-9, "{a} vs {b}");
        }
    }

    #[test]
    fn best_match_respects_cutoff() {
        let cands = vec!["ice cube|it was a good day".to_string()];
        assert!(best_match("ice cube|it was a good day", &cands, 88.0).is_some());
        assert!(best_match("nirvana|smells like teen spirit", &cands, 88.0).is_none());
    }
}
