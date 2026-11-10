//! Layout detection acceptance tests (M1 / D5).

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

mod support;

use std::path::PathBuf;

use nhs_core::{detect, Layout};

#[test]
fn discovery_finds_flake_home_configuration() {
    let fixture = support::flake_home();
    let discovery = detect(&fixture.fs, &fixture.env);

    match discovery.layout {
        Layout::FlakeHome {
            root,
            main_file,
            attr,
        } => {
            let config_dir = support::USER_CONFIG_DIR;
            assert_eq!(root, PathBuf::from(config_dir));
            assert_eq!(main_file, PathBuf::from(format!("{config_dir}/flake.nix")));
            assert_eq!(attr, "tester");
        }
        other => panic!("expected FlakeHome, got {other:?}"),
    }

    assert!(discovery
        .attempts
        .iter()
        .any(|attempt| attempt.step == "config_home_flake" && attempt.accepted));
}

#[test]
fn discovery_finds_classic_configuration() {
    let fixture = support::classic_home();
    let discovery = detect(&fixture.fs, &fixture.env);

    match discovery.layout {
        Layout::ClassicHome { main_file } => {
            let config_dir = support::USER_CONFIG_DIR;
            assert_eq!(main_file, PathBuf::from(format!("{config_dir}/home.nix")));
        }
        other => panic!("expected ClassicHome, got {other:?}"),
    }

    assert!(discovery
        .attempts
        .iter()
        .any(|attempt| attempt.step == "config_home_classic" && attempt.accepted));
}

#[test]
fn discovery_finds_home_manager_inside_nixos() {
    let fixture = support::flake_nixos();
    let discovery = detect(&fixture.fs, &fixture.env);

    match discovery.layout {
        Layout::FlakeNixos {
            root,
            main_file,
            host,
            user,
        } => {
            assert_eq!(root, PathBuf::from("/etc/nixos"));
            assert_eq!(main_file, PathBuf::from("/etc/nixos/flake.nix"));
            assert_eq!(host, "testerbox");
            assert_eq!(user, "tester");
        }
        other => panic!("expected FlakeNixos, got {other:?}"),
    }
}

#[test]
fn discovery_reports_profile_only_layout() {
    let fixture = support::profile_only();
    let discovery = detect(&fixture.fs, &fixture.env);

    match discovery.layout {
        Layout::ProfileOnly {
            profile_dir,
            generations,
        } => {
            assert_eq!(profile_dir, PathBuf::from(support::STATE_PROFILES_DIR));
            let ids: Vec<Option<u32>> =
                generations.iter().map(|generation| generation.id).collect();
            assert_eq!(ids, vec![Some(1), Some(2), Some(3)]);
            let profiles_dir = support::STATE_PROFILES_DIR;
            for (index, generation) in generations.iter().enumerate() {
                let expected_id = index + 1;
                assert_eq!(
                    generation.link,
                    PathBuf::from(format!("{profiles_dir}/home-manager-{expected_id}-link"))
                );
                assert_eq!(
                    generation.target.clone(),
                    Some(support::generation_path(
                        u32::try_from(expected_id).expect("small id")
                    ))
                );
            }
        }
        other => panic!("expected ProfileOnly, got {other:?}"),
    }

    assert_eq!(
        discovery.generations.current,
        Some(3),
        "the generation snapshot must resolve the current profile link"
    );
}

#[test]
fn discovery_keeps_generations_for_classic_layout() {
    let fixture = support::profile_only().dir(support::USER_CONFIG_DIR).file(
        format!("{}/home.nix", support::USER_CONFIG_DIR),
        support::CLASSIC_HOME_NIX,
    );
    let discovery = detect(&fixture.fs, &fixture.env);

    assert!(matches!(discovery.layout, Layout::ClassicHome { .. }));
    assert_eq!(discovery.generations.current, Some(3));
    assert_eq!(discovery.generations.generations.len(), 3);
}

#[test]
fn discovery_keeps_generations_for_flake_home_layout() {
    let fixture = support::profile_only().dir(support::USER_CONFIG_DIR).file(
        format!("{}/flake.nix", support::USER_CONFIG_DIR),
        support::FLAKE_HOME_NIX,
    );
    let discovery = detect(&fixture.fs, &fixture.env);

    assert!(matches!(discovery.layout, Layout::FlakeHome { .. }));
    assert_eq!(discovery.generations.current, Some(3));
    assert_eq!(discovery.generations.generations.len(), 3);
}

#[test]
fn discovery_never_guesses_when_two_layouts_exist() {
    let fixture = support::ambiguous_layouts();
    let discovery = detect(&fixture.fs, &fixture.env);

    match discovery.layout {
        Layout::Unknown { reason } => {
            assert!(
                reason.contains("FlakeHome"),
                "reason must name FlakeHome, was: {reason}"
            );
            assert!(
                reason.contains("FlakeNixos"),
                "reason must name FlakeNixos, was: {reason}"
            );
        }
        other => panic!("expected Unknown for two plausible layouts, got {other:?}"),
    }

    let accepted: Vec<&str> = discovery
        .attempts
        .iter()
        .filter(|attempt| attempt.accepted)
        .map(|attempt| attempt.step.as_str())
        .collect();
    assert!(accepted.contains(&"config_home_flake"));
    assert!(accepted.contains(&"nixos"));
}

/// The profile symlink decides which generation is current; the list itself is
/// read-only and must contain every link, in id order.
#[test]
fn generation_state_marks_the_profile_target_as_current() {
    let fixture = support::profile_only();
    let state = nhs_core::discovery::generation_state(
        &fixture.fs,
        std::path::Path::new(support::STATE_PROFILES_DIR),
    );
    assert_eq!(state.generations.len(), 3);
    assert_eq!(
        state.generations.iter().map(|g| g.id).collect::<Vec<_>>(),
        [Some(1), Some(2), Some(3)]
    );
    assert_eq!(state.current, Some(3));
}

/// Without a profile symlink the most recent generation is reported as current,
/// and the result says so instead of guessing silently.
#[test]
fn generation_state_falls_back_to_the_highest_id() {
    let mut fixture = support::Fixture::new();
    for index in 1..=3 {
        fixture = fixture.symlink(
            format!("{}/home-manager-{index}-link", support::STATE_PROFILES_DIR),
            support::generation_path(index),
        );
    }
    let state = nhs_core::discovery::generation_state(
        &fixture.fs,
        std::path::Path::new(support::STATE_PROFILES_DIR),
    );
    assert_eq!(state.generations.len(), 3);
    assert_eq!(state.current, Some(3));
}
