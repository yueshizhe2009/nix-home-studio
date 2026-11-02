//! M5.2 acceptance tests: which nixpkgs to search, and what "pinned" means.
//!
//! The central case is the one NixBox gets wrong: a lock entry that records an
//! input as `follows` has no `locked` of its own, so the indirection has to be
//! followed to the node it names. All fixtures are inline JSON — no flake, no
//! network, no `nix`.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use nhs_core::revision::{resolve, RevisionError, SourceKind};

const REV: &str = "0123456789abcdef0123456789abcdef01234567";

/// A lock whose only nixpkgs is reached by following a dependency's input.
fn lock_with_follows_array() -> String {
    format!(
        r#"{{
  "nodes": {{
    "root": {{ "inputs": {{ "home-manager": "home-manager", "base": "base", "nixpkgs": ["base", "nixpkgs"] }} }},
    "home-manager": {{ "inputs": {{ "nixpkgs": ["base", "nixpkgs"] }}, "locked": {{ "type": "github", "owner": "nix-community", "repo": "home-manager", "rev": "{REV}" }} }},
    "base": {{ "inputs": {{ "nixpkgs": "nixpkgs" }} }},
    "nixpkgs": {{ "locked": {{ "type": "github", "owner": "NixOS", "repo": "nixpkgs", "rev": "{REV}" }}, "original": {{ "owner": "NixOS", "repo": "nixpkgs", "type": "github" }} }}
  }},
  "root": "root",
  "version": 7
}}"#
    )
}

/// The same indirection, written as an explicit `follows` object.
fn lock_with_follows_object() -> String {
    format!(
        r#"{{
  "nodes": {{
    "root": {{ "inputs": {{ "nixpkgs": {{ "follows": "inputs.base.nixpkgs" }}, "base": "base" }} }},
    "base": {{ "inputs": {{ "nixpkgs": "nixpkgs" }} }},
    "nixpkgs": {{ "locked": {{ "type": "github", "owner": "NixOS", "repo": "nixpkgs", "rev": "{REV}" }} }}
  }},
  "root": "root",
  "version": 7
}}"#
    )
}

/// The ordinary shape: the root names nixpkgs directly.
fn lock_direct() -> String {
    format!(
        r#"{{
  "nodes": {{
    "root": {{ "inputs": {{ "nixpkgs": "nixpkgs" }} }},
    "nixpkgs": {{ "locked": {{ "type": "github", "owner": "NixOS", "repo": "nixpkgs", "rev": "{REV}" }}, "original": {{ "owner": "NixOS", "repo": "nixpkgs", "type": "github" }} }}
  }},
  "root": "root",
  "version": 7
}}"#
    )
}

/// The acceptance test: `follows` is followed, and the resolved revision is the
/// followed node's, not the dependency's.
#[test]
fn revision_resolution_follows_a_follows_input() {
    for (name, lock) in [
        ("array form", lock_with_follows_array()),
        ("object form", lock_with_follows_object()),
        ("direct", lock_direct()),
    ] {
        let resolution =
            resolve(None, Some(&lock)).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(resolution.kind, SourceKind::LockFile, "{name}");
        assert_eq!(resolution.revision.as_deref(), Some(REV), "{name}");
        assert_eq!(
            resolution.reference,
            format!("github:NixOS/nixpkgs/{REV}"),
            "{name}: the owner and repo must come from the followed node"
        );
        assert!(resolution.pinned, "{name}: a locked rev is pinned");
    }
}

/// An explicit ref wins over the lock, and only a commit makes it pinned.
#[test]
fn an_explicit_reference_wins_and_reports_whether_it_is_pinned() {
    let lock = lock_direct();
    let resolution =
        resolve(Some("github:NixOS/nixpkgs/nixos-24.05"), Some(&lock)).expect("resolve");
    assert_eq!(resolution.kind, SourceKind::Explicit);
    assert_eq!(resolution.reference, "github:NixOS/nixpkgs/nixos-24.05");
    assert!(!resolution.pinned, "a branch is not a pin");
    assert_eq!(resolution.revision, None);

    let resolution =
        resolve(Some(&format!("github:NixOS/nixpkgs/{REV}")), Some(&lock)).expect("resolve");
    assert!(resolution.pinned);
    assert_eq!(resolution.revision.as_deref(), Some(REV));

    let resolution = resolve(Some(&format!("nixpkgs?rev={REV}")), None).expect("resolve");
    assert!(resolution.pinned, "?rev= is a pin");
    assert_eq!(resolution.revision.as_deref(), Some(REV));
}

/// Only a real `rev` parameter marks a reference as pinned. Another parameter
/// whose name merely ends in `rev` must not.
#[test]
fn pinned_detection_only_accepts_a_real_rev_parameter() {
    let hex40 = "0123456789abcdef0123456789abcdef01234567";
    let short = "0123456";

    let cases: [(&str, Option<&str>); 6] = [
        // The trap: `notrev=` is a different parameter.
        ("github:NixOS/nixpkgs?notrev=0123456789abcdef", None),
        (
            "github:NixOS/nixpkgs?rev=0123456789abcdef",
            Some("0123456789abcdef"),
        ),
        (
            "github:NixOS/nixpkgs?ref=main&rev=0123456789abcdef",
            Some("0123456789abcdef"),
        ),
        (
            "github:NixOS/nixpkgs?rev=0123456789abcdef&dir=sub",
            Some("0123456789abcdef"),
        ),
        (
            &format!("github:NixOS/nixpkgs/{hex40}?dir=sub"),
            Some(hex40),
        ),
        // Too short to be a commit: not a pin.
        ("github:NixOS/nixpkgs?rev=0123", None),
    ];
    for (reference, expected) in cases {
        let resolution = resolve(Some(reference), None).expect("resolve");
        assert_eq!(
            resolution.pinned,
            expected.is_some(),
            "`{reference}` pinned? expected {:?}",
            expected
        );
        assert_eq!(resolution.revision.as_deref(), expected, "`{reference}`");
    }

    // A bare short hex segment is not a commit either.
    let resolution =
        resolve(Some(&format!("github:NixOS/nixpkgs/{short}")), None).expect("resolve");
    assert!(
        !resolution.pinned,
        "a 7-character segment is not a full commit"
    );
}

/// A `follows` path with a non-string element is a damaged lock, and damaged
/// locks fail closed instead of being silently shortened.
#[test]
fn a_follows_path_with_a_non_string_element_is_refused() {
    let broken = r#"{ "nodes": {
  "root": { "inputs": { "nixpkgs": [123, "base", "nixpkgs"] } },
  "base": { "inputs": { "nixpkgs": "nixpkgs" } },
  "nixpkgs": { "locked": { "type": "github", "owner": "NixOS", "repo": "nixpkgs", "rev": "0123456789abcdef0123456789abcdef01234567" } }
}, "root": "root", "version": 7 }"#;
    let error = resolve(None, Some(broken)).expect_err("must not drop the number");
    match &error {
        RevisionError::InvalidLock { detail } => {
            assert!(detail.contains("not a string"), "{detail}");
        }
        other => panic!("expected InvalidLock, got {other}"),
    }
}

/// With no ref and no lock, the fallback is the unqualified `nixpkgs` — and it is
/// labelled as not pinned.
#[test]
fn without_a_lock_the_default_is_unpinned() {
    let resolution = resolve(None, None).expect("resolve");
    assert_eq!(resolution.kind, SourceKind::UnpinnedDefault);
    assert_eq!(resolution.reference, "nixpkgs");
    assert!(!resolution.pinned);
    assert_eq!(resolution.revision, None);
}

/// The snapshot cache key changes with the reference and with the format version.
#[test]
fn the_cache_key_binds_the_reference_and_the_format() {
    let base = resolve(None, Some(&lock_direct())).expect("resolve");
    let same = resolve(None, Some(&lock_direct())).expect("resolve");
    assert_eq!(base.cache_key(1), same.cache_key(1), "same input, same key");
    assert_ne!(
        base.cache_key(1),
        base.cache_key(2),
        "a format bump invalidates"
    );
    let other = resolve(Some("github:NixOS/nixpkgs/nixos-24.05"), None).expect("resolve");
    assert_ne!(
        base.cache_key(1),
        other.cache_key(1),
        "another ref invalidates"
    );

    // A changed lock (a new rev) invalidates too.
    let moved = lock_direct().replace(REV, &"f".repeat(40));
    let moved = resolve(None, Some(&moved)).expect("resolve");
    assert_ne!(base.cache_key(1), moved.cache_key(1));
}

/// Broken locks are refused with a reason, not guessed at.
#[test]
fn a_lock_that_cannot_be_followed_is_refused() {
    let cases: [(&str, &str); 6] = [
        ("not json", "not a lock"),
        ("no nodes", r#"{ "root": "root", "version": 7 }"#),
        (
            "no nixpkgs",
            r#"{ "nodes": { "root": { "inputs": {} } }, "root": "root", "version": 7 }"#,
        ),
        (
            "dangling follows",
            r#"{ "nodes": { "root": { "inputs": { "nixpkgs": ["nixpkgs"] } } }, "root": "root", "version": 7 }"#,
        ),
        (
            "no rev",
            r#"{ "nodes": { "root": { "inputs": { "nixpkgs": "nixpkgs" } }, "nixpkgs": { "locked": { "type": "github", "owner": "NixOS", "repo": "nixpkgs" } } }, "root": "root", "version": 7 }"#,
        ),
        (
            "path type",
            r#"{ "nodes": { "root": { "inputs": { "nixpkgs": "nixpkgs" } }, "nixpkgs": { "locked": { "type": "path", "path": "/tmp/nixpkgs" } } }, "root": "root", "version": 7 }"#,
        ),
    ];
    for (name, lock) in cases {
        let error = resolve(None, Some(lock)).expect_err(name);
        let message = error.to_string();
        assert!(!message.is_empty(), "{name}");
        match name {
            "dangling follows" => assert!(
                matches!(
                    error,
                    RevisionError::FollowsTargetMissing { .. } | RevisionError::FollowsCycle { .. }
                ),
                "{name}: {message}"
            ),
            "no rev" => assert!(
                matches!(error, RevisionError::LockedIncomplete { field: "rev", .. }),
                "{name}: {message}"
            ),
            "path type" => assert!(
                matches!(error, RevisionError::UnsupportedLockedType { .. }),
                "{name}: {message}"
            ),
            _ => {}
        }
    }

    assert!(matches!(
        resolve(Some("   "), None),
        Err(RevisionError::EmptyExplicitRef)
    ));
}

/// A `follows` loop is detected instead of spinning, and a legitimate
/// indirection that ends at a locked node is not mistaken for one.
#[test]
fn a_follows_cycle_is_refused() {
    let legitimate = format!(
        r#"{{ "nodes": {{
  "root": {{ "inputs": {{ "base": "base", "nixpkgs": ["base", "nixpkgs"] }} }},
  "base": {{ "inputs": {{ "nixpkgs": "nixpkgs" }} }},
  "nixpkgs": {{ "locked": {{ "type": "github", "owner": "NixOS", "repo": "nixpkgs", "rev": "{REV}" }} }}
}}, "root": "root", "version": 7 }}"#
    );
    let resolution = resolve(None, Some(&legitimate)).expect("resolves through one hop");
    assert!(resolution.pinned);
    assert_eq!(resolution.revision.as_deref(), Some(REV));

    let looping = r#"{ "nodes": {
  "root": { "inputs": { "nixpkgs": ["base", "nixpkgs"] } },
  "base": { "inputs": { "nixpkgs": ["base", "nixpkgs"] } }
}, "root": "root", "version": 7 }"#;
    let error = resolve(None, Some(looping)).expect_err("must not loop forever");
    assert!(
        matches!(
            error,
            RevisionError::FollowsCycle { .. } | RevisionError::FollowsTargetMissing { .. }
        ),
        "{error}"
    );
}

/// A version-7 lock from a real project still resolves, so the parser is not
/// tuned only to the fixtures above.
#[test]
fn a_real_shaped_lock_still_resolves() {
    let lock = format!(
        r#"{{
  "nodes": {{
    "flake-utils": {{
      "inputs": {{ "systems": "systems" }},
      "locked": {{ "lastModified": 1710146030, "narHash": "sha256-abc", "owner": "numtide", "repo": "flake-utils", "rev": "{REV}", "type": "github" }},
      "original": {{ "owner": "numtide", "repo": "flake-utils", "type": "github" }}
    }},
    "nixpkgs": {{
      "locked": {{ "lastModified": 1710806803, "narHash": "sha256-def", "owner": "NixOS", "repo": "nixpkgs", "rev": "{REV}", "type": "github" }},
      "original": {{ "owner": "NixOS", "repo": "nixpkgs", "type": "github" }}
    }},
    "root": {{ "inputs": {{ "flake-utils": "flake-utils", "nixpkgs": "nixpkgs" }} }},
    "systems": {{
      "locked": {{ "lastModified": 1681028828, "narHash": "sha256-ghi", "owner": "nix-systems", "repo": "default", "rev": "{REV}", "type": "github" }},
      "original": {{ "owner": "nix-systems", "repo": "default", "type": "github" }}
    }}
  }},
  "root": "root",
  "version": 7
}}"#
    );
    let resolution = resolve(None, Some(&lock)).expect("resolve");
    assert_eq!(resolution.reference, format!("github:NixOS/nixpkgs/{REV}"));
    assert!(resolution.pinned);
}
