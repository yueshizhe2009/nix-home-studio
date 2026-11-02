//! Toolchain resolution acceptance tests (M1 / architecture §3.1).
//!
//! Tests are grouped by the layout they model, because the two are not
//! interchangeable evidence:
//!
//! * **Real layout** — reproduces this host exactly: a generation whose `bin/`
//!   holds only `home-manager-generation`, with the command in its own store
//!   derivation. This is the only shape verified against the live machine.
//! * **Alternative layout** — a generation that carries `bin/home-manager`
//!   itself, as some Home Manager versions or hand-built setups do. Supported,
//!   but fixture-only evidence; it must never be the reason a test passes for
//!   the real case.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

mod support;

use std::path::PathBuf;

use nhs_core::{resolve_toolchain, HmInvocation};

use nhs_core::toolchain::STRATEGY_ORDER;

const STRATEGIES: [&str; 5] = STRATEGY_ORDER;

#[test]
fn toolchain_prefers_path_binary_when_present() {
    let fixture = support::toolchain_path_binary();
    let toolchain = resolve_toolchain(&fixture.fs, &fixture.env);

    match &toolchain.home_manager {
        HmInvocation::Binary(binary) => {
            assert_eq!(binary, &PathBuf::from(support::PATH_BINARY));
        }
        other => panic!("expected Binary, got {other:?}"),
    }

    let outcome = toolchain.attempt("path_binary").expect("attempt recorded");
    assert!(
        outcome.contains("found on PATH"),
        "unexpected outcome: {outcome}"
    );
    assert_eq!(toolchain.home_manager.strategy(), "path_binary");
}

#[test]
fn toolchain_resolves_home_manager_from_the_store_when_not_on_path() {
    // The audit host's exact shape: a generation with no command, and the command
    // in its own store derivation. Getting `Absent` or `NixRun` here would mean
    // demanding a network fetch for a binary that is already on disk.
    let fixture = support::with_home_manager_in_store(support::profile_only());
    let toolchain = resolve_toolchain(&fixture.fs, &fixture.env);

    match &toolchain.home_manager {
        HmInvocation::FromStore {
            binary,
            version,
            candidates,
        } => {
            assert_eq!(version, "0-unstable-2026-08-06");
            assert_eq!(candidates.len(), 2, "both store builds are candidates");
            assert!(
                binary.to_string_lossy().contains("-home-manager-"),
                "unexpected store binary: {}",
                binary.display()
            );
            assert!(
                binary.to_string_lossy().ends_with("/bin/home-manager"),
                "must be the command, not a store decoy: {}",
                binary.display()
            );
            let name = binary.to_string_lossy();
            for decoy in [
                "-home-manager-generation",
                "-home-manager-path",
                "-home-manager-files",
                "-home-manager-source",
            ] {
                assert!(!name.contains(decoy), "picked a decoy ({decoy}): {name}");
            }
            assert!(
                !name.ends_with(".drv/bin/home-manager"),
                "picked a .drv: {name}"
            );
        }
        other => panic!("expected FromStore, got {other:?}"),
    }

    let path_outcome = toolchain.attempt("path_binary").expect("attempt recorded");
    assert!(
        path_outcome.contains("not found in any of"),
        "unexpected outcome: {path_outcome}"
    );
    let generation_outcome = toolchain
        .attempt("generation_or_store")
        .expect("attempt recorded");
    assert!(
        generation_outcome.contains("found it in the store"),
        "unexpected outcome: {generation_outcome}"
    );
    // The network fallback must not have been chosen.
    assert_eq!(
        toolchain.attempt("nix_run").map(str::to_string),
        Some("skipped: home-manager already resolved by generation_or_store".to_string())
    );
}

/// A generation that *does* carry the command still wins over the store scan.
#[test]
fn toolchain_prefers_the_command_inside_the_generation_when_present() {
    let mut fixture = support::profile_only();
    let generation = support::generation_path(3);
    fixture = fixture.file(
        format!("{}/bin/home-manager", generation.display()),
        "#!/bin/sh\n",
    );
    let toolchain = resolve_toolchain(&fixture.fs, &fixture.env);
    match &toolchain.home_manager {
        HmInvocation::FromGeneration {
            generation: resolved,
            ..
        } => {
            assert_eq!(resolved, &generation);
        }
        other => panic!("expected FromGeneration, got {other:?}"),
    }
    let outcome = toolchain
        .attempt("generation_or_store")
        .expect("attempt recorded");
    assert!(
        outcome.contains("found inside generation"),
        "unexpected outcome: {outcome}"
    );
}

#[test]
fn toolchain_resolves_a_profile_element_installation() {
    let fixture = support::toolchain_profile_element();
    let toolchain = resolve_toolchain(&fixture.fs, &fixture.env);

    match toolchain.home_manager {
        HmInvocation::ProfileElement {
            profile_dir,
            binary,
        } => {
            assert_eq!(profile_dir, PathBuf::from("/home/tester/.nix-profile"));
            assert_eq!(
                binary,
                PathBuf::from("/home/tester/.nix-profile/bin/home-manager")
            );
        }
        other => panic!("expected ProfileElement, got {other:?}"),
    }
}

#[test]
fn toolchain_falls_back_to_nix_run_only_after_all_others_fail() {
    let fixture = support::toolchain_nix_run();
    let toolchain = resolve_toolchain(&fixture.fs, &fixture.env);

    match &toolchain.home_manager {
        HmInvocation::NixRun { flake_ref } => {
            assert_eq!(flake_ref, "nixpkgs#home-manager");
        }
        other => panic!("expected NixRun, got {other:?}"),
    }

    let path_outcome = toolchain.attempt("path_binary").expect("attempt recorded");
    assert!(path_outcome.contains("not found"), "{path_outcome}");
    let generation_outcome = toolchain
        .attempt("generation_or_store")
        .expect("attempt recorded");
    assert!(
        generation_outcome.contains("no HM generation profile"),
        "{generation_outcome}"
    );
    let profile_outcome = toolchain
        .attempt("profile_element")
        .expect("attempt recorded");
    assert!(
        profile_outcome.contains("not found in profile roots"),
        "{profile_outcome}"
    );
    let nix_run_outcome = toolchain.attempt("nix_run").expect("attempt recorded");
    assert!(nix_run_outcome.starts_with("selected"), "{nix_run_outcome}");
}

#[test]
fn toolchain_reports_absent_with_every_strategy_named() {
    let fixture = support::toolchain_absent();
    let toolchain = resolve_toolchain(&fixture.fs, &fixture.env);

    assert_eq!(toolchain.home_manager, HmInvocation::Absent);
    for strategy in STRATEGIES {
        let outcome = toolchain
            .attempt(strategy)
            .unwrap_or_else(|| panic!("strategy {strategy} missing from attempts"));
        assert!(!outcome.is_empty(), "strategy {strategy} has no outcome");
    }
    let absent = toolchain.attempt("absent").expect("attempt recorded");
    assert!(absent.starts_with("selected"), "{absent}");
}

#[test]
fn nixos_rebuild_is_detected_but_never_executable() {
    let fixture = support::with_nixos_rebuild();
    let toolchain = resolve_toolchain(&fixture.fs, &fixture.env);

    assert_eq!(
        toolchain.nixos_rebuild,
        Some(PathBuf::from("/run/current-system/sw/bin/nixos-rebuild"))
    );
    let outcome = toolchain
        .attempt("nixos_rebuild")
        .expect("attempt recorded");
    assert!(outcome.contains("display only"), "{outcome}");

    // Shape check: `nixos-rebuild` is detected for display, and there is no way
    // to execute it — the only programs `proc` can run are `nix` and
    // `home-manager` (M4 allowlist; gate G3 lives in `tests/safety.rs`).
    use nhs_core::Program;
    assert_eq!(
        Program::from_name("nixos-rebuild"),
        None,
        "nixos-rebuild must not be executable (D2)"
    );
    assert_eq!(Program::ALL.len(), 2);
}

// ---------------------------------------------------------------------------
// Real layout: generation without the command, command in its own store path.
// ---------------------------------------------------------------------------

/// With several Home Manager versions in the store, the highest version must
/// win — not the lexicographically largest store hash.
#[test]
fn real_layout_picks_the_highest_version_when_several_stores_exist() {
    // Hash prefixes are deliberately ordered so a hash-based pick would choose
    // the *older* version: `zzz…` > `aaa…`, but 2026-08-06 < 2026-09-01.
    let fixture = support::with_home_manager_versions(&[
        ("zzzzzzzz", "0-unstable-2026-08-06"),
        ("aaaaaaaa", "0-unstable-2026-09-01"),
        ("mmmmmmmm", "0-unstable-2026-08-15"),
    ]);
    let toolchain = resolve_toolchain(&fixture.fs, &fixture.env);

    match &toolchain.home_manager {
        HmInvocation::FromStore {
            binary,
            version,
            candidates,
        } => {
            assert_eq!(
                version, "0-unstable-2026-09-01",
                "highest version must win; picked {}",
                version
            );
            assert!(
                binary
                    .to_string_lossy()
                    .contains("aaaaaaaa-home-manager-0-unstable-2026-09-01"),
                "selected the wrong store path: {}",
                binary.display()
            );
            assert_eq!(candidates.len(), 3, "all three must be reported");
            assert_eq!(
                candidates.first(),
                Some(binary),
                "the winner must be first in the candidate list"
            );
        }
        other => panic!("expected FromStore, got {other:?}"),
    }
}

/// A stable, documented tie-break: equal versions resolve to the same path
/// regardless of the order the store listing returns them in.
#[test]
fn real_layout_breaks_equal_version_ties_deterministically() {
    let forward =
        support::with_home_manager_versions(&[("bbbbbbbb", "26.11"), ("aaaaaaaa", "26.11")]);
    let reverse =
        support::with_home_manager_versions(&[("aaaaaaaa", "26.11"), ("bbbbbbbb", "26.11")]);

    let first = resolve_toolchain(&forward.fs, &forward.env);
    let second = resolve_toolchain(&reverse.fs, &reverse.env);

    let binary_of = |toolchain: &nhs_core::Toolchain| match &toolchain.home_manager {
        HmInvocation::FromStore { binary, .. } => binary.clone(),
        other => panic!("expected FromStore, got {other:?}"),
    };
    assert_eq!(
        binary_of(&first),
        binary_of(&second),
        "equal versions must resolve identically"
    );
    assert!(
        binary_of(&first).to_string_lossy().contains("aaaaaaaa"),
        "tie-break is the ascending store path"
    );
}

/// Version comparison handles both calendar-style and release-style versions.
#[test]
fn real_layout_compares_calendar_and_release_versions() {
    let fixture = support::with_home_manager_versions(&[
        ("cccccccc", "25.05"),
        ("dddddddd", "26.11"),
        ("eeeeeeee", "0-unstable-2026-08-06"),
    ]);
    let toolchain = resolve_toolchain(&fixture.fs, &fixture.env);
    match &toolchain.home_manager {
        HmInvocation::FromStore { version, .. } => {
            // 26.11 > 25.05, and both outrank the 0-unstable build on the first
            // numeric component (26/25 vs 0).
            assert_eq!(version, "26.11");
        }
        other => panic!("expected FromStore, got {other:?}"),
    }
}

/// Decoys must never be selected even when they are the only store entries.
#[test]
fn real_layout_ignores_store_decoys() {
    let fixture = support::with_home_manager_decoys_only();
    let toolchain = resolve_toolchain(&fixture.fs, &fixture.env);
    assert!(
        !matches!(toolchain.home_manager, HmInvocation::FromStore { .. }),
        "decoys must not be treated as the command: {:?}",
        toolchain.home_manager
    );
}

// ---------------------------------------------------------------------------
// Alternative layout: the generation carries `bin/home-manager` itself.
// Fixture-only evidence; it must not be how the real case passes.
// ---------------------------------------------------------------------------

#[test]
fn alternative_layout_uses_the_command_inside_the_generation() {
    let mut fixture = support::profile_only();
    let generation = support::generation_path(3);
    fixture = fixture.file(
        format!("{}/bin/home-manager", generation.display()),
        "#!/bin/sh\n",
    );
    let toolchain = resolve_toolchain(&fixture.fs, &fixture.env);
    match &toolchain.home_manager {
        HmInvocation::FromGeneration {
            generation: resolved,
            ..
        } => assert_eq!(resolved, &generation),
        other => panic!("expected FromGeneration, got {other:?}"),
    }
}

/// The recorded strategy order is normative and documented in
/// `docs/architecture.md` §3.1. Implementation drift must fail here.
#[test]
fn toolchain_attempts_follow_the_documented_order() {
    let fixture = support::toolchain_absent();
    let toolchain = resolve_toolchain(&fixture.fs, &fixture.env);

    let recorded: Vec<&str> = toolchain
        .attempts
        .iter()
        .map(|(strategy, _)| strategy.as_str())
        .filter(|strategy| *strategy != "nix" && *strategy != "nixos_rebuild")
        .collect();
    assert_eq!(
        recorded, STRATEGIES,
        "toolchain strategies must be tried in the documented order"
    );
}
