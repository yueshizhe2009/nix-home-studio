//! Gate G4: an activation command cannot be built without a confirmation, and a
//! build is never an activation.
//!
//! Nothing here runs a process. The point is the *type*: `switch_command` returns
//! an error when the caller has no `Confirmation`, and the CLI is checked (in
//! `tests/safety.rs`) to never grant one in this version.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used
)]

use std::path::PathBuf;

use nhs_core::discovery::Layout;
use nhs_core::proc::Program;
use nhs_core::switch::{switch_command, switch_preview, Confirmation, SwitchError};
use nhs_core::validation::build_preview;

fn classic() -> Layout {
    Layout::ClassicHome {
        main_file: PathBuf::from("/home/tester/.config/home-manager/home.nix"),
    }
}

fn flake() -> Layout {
    Layout::FlakeHome {
        root: PathBuf::from("/home/tester/dotfiles"),
        main_file: PathBuf::from("/home/tester/dotfiles/home.nix"),
        attr: "tester".to_string(),
    }
}

#[test]
fn switch_without_confirmation_is_refused() {
    for layout in [classic(), flake()] {
        match switch_command(&layout, None) {
            Err(SwitchError::MissingConfirmation) => {}
            other => panic!("expected MissingConfirmation, got {other:?}"),
        }
    }
}

#[test]
fn switch_with_confirmation_is_the_home_manager_switch_subcommand() {
    let spec = switch_command(&classic(), Some(Confirmation::grant())).expect("granted");
    assert_eq!(spec.program, Program::HomeManager);
    assert_eq!(spec.args.first().and_then(|a| a.to_str()), Some("switch"));

    let spec = switch_command(&flake(), Some(Confirmation::grant())).expect("granted");
    assert_eq!(spec.args.first().and_then(|a| a.to_str()), Some("switch"));
    assert!(spec.display().contains(".#tester"), "{}", spec.display());
}

#[test]
fn build_and_switch_are_distinct_subcommands_with_distinct_argv() {
    for layout in [classic(), flake()] {
        let build = build_preview(&layout).expect("build preview");
        let switch = switch_preview(&layout).expect("switch preview");
        assert_ne!(build, switch, "build and switch must differ");
        assert!(build.contains(" build"), "{build}");
        assert!(!build.contains("switch"), "{build}");
        assert!(switch.contains(" switch"), "{switch}");
        assert!(!switch.contains("build"), "{switch}");
    }
}

#[test]
fn the_switch_preview_needs_no_confirmation_because_it_runs_nothing() {
    let preview = switch_preview(&classic()).expect("preview");
    assert!(
        preview.starts_with("home-manager switch"),
        "the preview is the command line only: {preview}"
    );
}

#[test]
fn a_layout_without_a_configuration_cannot_be_activated() {
    let layout = Layout::ProfileOnly {
        profile_dir: PathBuf::from("/home/tester/.local/state/nix/profiles"),
        generations: Vec::new(),
    };
    assert!(matches!(
        switch_command(&layout, Some(Confirmation::grant())),
        Err(SwitchError::UnsupportedLayout { .. })
    ));
    assert!(matches!(
        switch_preview(&layout),
        Err(SwitchError::UnsupportedLayout { .. })
    ));
}
