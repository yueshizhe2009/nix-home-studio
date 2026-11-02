//! Code copied from an Apache-2.0 upstream, kept in its own tree.
//!
//! Each sub-directory holds the adapted sources plus a `PROVENANCE.md` recording
//! the upstream repository, the exact commit, the original path and every change
//! made here. `NOTICE` at the repository root carries the attribution that must
//! survive redistribution.

#[path = "nix-log/nix_log.rs"]
pub mod nix_log;

#[path = "nix-search/mod.rs"]
pub mod nix_search;
