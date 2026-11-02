//! Post-write validation.
//!
//! Gate G6: a file this application writes must still be valid Nix. Refusing a
//! write *before* it lands is better than reporting a broken config afterwards,
//! so [`check`] is called on the planned text during `commit`, before any byte
//! is replaced.

use std::path::Path;

/// Why a planned text was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidNix {
    /// Human-readable first error, from `rnix`.
    pub detail: String,
    /// 1-based line of the first error, when `rnix` reports a position.
    pub line: Option<usize>,
}

/// Validate `text` when `path` is a Nix file; every other extension is accepted
/// as-is, because this crate also writes JSON.
pub fn check(path: &Path, text: &str) -> Result<(), InvalidNix> {
    if !is_nix_file(path) {
        return Ok(());
    }
    parse(text)
}

/// Validate `text` as Nix regardless of any path.
pub fn parse(text: &str) -> Result<(), InvalidNix> {
    let parsed = rnix::Root::parse(text);
    let errors = parsed.errors();
    if errors.is_empty() {
        return Ok(());
    }
    let detail = errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    Err(InvalidNix { detail, line: None })
}

/// True for `*.nix`, case-insensitively.
#[must_use]
pub fn is_nix_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("nix"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

    use super::{check, is_nix_file, parse};
    use std::path::Path;

    #[test]
    fn a_module_function_parses() {
        assert!(parse("{ pkgs, ... }: { home.packages = [ pkgs.fd ]; }").is_ok());
    }

    #[test]
    fn broken_syntax_is_rejected_with_a_detail() {
        let error = parse("{ pkgs, ... }: { home.packages = [ }").expect_err("must fail");
        assert!(!error.detail.is_empty(), "the reason must be reported");
    }

    #[test]
    fn non_nix_paths_are_not_parsed() {
        assert!(check(Path::new("/tmp/state.json"), "not nix at all {").is_ok());
    }

    #[test]
    fn nix_paths_are_parsed() {
        assert!(check(Path::new("/tmp/home.nix"), "{").is_err());
        assert!(check(Path::new("/tmp/HOME.NIX"), "{").is_err());
    }

    #[test]
    fn extension_detection_is_case_insensitive() {
        assert!(is_nix_file(Path::new("a.nix")));
        assert!(is_nix_file(Path::new("a.NIX")));
        assert!(!is_nix_file(Path::new("a.nix.bak")));
    }
}
