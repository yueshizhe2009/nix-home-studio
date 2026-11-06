//! GUI argument handling. Help and version must not initialise GTK, so these
//! tests deliberately remove the display environment.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nhs-gui"))
        .args(args)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .expect("run nhs-gui")
}

#[test]
fn help_works_without_a_display() {
    for flag in ["--help", "-h"] {
        let output = run(&[flag]);
        assert!(output.status.success(), "{flag} failed");
        let text = String::from_utf8_lossy(&output.stdout);
        for needle in ["usage:", "--check", "--flow-check", "--version"] {
            assert!(text.contains(needle), "{flag} is missing {needle}: {text}");
        }
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn version_works_without_a_display() {
    let expected = format!("nhs-gui {}", env!("CARGO_PKG_VERSION"));
    for flag in ["--version", "-V"] {
        let output = run(&[flag]);
        assert!(output.status.success(), "{flag} failed");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn unknown_arguments_are_refused() {
    for arguments in [
        vec!["--bogus"],
        vec!["--check", "--bogus"],
        vec!["--help", "--bogus"],
        vec!["--version", "--bogus"],
    ] {
        let output = run(&arguments);
        assert!(!output.status.success(), "{arguments:?} succeeded");
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("usage:"), "{arguments:?} stderr: {error}");
        assert!(output.stdout.is_empty(), "{arguments:?} printed JSON");
    }
}

#[test]
fn check_does_not_ignore_extra_arguments() {
    let output = run(&["--check", "--bogus"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("expected one option"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
}
