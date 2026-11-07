//! Clean-host portability: an empty HOME must explain how to create a
//! configuration, while an existing one must not show that hint.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nhs"))
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("USER", "tester")
        .env("LOGNAME", "tester")
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run nhs")
}

fn home() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let home = dir.path().join("home");
    fs::create_dir_all(&home).expect("create HOME");
    (dir, home)
}

#[test]
fn an_empty_home_explains_how_to_create_a_configuration() {
    let (_dir, home) = home();

    let discover = run(&home, &["discover"]);
    assert!(discover.status.success());
    let output = String::from_utf8_lossy(&discover.stdout);
    assert!(output.contains("layout: unknown"), "{output}");
    assert!(output.contains("hint:"), "{output}");
    assert!(output.contains("home-manager init"), "{output}");

    let plan = run(&home, &["plan", "--packages", "hello"]);
    assert_eq!(plan.status.code(), Some(3));
    let error = String::from_utf8_lossy(&plan.stderr);
    assert!(error.contains("hint:"), "{error}");
    assert!(error.contains("home-manager init"), "{error}");

    assert!(!home.join(".local/state/nix-home-studio").exists());
}

#[test]
fn an_existing_configuration_does_not_get_the_creation_hint() {
    let (_dir, home) = home();
    let config = home.join(".config/home-manager");
    fs::create_dir_all(&config).expect("create config dir");
    fs::write(
        config.join("home.nix"),
        "{ ... }:\n{\n  home.stateVersion = \"24.05\";\n}\n",
    )
    .expect("write home.nix");

    let discover = run(&home, &["discover"]);
    assert!(discover.status.success());
    let output = String::from_utf8_lossy(&discover.stdout);
    assert!(output.contains("layout: classic-home"), "{output}");
    assert!(!output.contains("hint:"), "{output}");
}
