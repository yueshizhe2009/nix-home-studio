//! Fake toolchain harness checks (M1 deliverable A4).
//!
//! Two things are proven here:
//!
//! * the generated scripts have the shape the harness promises;
//! * the scripts actually run — argv, stdout, stderr and exit code all behave
//!   like the real command, so M4 can drive them and assert on `argv.log`.
//!
//! Executing a fake shell script is not a violation of the "`nhs-core` never
//! spawns a process" invariant: that invariant is about library code under
//! `src/`, and `no_write_sites_in_m1` / `safety.rs` keep asserting it. Nothing
//! here invokes `nix`, `home-manager`, `nixos-rebuild` or `sudo`.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

mod support;

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn fake_toolchain_writes_an_executable_argv_logging_script() {
    let fake = support::FakeToolchain::new();
    let script = fake.install("home-manager", "home-manager 1.0", "boom", 3);

    let contents = std::fs::read_to_string(&script).expect("read fake script");
    assert!(contents.starts_with("#!/bin/sh"));
    assert!(contents.contains("argv.log"));
    assert!(contents.contains("home-manager 1.0"));
    assert!(contents.contains("boom"));
    assert!(contents.contains("exit 3"));

    let mode = std::fs::metadata(&script)
        .expect("stat fake script")
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0, "fake toolchain script must be executable");

    let env = fake.env();
    assert_eq!(env.path_entries, vec![fake.path().to_path_buf()]);
}

/// The fake command really runs, records its argv, and reports the configured
/// exit status — the contract M4 depends on.
#[test]
fn fake_toolchain_runs_and_records_its_argv() {
    let fake = support::FakeToolchain::new();
    let script = fake.install("home-manager", "generation 7", "a warning", 4);

    let output = Command::new(&script)
        .args(["generations", "--flake", "/tmp/x#me"])
        .output()
        .expect("run fake home-manager");

    assert_eq!(
        output.status.code(),
        Some(4),
        "configured exit code must be preserved"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "generation 7"
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr).trim(), "a warning");

    let log = std::fs::read_to_string(fake.argv_log()).expect("read argv log");
    assert_eq!(
        log.trim(),
        "generations --flake /tmp/x#me",
        "argv must be recorded verbatim, space separated"
    );
}

/// Repeated invocations append, so a test can assert the whole call sequence
/// (M4 needs this to prove `build` never becomes `switch`).
#[test]
fn fake_toolchain_appends_each_invocation_in_order() {
    let fake = support::FakeToolchain::new();
    let script = fake.install("home-manager", "", "", 0);

    for args in [
        vec!["build", "--flake", "/tmp/x#me"],
        vec!["switch", "--flake", "/tmp/x#me"],
    ] {
        let status = Command::new(&script)
            .args(&args)
            .status()
            .expect("run fake home-manager");
        assert!(status.success());
    }

    let log = std::fs::read_to_string(fake.argv_log()).expect("read argv log");
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "both invocations must be recorded: {log}");
    assert_eq!(lines[0], "build --flake /tmp/x#me");
    assert_eq!(lines[1], "switch --flake /tmp/x#me");
}

/// A fake toolchain on `PATH` is resolvable by the real resolver without
/// touching any real store or the host's `PATH`.
#[test]
fn fake_toolchain_is_resolvable_by_the_real_resolver() {
    use nhs_core::{resolve_toolchain, HmInvocation};

    let fake = support::FakeToolchain::new();
    let expected = fake.install("home-manager", "", "", 0);

    // `RealFs`, because the fake script lives in a real temporary directory.
    // `PATH` is the fake directory only, so the store is never consulted.
    let toolchain = resolve_toolchain(&nhs_core::RealFs, &fake.env());
    match &toolchain.home_manager {
        HmInvocation::Binary(binary) => assert_eq!(binary, &expected),
        other => panic!("expected Binary from the fake PATH, got {other:?}"),
    }
}
