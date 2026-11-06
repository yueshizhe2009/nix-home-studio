//! CLI argument handling: help and version are part of the delivery interface.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nhs"))
        .args(args)
        .output()
        .expect("run nhs")
}

#[test]
fn version_flags_print_the_package_version() {
    let expected = format!("nhs {}", env!("CARGO_PKG_VERSION"));
    for flag in ["--version", "-V"] {
        let output = run(&[flag]);
        assert!(output.status.success(), "{flag} failed");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            expected,
            "{flag} output"
        );
        assert!(
            output.stderr.is_empty(),
            "{flag} wrote to stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn version_rejects_extra_arguments() {
    let output = run(&["--version", "--json"]);
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("takes no arguments"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
}
