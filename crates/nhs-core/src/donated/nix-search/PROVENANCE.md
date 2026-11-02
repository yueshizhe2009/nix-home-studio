# nix-search ranking — donated from NixBox

| Field | Value |
| --- | --- |
| Upstream repository | https://github.com/SINGH-RAJVEER/nixbox |
| Upstream commit | `7bab22d13a6ab70b9dede01d359dd546bb489060` (2026-10-23) |
| Original path | `crates/nixbox-nix/src/search.rs` |
| Licence | Apache-2.0 (see `NOTICE` at the repository root) |
| Local path | `crates/nhs-core/src/donated/nix-search/rank.rs` |

## What was taken

`Relevance`, `relevance_fields`, `token_position` and the query normalisation
(`ranking_term`) — the ranking tiers that decide which package a search shows
first. The tier scale is unchanged:

| Tier | Meaning |
| --- | --- |
| 0 | the query equals the package name or the attribute |
| 1 | a name starts with the query |
| 2 | the query is a whole token inside a name |
| 3 | the query is a substring of a name |
| 4 | the query is a whole token in the description |
| 5 | the query is a substring of the description |
| 6 | no match (an empty query puts everything here) |

Ordering is `(tier, position, name_len, name)`, so the order is total and a page
boundary cannot move a hit between runs.

## Changes made here

1. `haystack[..position]` / `haystack[position + len..]` became `get(..)` /
   `get(..)`, because this workspace denies slice indexing.
2. The code was lifted out of a Tokio-based module: **no async, no `Command`,
   no I/O**. Only the three pure functions and their type were taken; the
   `Visitor` that streamed `nix search --json` was **not** copied (this project
   parses that output separately, with `serde_json`).
3. `ranking_term` was extracted from upstream's private `ranking_term` for reuse
   by the catalogue's own query path; behaviour is identical.
4. Unit tests for the tier scale, token boundaries and tie-breaking were written
   here; upstream has no tests for these functions.
