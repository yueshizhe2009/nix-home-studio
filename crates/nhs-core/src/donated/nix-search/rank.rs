//! Search relevance tiers.
//!
//! Donated from NixBox (`crates/nixbox-nix/src/search.rs`, commit `7bab22d`);
//! see `PROVENANCE.md`. The tier boundaries are unchanged, so a user who liked
//! how NixBox ranked results gets the same order here. Two mechanical edits:
//! slice indexing became `get(..)`, and the module no longer sits next to any
//! async code.

/// Where a hit landed, as a sortable key.
///
/// Ordering is the whole point: lower tiers first, then earlier matches, then
/// shorter names, then the name itself so the order is total and stable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Relevance {
    /// 0 exact name, 1 prefix, 2 token in a name, 3 substring in a name,
    /// 4 token in the description, 5 substring in the description, 6 no match.
    pub tier: u8,
    /// Byte position of the match; `usize::MAX` when there is none.
    pub position: usize,
    /// Length of `pname`, so `fd` outranks `fd-find` at the same tier.
    pub name_len: usize,
    /// `pname`, the final tie-break.
    pub name: String,
}

/// Normalise the query the way NixBox does: trim, drop `^`/`$` anchors, lowercase.
#[must_use]
pub fn ranking_term(query: &str) -> String {
    let query = query.trim();
    let query = query.strip_prefix('^').unwrap_or(query);
    query.strip_suffix('$').unwrap_or(query).to_lowercase()
}

/// Rank one hit. All three fields are expected to be lowercase already; use
/// [`ranking_term`] for the query.
#[must_use]
pub fn relevance_fields(attr: &str, pname: &str, description: &str, query: &str) -> Relevance {
    let names = [pname, attr];

    let (tier, position) = if query.is_empty() {
        (6, usize::MAX)
    } else if names.contains(&query) {
        (0, 0)
    } else if names.iter().any(|name| name.starts_with(query)) {
        (1, 0)
    } else if let Some(position) = names
        .iter()
        .filter_map(|name| token_position(name, query))
        .min()
    {
        (2, position)
    } else if let Some(position) = names.iter().filter_map(|name| name.find(query)).min() {
        (3, position)
    } else if let Some(position) = token_position(description, query) {
        (4, position)
    } else if let Some(position) = description.find(query) {
        (5, position)
    } else {
        (6, usize::MAX)
    };

    Relevance {
        tier,
        position,
        name_len: pname.chars().count(),
        name: pname.to_string(),
    }
}

/// First position where `needle` appears as a whole token.
///
/// "Token" means the characters on both sides are not alphanumeric, so `fd`
/// matches `fd-find` but not `fdisk`.
#[must_use]
pub fn token_position(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .match_indices(needle)
        .find_map(|(position, matched)| {
            let before = haystack
                .get(..position)
                .and_then(|head| head.chars().next_back());
            let after = haystack
                .get(position + matched.len()..)
                .and_then(|tail| tail.chars().next());
            let starts_token = before.is_none_or(|ch| !ch.is_alphanumeric());
            let ends_token = after.is_none_or(|ch| !ch.is_alphanumeric());
            (starts_token && ends_token).then_some(position)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier(attr: &str, pname: &str, description: &str, query: &str) -> u8 {
        relevance_fields(attr, pname, description, query).tier
    }

    #[test]
    fn tiers_follow_the_upstream_scale() {
        assert_eq!(tier("firefox", "firefox", "browser", "firefox"), 0);
        assert_eq!(tier("ripgrep", "ripgrep", "", "rip"), 1);
        // A prefix wins over a token: `fd` matches the start of `fd-find` (1).
        assert_eq!(tier("fd-find", "fd-find", "", "fd"), 1);
        // A token later in the name is tier 2 …
        assert_eq!(tier("fd-find", "fd-find", "", "find"), 2);
        // … and a substring that is not a token is tier 3.
        assert_eq!(tier("ripgrep", "ripgrep", "", "grep"), 3);
        assert_eq!(tier("x", "x", "a fast search tool", "search"), 4);
        assert_eq!(tier("x", "x", "a fast search tool", "ast sear"), 5);
        assert_eq!(tier("x", "x", "nothing here", "zzz"), 6);
        assert_eq!(tier("x", "x", "", ""), 6, "an empty query ranks nothing");
    }

    #[test]
    fn token_boundaries_matter() {
        assert_eq!(token_position("fd-find", "fd"), Some(0));
        assert_eq!(token_position("fdisk", "fd"), None);
        assert_eq!(token_position("my fd tool", "fd"), Some(3));
    }

    #[test]
    fn shorter_names_win_within_a_tier() {
        let short = relevance_fields("fd", "fd", "", "f");
        let long = relevance_fields("fdisk", "fdisk", "", "f");
        assert_eq!(short.tier, long.tier);
        assert!(short < long);
    }

    #[test]
    fn the_ranking_term_drops_anchors_and_case() {
        assert_eq!(ranking_term("  ^Firefox$ "), "firefox");
    }
}
