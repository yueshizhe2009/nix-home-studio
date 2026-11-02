//! Unified diff rendering and insertion detection.
//!
//! The diff is what the user actually reviews, so it is produced by a real
//! diff algorithm rather than by string comparison.

use similar::{ChangeTag, TextDiff};

/// Three lines of context, the usual `git diff` default.
const CONTEXT_RADIUS: usize = 3;

/// Render a unified diff between `old` and `new`.
///
/// Returns an empty string when the texts are identical. Headers use the
/// conventional `--- a/<name>` / `+++ b/<name>` form with the file name only,
/// because the absolute path is already shown separately in the UI.
#[must_use]
pub fn unified(old: &str, new: &str, name: &str) -> String {
    if old == new {
        return String::new();
    }
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();
    for (index, group) in diff.grouped_ops(CONTEXT_RADIUS).iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if let Some(first) = group.first() {
            use std::fmt::Write as _;
            let _ = writeln!(
                out,
                "@@ -{},{} +{},{} @@",
                first.old_range().start + 1,
                first.old_range().len(),
                first.new_range().start + 1,
                first.new_range().len()
            );
        }
        for op in group {
            for change in diff.iter_changes(op) {
                let sign = match change.tag() {
                    ChangeTag::Delete => '-',
                    ChangeTag::Insert => '+',
                    ChangeTag::Equal => ' ',
                };
                out.push(sign);
                out.push_str(change.value());
                if !change.value().ends_with('\n') {
                    out.push('\n');
                }
            }
        }
    }
    if out.is_empty() {
        // Defensive: a real difference always produces at least one hunk, but
        // never return silently-empty for differing input.
        return format!("--- a/{name}\n+++ b/{name}\n");
    }
    format!("--- a/{name}\n+++ b/{name}\n{out}")
}

/// True when every changed line is an addition and no existing line was
/// removed or modified — i.e. the edit only inserts text.
///
/// This is derived from the diff rather than declared by the caller, so a
/// "one import line" claim cannot be made by a change that rewrites a file.
#[must_use]
pub fn is_pure_insertion(old: &str, new: &str) -> bool {
    if old == new {
        return false;
    }
    let diff = TextDiff::from_lines(old, new);
    let mut inserted = false;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => inserted = true,
            ChangeTag::Equal => {}
            ChangeTag::Delete => return false,
        }
    }
    inserted
}

/// Count added and removed lines, for reporting.
#[must_use]
pub fn counts(old: &str, new: &str) -> (usize, usize) {
    let diff = TextDiff::from_lines(old, new);
    let mut added = 0;
    let mut removed = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used)]

    use super::{counts, is_pure_insertion, unified};

    #[test]
    fn identical_text_has_no_diff() {
        assert!(unified("a\n", "a\n", "home.nix").is_empty());
        assert!(!is_pure_insertion("a\n", "a\n"));
    }

    #[test]
    fn diff_names_the_file_without_an_absolute_path() {
        let diff = unified("{ }\n", "{ x = 1; }\n", "home.nix");
        assert!(
            diff.starts_with("--- a/home.nix\n+++ b/home.nix\n"),
            "{diff}"
        );
        assert!(!diff.contains("/home/"));
    }

    #[test]
    fn a_single_added_line_is_a_pure_insertion() {
        let old = "{\n  imports = [ ];\n}\n";
        let new = "{\n  imports = [ ./nhs.nix ];\n}\n";
        // This modifies the list rather than adding a line: not pure insertion.
        assert!(!is_pure_insertion(old, new));

        let old = "{\n}\n";
        let new = "{\n  x = 1;\n}\n";
        assert!(is_pure_insertion(old, new));
    }

    #[test]
    fn counts_report_added_and_removed_lines() {
        assert_eq!(counts("a\n", "a\nb\n"), (1, 0));
        assert_eq!(counts("a\nb\n", "b\n"), (0, 1));
        assert_eq!(counts("a\n", "b\n"), (1, 1));
    }
}
