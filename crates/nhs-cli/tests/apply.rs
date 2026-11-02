//! M4.4 acceptance tests: `nhs plan`, `nhs validate`, `nhs apply`.
//!
//! Every build here is a shell script on a private `PATH`; no `nix` runs. The
//! script records the command it was given, which is how these tests can assert
//! *what would have run* — including the commands that must never be built.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use tempfile::TempDir;

const HOME_NIX: &str = "{ pkgs, ... }:\n{\n  imports = [ ];\n}\n";
const IMPORT_LINE: &str = "./modules/nhs-packages.nix";

struct Fixture {
    home: PathBuf,
    state: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    #[allow(dead_code)]
    dir: TempDir,
}

impl Fixture {
    /// A classic Home Manager tree and a fake `home-manager` that succeeds.
    fn new() -> Self {
        Self::with_script("printf 'build ok\\n'\nexit 0\n")
    }

    /// A tree whose fake `home-manager` fails with `stderr`.
    fn failing(stderr: &str) -> Self {
        Self::with_script(&format!("printf '%s\\n' '{stderr}' >&2\nexit 1\n"))
    }

    fn with_script(body: &str) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let config = home.join(".config").join("home-manager");
        fs::create_dir_all(config.join("modules")).expect("config dir");
        fs::write(config.join("home.nix"), HOME_NIX).expect("home.nix");
        let state = home.join(".local").join("state");
        fs::create_dir_all(&state).expect("state dir");
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).expect("bin dir");
        let log = dir.path().join("argv.log");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\t%s\\n' \"$PWD\" \"$*\" >> '{}'\n{body}",
            log.display()
        );
        let path = bin.join("home-manager");
        fs::write(&path, script).expect("write fake");
        let mut permissions = fs::metadata(&path).expect("stat fake").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod fake");

        Self {
            home,
            state,
            bin,
            log,
            dir,
        }
    }

    fn config_dir(&self) -> PathBuf {
        self.home.join(".config").join("home-manager")
    }

    fn module(&self) -> PathBuf {
        self.config_dir().join("modules").join("nhs-packages.nix")
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_nhs"))
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_STATE_HOME", &self.state)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("USER", "tester")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .expect("run nhs")
    }

    fn recorded(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// `plan` shows a diff and touches nothing.
#[test]
fn plan_prints_a_diff_and_writes_nothing() {
    let fixture = Fixture::new();
    let output = fixture.run(&["plan", "--packages", "firefox"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    let text = stdout(&output);
    assert!(text.contains("nhs-packages.nix"), "{text}");
    assert!(text.contains("home.packages"), "{text}");
    assert!(
        !fixture.module().exists(),
        "a plan must not create the module"
    );
    assert_eq!(
        fs::read_to_string(fixture.config_dir().join("home.nix")).expect("home.nix"),
        HOME_NIX,
        "a plan must not edit the main file"
    );
    assert!(fixture.recorded().is_empty(), "a plan must not run a build");
}

/// `plan --json` gives a UI something stable to render.
#[test]
fn plan_json_lists_the_changes() {
    let fixture = Fixture::new();
    let output = fixture.run(&["plan", "--json", "--packages", "firefox,ripgrep"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value: serde_json::Value = serde_json::from_str(&stdout(&output)).expect("plan emits JSON");
    assert_eq!(value["schema"], 1);
    let changes = value["changes"].as_array().expect("changes array");
    assert_eq!(changes.len(), 2, "{value}");
    assert_eq!(changes[0]["kind"], "create");
    assert!(
        changes[0]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("modules/nhs-packages.nix")),
        "{value}"
    );
    assert_eq!(changes[1]["kind"], "replace");
    assert!(
        changes[0]["unified_diff"]
            .as_str()
            .is_some_and(|diff| diff.contains("pkgs.firefox")),
        "{value}"
    );
}

/// `apply` validates in a copy, writes the module and the import, and stops —
/// it never activates.
#[test]
fn apply_writes_the_module_and_the_import_without_activating() {
    let fixture = Fixture::new();
    let output = fixture.run(&["apply", "--packages", "firefox"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    let module = fs::read_to_string(fixture.module()).expect("module written");
    assert!(module.contains("pkgs.firefox"), "{module}");
    let main = fs::read_to_string(fixture.config_dir().join("home.nix")).expect("home.nix");
    assert!(main.contains(IMPORT_LINE), "{main}");
    assert!(
        main.contains("imports = [ ./modules/nhs-packages.nix ]"),
        "{main}"
    );

    let text = stdout(&output);
    assert!(text.contains("applied snapshot"), "{text}");
    assert!(
        text.contains("no activation was performed"),
        "apply must say that nothing was activated: {text}"
    );

    // The build ran once, as a build.
    let recorded = fixture.recorded();
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert!(recorded[0].contains("build"), "{recorded:?}");
    assert!(!recorded[0].contains("switch"), "{recorded:?}");
}

/// Applying the same selection twice is a no-op the second time.
#[test]
fn a_second_apply_is_nothing_to_do() {
    let fixture = Fixture::new();
    let first = fixture.run(&["apply", "--packages", "firefox"]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));

    let second = fixture.run(&["apply", "--packages", "firefox"]);
    assert_eq!(second.status.code(), Some(2), "{}", stderr(&second));
    assert!(
        fixture.recorded().len() == 1,
        "the second run must not build again: {:?}",
        fixture.recorded()
    );
}

/// A failing build leaves the configuration alone and says so.
#[test]
fn apply_refuses_to_write_when_the_build_fails() {
    let fixture = Fixture::failing("error: attribute 'nope' missing");
    let output = fixture.run(&["apply", "--packages", "firefox"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));

    assert!(
        !fixture.module().exists(),
        "a failed build must not create the module"
    );
    assert_eq!(
        fs::read_to_string(fixture.config_dir().join("home.nix")).expect("home.nix"),
        HOME_NIX,
        "a failed build must not edit the main file"
    );
    let text = stdout(&output) + &stderr(&output);
    assert!(text.contains("missing-attribute"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");
}

/// `validate` runs a build and says so, and it never plans an activation.
#[test]
fn validate_plans_a_build_and_never_a_switch() {
    let fixture = Fixture::new();
    let output = fixture.run(&["validate", "--packages", "firefox"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    let recorded = fixture.recorded();
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert!(recorded[0].contains("build"), "{recorded:?}");
    assert!(
        !recorded[0].contains("switch"),
        "validation must never activate: {recorded:?}"
    );
    // Nothing was written to the configuration.
    assert!(!fixture.module().exists());
}

/// `validate --json` classifies the failure for a UI.
#[test]
fn validate_json_reports_the_classified_failure() {
    let fixture = Fixture::failing("error: attribute 'nope' missing");
    let output = fixture.run(&["validate", "--json", "--packages", "firefox"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let value: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("validate emits JSON");
    assert_eq!(value["ok"], false, "{value}");
    assert_eq!(value["kind"], "missing-attribute", "{value}");
    assert_eq!(value["activated"], serde_json::Value::Null);
    assert!(
        value["scratch"]
            .as_str()
            .is_some_and(|path| !path.is_empty()),
        "a failed validation keeps the copy and names it: {value}"
    );
}

/// `--switch` is refused before anything is read, and it runs nothing at all.
#[test]
fn switch_is_refused_and_runs_nothing() {
    let fixture = Fixture::new();
    let output = fixture.run(&["apply", "--switch", "--packages", "firefox"]);
    assert_eq!(
        output.status.code(),
        Some(3),
        "a policy refusal exits 3: {}",
        stderr(&output)
    );

    let message = stderr(&output) + &stdout(&output);
    assert!(message.contains("not available"), "{message}");
    assert!(
        message.contains("home-manager switch"),
        "the refusal must show the exact command it would run: {message}"
    );
    assert!(
        fixture.recorded().is_empty(),
        "a refused switch must not start any process: {:?}",
        fixture.recorded()
    );
    assert!(!fixture.module().exists(), "a refusal must not write");
    assert_eq!(
        fs::read_to_string(fixture.config_dir().join("home.nix")).expect("home.nix"),
        HOME_NIX
    );
}

/// The refusal must not depend on the filesystem at all: the same message for a
/// real configuration and for a home directory that does not exist.
///
/// This is what distinguishes "refused before doing anything" from "detected the
/// layout, then refused". The previous implementation called `detect()`, so the
/// message named the real `home.nix` path here and said something else when no
/// configuration was found; both runs must now be byte-identical.
#[test]
fn switch_is_refused_without_reading_the_filesystem() {
    let fixture = Fixture::new();
    let real = fixture.run(&["apply", "--switch", "--packages", "firefox"]);

    let missing_home = fixture.dir.path().join("does-not-exist");
    let absent = Command::new(env!("CARGO_BIN_EXE_nhs"))
        .args(["apply", "--switch", "--packages", "firefox"])
        .env("HOME", &missing_home)
        .env("XDG_STATE_HOME", &fixture.state)
        .env("PATH", format!("{}:/usr/bin:/bin", fixture.bin.display()))
        .env("USER", "tester")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("run nhs");

    assert_eq!(real.status.code(), Some(3));
    assert_eq!(absent.status.code(), Some(3));
    assert_eq!(
        stderr(&real),
        stderr(&absent),
        "the refusal text must not depend on what is on disk"
    );

    // And it names no path from the fixture, so nothing was discovered.
    let message = stderr(&real);
    assert!(!message.contains("missing-attribute"), "{message}");
    assert!(
        !message.contains(&fixture.home.display().to_string()),
        "the refusal must not name a discovered path: {message}"
    );
    assert!(
        !message.contains("profile-only") && !message.contains("classic-home"),
        "the refusal must not report a detected layout: {message}"
    );
}

/// `--state-dir` is the final state directory, not an `XDG_STATE_HOME` parent.
#[test]
fn state_dir_flag_is_the_final_state_directory() {
    let fixture = Fixture::new();
    let state = fixture.dir.path().join("explicit-state");
    let output = fixture.run(&[
        "apply",
        "--packages",
        "firefox",
        "--state-dir",
        state.to_str().expect("utf-8 path"),
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        state.join("snapshots").is_dir(),
        "--state-dir must hold the snapshots directly: {:?}",
        fs::read_dir(&state).map(|entries| entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect::<Vec<_>>())
    );
    assert!(
        !state.join("nix-home-studio").exists(),
        "no `nix-home-studio` level may be added under --state-dir"
    );
}

/// `--yes` does not unlock `--switch` (G4 at the CLI level).
#[test]
fn switch_with_yes_is_still_refused() {
    let fixture = Fixture::new();
    let output = fixture.run(&["apply", "--switch", "--yes", "--packages", "firefox"]);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(fixture.recorded().is_empty());
}

/// `--yes` without `--switch` is a usage error, not a silent no-op.
#[test]
fn yes_without_switch_is_a_usage_error() {
    let fixture = Fixture::new();
    let output = fixture.run(&["apply", "--yes", "--packages", "firefox"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stderr(&output).contains("--yes"), "{}", stderr(&output));
}

/// A hand-edited managed block is refused by the CLI with exit 3, and the file
/// is left alone.
#[test]
fn a_hand_modified_module_is_refused() {
    let fixture = Fixture::new();
    // Create the module through a successful apply …
    let first = fixture.run(&["apply", "--packages", "firefox"]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));

    // … then edit inside the markers, in a shape the generator cannot re-emit.
    let module = fs::read_to_string(fixture.module()).expect("module");
    let edited = module.replace(
        "    pkgs.firefox\n",
        "    pkgs.firefox\n    (import ./local.nix { })\n",
    );
    assert_ne!(edited, module);
    fs::write(fixture.module(), &edited).expect("edit module");

    let output = fixture.run(&["apply", "--packages", "ripgrep"]);
    assert_eq!(
        output.status.code(),
        Some(3),
        "a hand-modified block is a refusal: {}",
        stderr(&output)
    );
    assert!(stderr(&output).contains("line"), "{}", stderr(&output));
    assert_eq!(
        fs::read_to_string(fixture.module()).expect("module"),
        edited,
        "the refused file must not be modified"
    );
}

/// A system-only option cannot even be expressed by the CLI: settings are
/// created with `Scope::Home`, and unknown NixOS-only names are refused by
/// `check_target` only when a caller asks for one. This test pins the CLI half:
/// an unknown option path still reaches the plan (the build is what rejects it).
#[test]
fn a_setting_reaches_the_plan_as_written() {
    let fixture = Fixture::new();
    let output = fixture.run(&[
        "plan",
        "--packages",
        "firefox",
        "--set",
        "programs.git.enable=true",
        "--set",
        "programs.git.userName=\"Ada\"",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("programs.git.enable = true;"), "{text}");
    assert!(text.contains("programs.git.userName = \"Ada\";"), "{text}");
}

/// Bad `--set` values are usage errors, not refusals and not panics.
#[test]
fn a_bad_setting_value_is_a_usage_error() {
    let fixture = Fixture::new();
    for bad in ["no-equals", "a=", "a=not-json", "a={\"nested\":1}"] {
        let output = fixture.run(&["plan", "--set", bad]);
        assert_eq!(
            output.status.code(),
            Some(1),
            "`--set {bad}` should be a usage error: {}",
            stderr(&output)
        );
    }
}

/// An unknown subcommand is a usage error with the usage text.
#[test]
fn an_unknown_subcommand_prints_usage() {
    let fixture = Fixture::new();
    let output = fixture.run(&["frobnicate"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("usage:"), "{}", stderr(&output));
}
