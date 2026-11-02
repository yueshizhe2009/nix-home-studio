//! CLI acceptance test: `nhs discover --json` against a temporary
//! `ProfileOnly` fixture (D5 primary acceptance target).

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

struct Fixture {
    home: PathBuf,
    config: PathBuf,
    state: PathBuf,
    store: PathBuf,
    /// Kept alive so the fixture tree outlives the test body.
    #[allow(dead_code)]
    dir: TempDir,
}

fn build_profile_only_fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("create fixture dir");
    let home = dir.path().join("home");
    let config = home.join(".config");
    let state = home.join(".local").join("state");
    let profiles = state.join("nix").join("profiles");
    fs::create_dir_all(&profiles).expect("create profiles dir");

    let store = dir.path().join("store");
    fs::create_dir_all(&store).expect("create store dir");
    for index in 1..=3 {
        let generation = store.join(format!("hm-gen{index}-home-manager-generation"));
        // Real generations hold `activate` and a `bin/home-manager-generation`
        // symlink, never the `home-manager` command itself.
        fs::create_dir_all(generation.join("bin")).expect("create generation bin");
        fs::write(generation.join("activate"), "#!/bin/sh\n").expect("write activate");
        symlink(
            generation.join("activate"),
            generation.join("bin").join("home-manager-generation"),
        )
        .expect("link generation activate");
    }
    // The command lives in its own store derivation, as on the audit host.
    let hm_store = store.join("86nwi02p-home-manager-0-unstable-2026-08-06");
    fs::create_dir_all(hm_store.join("bin")).expect("create hm store bin");
    fs::write(hm_store.join("bin").join("home-manager"), "#!/bin/sh\n")
        .expect("write home-manager");

    symlink(
        store.join("hm-gen3-home-manager-generation"),
        profiles.join("home-manager"),
    )
    .expect("link current generation");
    for index in 1..=3 {
        symlink(
            store.join(format!("hm-gen{index}-home-manager-generation")),
            profiles.join(format!("home-manager-{index}-link")),
        )
        .expect("link generation");
    }

    Fixture {
        home,
        config,
        store,
        state,
        dir,
    }
}

fn run_discover(fixture: &Fixture) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_nhs"))
        .args(["discover", "--json", "--home"])
        .arg(&fixture.home)
        .arg("--config-home")
        .arg(&fixture.config)
        .env("HOME", &fixture.home)
        .env("XDG_CONFIG_HOME", &fixture.config)
        .env("XDG_STATE_HOME", &fixture.state)
        .env("PATH", "/usr/bin")
        .env("USER", "tester")
        .env("NHS_STORE_ROOT", &fixture.store)
        .output()
        .expect("run nhs");

    assert!(
        output.status.success(),
        "nhs discover failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid JSON")
}

#[test]
fn discover_json_reports_profile_only_and_why_no_config_was_found() {
    let fixture = build_profile_only_fixture();
    let value = run_discover(&fixture);

    assert_eq!(value["schema"].as_u64(), Some(1));
    assert_eq!(value["layout"]["kind"], "profile-only");
    assert_eq!(value["main_file"], serde_json::Value::Null);
    assert_eq!(
        value["layout"]["profile_dir"].as_str(),
        Some(
            Path::new(&fixture.state)
                .join("nix")
                .join("profiles")
                .to_str()
                .unwrap()
        )
    );

    let generations = value["generations"].as_array().expect("generations array");
    assert_eq!(generations.len(), 3);

    let attempts = value["discovery_attempts"]
        .as_array()
        .expect("attempts array");
    let flake_attempt = attempts
        .iter()
        .find(|attempt| attempt["step"] == "config_home_flake")
        .expect("flake attempt recorded");
    assert_eq!(flake_attempt["accepted"], false);
    assert_eq!(flake_attempt["outcome"], "not present");

    assert_eq!(value["toolchain"]["home_manager"]["kind"], "from-store");
    let strategies: Vec<&str> = value["toolchain"]["attempts"]
        .as_array()
        .expect("toolchain attempts")
        .iter()
        .filter_map(|attempt| attempt["strategy"].as_str())
        .collect();
    for expected in [
        "path_binary",
        "generation_or_store",
        "profile_element",
        "nix_run",
        "absent",
    ] {
        assert!(
            strategies.contains(&expected),
            "toolchain attempts must name {expected}: {strategies:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// `--home` / `--config-home` precedence (documented in the module doc).
// ---------------------------------------------------------------------------

/// `--home` alone moves the derived config and state dirs.
#[test]
fn home_flag_moves_derived_config_and_state_dirs() {
    let fixture = build_profile_only_fixture();
    // No XDG_* exported: config and state are derived from the requested home.
    let value = Command::new(env!("CARGO_BIN_EXE_nhs"))
        .args(["discover", "--json", "--home"])
        .arg(&fixture.home)
        .env_clear()
        .env("HOME", "/nonexistent/should/be/overridden")
        .env("PATH", "/usr/bin")
        .env("USER", "tester")
        .env("NHS_STORE_ROOT", &fixture.store)
        .output()
        .map(|output| {
            assert!(
                output.status.success(),
                "nhs discover failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("valid JSON")
        })
        .expect("run nhs");

    assert_eq!(value["layout"]["kind"], "profile-only");
    assert_eq!(
        value["layout"]["profile_dir"].as_str(),
        Some(
            Path::new(&fixture.state)
                .join("nix")
                .join("profiles")
                .to_str()
                .expect("utf-8 path")
        ),
        "--home must drive the derived state directory"
    );
}

/// An exported `XDG_CONFIG_HOME` outranks `--home` for config discovery.
#[test]
fn xdg_config_home_outranks_the_home_flag() {
    let fixture = build_profile_only_fixture();
    let elsewhere = fixture.dir.path().join("elsewhere");

    let value = Command::new(env!("CARGO_BIN_EXE_nhs"))
        .args(["discover", "--json", "--home"])
        .arg(&fixture.home)
        .env_clear()
        .env("HOME", &fixture.home)
        .env("XDG_CONFIG_HOME", &elsewhere)
        .env("PATH", "/usr/bin")
        .env("USER", "tester")
        .env("NHS_STORE_ROOT", &fixture.store)
        .output()
        .map(|output| {
            assert!(output.status.success());
            serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("valid JSON")
        })
        .expect("run nhs");

    let attempts = value["discovery_attempts"]
        .as_array()
        .expect("attempts array");
    let flake = attempts
        .iter()
        .find(|attempt| attempt["step"] == "config_home_flake")
        .expect("flake attempt recorded");
    assert_eq!(
        flake["path"].as_str(),
        Some(
            elsewhere
                .join("home-manager")
                .join("flake.nix")
                .to_str()
                .expect("utf-8 path")
        ),
        "XDG_CONFIG_HOME must win over --home"
    );
}

/// `--config-home` overrides both `XDG_CONFIG_HOME` and `--home`.
#[test]
fn config_home_flag_outranks_xdg_and_home() {
    let fixture = build_profile_only_fixture();
    let explicit = fixture.dir.path().join("explicit-config");

    let value = Command::new(env!("CARGO_BIN_EXE_nhs"))
        .args(["discover", "--json", "--home"])
        .arg(&fixture.home)
        .arg("--config-home")
        .arg(&explicit)
        .env_clear()
        .env("HOME", &fixture.home)
        .env("XDG_CONFIG_HOME", fixture.dir.path().join("ignored"))
        .env("PATH", "/usr/bin")
        .env("USER", "tester")
        .env("NHS_STORE_ROOT", &fixture.store)
        .output()
        .map(|output| {
            assert!(output.status.success());
            serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("valid JSON")
        })
        .expect("run nhs");

    let attempts = value["discovery_attempts"]
        .as_array()
        .expect("attempts array");
    let flake = attempts
        .iter()
        .find(|attempt| attempt["step"] == "config_home_flake")
        .expect("flake attempt recorded");
    assert_eq!(
        flake["path"].as_str(),
        Some(
            explicit
                .join("home-manager")
                .join("flake.nix")
                .to_str()
                .expect("utf-8 path")
        ),
        "--config-home must win over both"
    );
}
