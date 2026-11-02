//! M4.3 acceptance tests: validation happens in a copy, never in the live tree.
//!
//! These tests use the **real** filesystem in temporary directories, because the
//! property under test is about real files: whether a symlink was materialised,
//! whether the live bytes changed, whether the copy was actually removed. The
//! build itself is a recorded fake — no `nix` is run.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::fs;
use std::os::unix::fs::symlink;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Mutex;

use nhs_core::discovery::Layout;
use nhs_core::edit::{EditEngine, StateLayout};
use nhs_core::proc::{CancelToken, CommandSpec, Outcome, RunError, Runner, Stream};
use nhs_core::validation::{validate, validate_with_limits, Limits, ValidationError};
use nhs_core::RealFs;

/// What the build could see from its working directory, captured while it runs
/// (the copy is removed on success, so it cannot be inspected afterwards).
#[derive(Debug, Clone, Default)]
struct Observation {
    home: Option<String>,
    home_is_symlink: bool,
}

/// A runner that records the command, looks at the copy, and returns a scripted
/// result.
struct ScriptedRunner {
    exit_code: i32,
    stderr: String,
    seen: Mutex<Vec<CommandSpec>>,
    observed: Mutex<Vec<Observation>>,
}

impl ScriptedRunner {
    fn succeeding() -> Self {
        Self {
            exit_code: 0,
            stderr: String::new(),
            seen: Mutex::new(Vec::new()),
            observed: Mutex::new(Vec::new()),
        }
    }

    fn failing(kind: &str) -> Self {
        Self {
            exit_code: 1,
            stderr: format!("error: {kind}"),
            seen: Mutex::new(Vec::new()),
            observed: Mutex::new(Vec::new()),
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        self.seen.lock().expect("seen").clone()
    }

    fn observations(&self) -> Vec<Observation> {
        self.observed.lock().expect("observed").clone()
    }
}

impl Runner for ScriptedRunner {
    fn run(
        &self,
        spec: &CommandSpec,
        _cancel: &CancelToken,
        _sink: &mut dyn FnMut(Stream, &str),
    ) -> Result<Outcome, RunError> {
        self.seen.lock().expect("seen").push(spec.clone());
        let home = spec.cwd.join("home.nix");
        self.observed.lock().expect("observed").push(Observation {
            home: fs::read_to_string(&home).ok(),
            home_is_symlink: fs::symlink_metadata(&home)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false),
        });
        Ok(Outcome {
            status: ExitStatus::from_raw(self.exit_code << 8),
            cancelled: false,
            timed_out: false,
            stdout: String::new(),
            stderr: self.stderr.clone(),
            truncated: false,
        })
    }
}

/// A classic Home Manager tree with one module, plus a matching layout.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("home-manager");
        fs::create_dir_all(root.join("modules")).expect("root");
        fs::write(root.join("home.nix"), "{\n  imports = [ ];\n}\n").expect("home.nix");
        fs::write(
            root.join("modules/nhs-packages.nix"),
            "{\n  home.packages = [ ];\n}\n",
        )
        .expect("module");
        let state = dir.path().join("state");
        fs::create_dir_all(&state).expect("state");
        Self {
            _dir: dir,
            root,
            state,
        }
    }

    fn layout(&self) -> Layout {
        Layout::ClassicHome {
            main_file: self.root.join("home.nix"),
        }
    }

    fn engine(&self) -> EditEngine<'_> {
        EditEngine::new(
            &RealFs,
            StateLayout {
                state_dir: self.state.clone(),
                config_root: self.root.clone(),
            },
        )
    }

    /// A plan that rewrites `home.nix` with one extra import.
    fn plan(&self) -> nhs_core::Plan {
        let engine = self.engine();
        let change = engine
            .stage(
                &self.root.join("home.nix"),
                "{\n  imports = [ ./modules/nhs-packages.nix ];\n}\n",
            )
            .expect("stage");
        engine.plan(vec![change])
    }

    fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.root.join(relative)).expect("read")
    }
}

fn validate_fixture(
    fixture: &Fixture,
    runner: &ScriptedRunner,
) -> Result<nhs_core::ValidationReport, ValidationError> {
    let mut sink = |_: Stream, _: &str| {};
    validate(
        &RealFs,
        &fixture.layout(),
        &fixture.plan(),
        &fixture.state,
        runner,
        &CancelToken::new(),
        &mut sink,
    )
}

/// The build runs in a copy that contains the planned content, and the live tree
/// is byte-identical afterwards.
#[test]
fn validate_runs_against_a_scratch_copy_and_never_the_live_root() {
    let fixture = Fixture::new();
    let original = fixture.read("home.nix");
    let runner = ScriptedRunner::succeeding();

    let report = validate_fixture(&fixture, &runner).expect("validate");
    assert!(report.success, "{report:?}");

    let commands = runner.commands();
    assert_eq!(commands.len(), 1, "exactly one build");
    let cwd = &commands[0].cwd;
    assert_ne!(
        cwd, &fixture.root,
        "the build must not run in the live tree"
    );
    assert!(
        cwd.starts_with(&fixture.state),
        "the build directory must live under the state dir: {}",
        cwd.display()
    );
    // The copy had the plan applied — observed while the build was running,
    // because a successful run removes it.
    let observed = runner.observations();
    let planned = observed[0].home.clone().expect("the build saw home.nix");
    assert!(
        planned.contains("./modules/nhs-packages.nix"),
        "the copy must contain the planned content:\n{planned}"
    );
    // The live tree does not.
    assert_eq!(fixture.read("home.nix"), original, "the live tree changed");

    // Success removes the copy.
    assert!(report.kept_scratch.is_none());
    assert!(!cwd.exists(), "the scratch copy must be removed on success");
}

/// A failed build leaves the live tree alone, keeps the copy, and records the
/// attempt.
#[test]
fn a_failed_build_leaves_files_in_place_and_records_a_failed_snapshot() {
    let fixture = Fixture::new();
    let original = fixture.read("home.nix");
    let runner = ScriptedRunner::failing("hash mismatch in fixed-output derivation");

    let report = validate_fixture(&fixture, &runner).expect("validate ran");
    assert!(!report.success);

    // Live tree untouched.
    assert_eq!(fixture.read("home.nix"), original, "the live tree changed");

    // The copy is kept, with the planned content, so the failure can be
    // inspected.
    let kept = report.kept_scratch.clone().expect("the copy is kept");
    assert!(kept.exists(), "the kept copy must exist");
    let planned = fs::read_to_string(kept.join("root/home.nix")).expect("read kept copy");
    assert!(planned.contains("./modules/nhs-packages.nix"));

    // The attempt is recorded.
    let record_path = report.record.clone().expect("a record is written");
    assert!(record_path.exists(), "the record must exist");
    let record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&record_path).expect("read record"))
            .expect("parse record");
    assert_eq!(record["format_version"], 1);
    assert_eq!(record["kind"], "hash-mismatch");
    assert!(
        record["scratch"]
            .as_str()
            .is_some_and(|scratch| scratch == kept.display().to_string()),
        "the record must name the kept copy: {record}"
    );
    assert!(
        record["planned"]
            .as_array()
            .is_some_and(|planned| !planned.is_empty()),
        "the record must list what the plan would have changed"
    );

    // The classifier's advice reaches the report.
    let diagnosis = report.diagnosis.expect("a diagnosis");
    assert_eq!(diagnosis.kind, nhs_core::classify::ErrorKind::HashMismatch);
    assert!(!diagnosis.hint.is_empty());
}

/// A symlink in the configuration is materialised in the copy, so a build that
/// writes through it cannot reach the real file.
#[test]
fn a_symlink_is_materialised_so_the_copy_cannot_write_through() {
    let dir = tempfile::tempdir().expect("temp dir");
    let outside = dir.path().join("dotfiles");
    fs::create_dir_all(&outside).expect("dotfiles");
    let real_home = outside.join("home.nix");
    fs::write(&real_home, "# the real file, outside the config root\n").expect("write");

    let root = dir.path().join("config");
    fs::create_dir_all(&root).expect("root");
    symlink(&real_home, root.join("home.nix")).expect("symlink");
    let state = dir.path().join("state");
    fs::create_dir_all(&state).expect("state");

    let fixture = Fixture {
        _dir: dir,
        root,
        state,
    };
    // Staging follows the link, so the plan's change carries `resolved_from`.
    let plan = fixture.plan();
    assert!(
        plan.changes[0].resolved_from.is_some(),
        "the plan must remember the link"
    );

    let runner = ScriptedRunner::succeeding();
    let mut sink = |_: Stream, _: &str| {};
    let report = validate(
        &RealFs,
        &fixture.layout(),
        &plan,
        &fixture.state,
        &runner,
        &CancelToken::new(),
        &mut sink,
    )
    .expect("validate");
    assert!(report.success, "{report:?}");

    let observed = runner.observations();
    assert!(
        !observed[0].home_is_symlink,
        "the copy must hold a regular file, not a link"
    );
    assert!(
        observed[0]
            .home
            .as_deref()
            .is_some_and(|text| text.contains("./modules/nhs-packages.nix")),
        "the copy must have the plan applied"
    );
    assert_eq!(
        fs::read_to_string(&real_home).expect("read real"),
        "# the real file, outside the config root\n",
        "the file the link pointed at must be untouched"
    );
}

/// Copying more files than allowed is refused before anything is built.
#[test]
fn the_file_count_limit_is_refused() {
    let fixture = Fixture::new();
    let runner = ScriptedRunner::succeeding();
    let mut sink = |_: Stream, _: &str| {};
    let error = validate_with_limits(
        &RealFs,
        &fixture.layout(),
        &fixture.plan(),
        &fixture.state,
        &runner,
        &CancelToken::new(),
        &mut sink,
        Limits {
            max_files: 1,
            max_bytes: 1 << 30,
        },
    )
    .expect_err("two files exceed the limit");
    assert!(matches!(error, ValidationError::TooLarge { .. }), "{error}");
    assert!(
        runner.commands().is_empty(),
        "the build must not run when the copy was refused"
    );
    // The half-copied scratch tree is removed.
    let scratch_dir = fixture.state.join("scratch");
    let left = fs::read_dir(&scratch_dir)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(left, 0, "a refused copy must not leave a scratch tree");
}

/// The byte limit is enforced as well.
#[test]
fn the_byte_limit_is_enforced() {
    let fixture = Fixture::new();
    let runner = ScriptedRunner::succeeding();
    let mut sink = |_: Stream, _: &str| {};
    let error = validate_with_limits(
        &RealFs,
        &fixture.layout(),
        &fixture.plan(),
        &fixture.state,
        &runner,
        &CancelToken::new(),
        &mut sink,
        Limits {
            max_files: 100,
            max_bytes: 8,
        },
    )
    .expect_err("the tree is larger than eight bytes");
    assert!(matches!(error, ValidationError::TooLarge { .. }), "{error}");
}

/// A layout with no build command is refused by name.
#[test]
fn an_unsupported_layout_is_refused() {
    let fixture = Fixture::new();
    let runner = ScriptedRunner::succeeding();
    let mut sink = |_: Stream, _: &str| {};
    let layout = Layout::ProfileOnly {
        profile_dir: fixture.root.clone(),
        generations: Vec::new(),
    };
    let error = validate(
        &RealFs,
        &layout,
        &fixture.plan(),
        &fixture.state,
        &runner,
        &CancelToken::new(),
        &mut sink,
    )
    .expect_err("profile-only has nothing to build");
    match error {
        ValidationError::UnsupportedLayout { kind, reason } => {
            assert_eq!(kind, "profile-only");
            assert!(!reason.is_empty());
        }
        other => panic!("expected UnsupportedLayout, got {other}"),
    }
}

/// Validation only ever plans a build: the argv is `build …`, never `switch`.
#[test]
fn the_build_front_door_is_a_build_and_never_a_switch() {
    let fixture = Fixture::new();
    let runner = ScriptedRunner::succeeding();
    validate_fixture(&fixture, &runner).expect("validate");

    let commands = runner.commands();
    let args: Vec<String> = commands[0]
        .args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args.first().map(String::as_str), Some("build"));
    assert!(
        !args.iter().any(|arg| arg.contains("switch")),
        "validation must never plan an activation: {args:?}"
    );
    assert_eq!(commands[0].program, nhs_core::Program::HomeManager);
    assert_eq!(args.get(1).map(String::as_str), Some("-f"));

    // A flake layout uses `nix build --dry-run` with the structured log.
    let flake = Layout::FlakeHome {
        root: fixture.root.clone(),
        main_file: fixture.root.join("home.nix"),
        attr: "tester".to_string(),
    };
    let runner = ScriptedRunner::succeeding();
    let mut sink = |_: Stream, _: &str| {};
    validate(
        &RealFs,
        &flake,
        &fixture.plan(),
        &fixture.state,
        &runner,
        &CancelToken::new(),
        &mut sink,
    )
    .expect("validate flake");
    let commands = runner.commands();
    let args: Vec<String> = commands[0]
        .args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(commands[0].program, nhs_core::Program::Nix);
    assert_eq!(args.first().map(String::as_str), Some("build"));
    assert!(args.iter().any(|arg| arg == "--dry-run"));
    assert!(args.iter().any(|arg| arg == "internal-json"));
    assert!(args
        .iter()
        .any(|arg| arg == ".#homeConfigurations.tester.activationPackage"));
    assert!(!args.iter().any(|arg| arg.contains("switch")));
}

/// The record is only produced for failures; a success leaves no debris behind.
#[test]
fn a_successful_validation_writes_no_record() {
    let fixture = Fixture::new();
    let runner = ScriptedRunner::succeeding();
    let report = validate_fixture(&fixture, &runner).expect("validate");
    assert!(report.success);
    assert!(report.record.is_none());
    assert!(
        !fixture.state.join("validations").exists(),
        "a successful run must not create an attempt record"
    );
    assert_eq!(
        fs::read_dir(fixture.state.join("scratch"))
            .map(|entries| entries.count())
            .unwrap_or(0),
        0
    );
}

/// The copy is made of regular files only, so the build cannot follow a link out
/// of the sandbox even if it tries.
#[test]
fn every_entry_in_the_copy_is_a_regular_file_or_directory() {
    let fixture = Fixture::new();
    let runner = ScriptedRunner::succeeding();
    let mut sink = |_: Stream, _: &str| {};
    let report = validate(
        &RealFs,
        &fixture.layout(),
        &fixture.plan(),
        &fixture.state,
        &runner,
        &CancelToken::new(),
        &mut sink,
    )
    .expect("validate");
    assert!(report.success);

    // Re-run with a failing build so the copy is kept and can be inspected.
    let runner = ScriptedRunner::failing("attribute 'nope' missing");
    let report = validate_fixture(&fixture, &runner).expect("validate");
    let kept = report.kept_scratch.expect("kept");
    let mut pending = vec![kept.join("root")];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).expect("read dir") {
            let entry = entry.expect("entry");
            let metadata = fs::symlink_metadata(entry.path()).expect("stat");
            assert!(
                !metadata.file_type().is_symlink(),
                "the copy must not contain a symlink: {}",
                entry.path().display()
            );
            if metadata.is_dir() {
                pending.push(entry.path());
            }
        }
    }
}

/// `Plan` paths that are outside the tree and not reached through a link are
/// refused instead of being silently mapped somewhere else.
#[test]
fn a_plan_path_outside_the_tree_is_refused() {
    let fixture = Fixture::new();
    let mut plan = fixture.plan();
    plan.changes[0].path = PathBuf::from("/somewhere/else/home.nix");
    plan.changes[0].resolved_from = None;

    let runner = ScriptedRunner::succeeding();
    let mut sink = |_: Stream, _: &str| {};
    let error = validate(
        &RealFs,
        &fixture.layout(),
        &plan,
        &fixture.state,
        &runner,
        &CancelToken::new(),
        &mut sink,
    )
    .expect_err("a path outside the root cannot be validated");
    assert!(
        matches!(error, ValidationError::OutsideRoot { .. }),
        "{error}"
    );
}

/// The kept copy path is absolute, so a GUI can open it without guessing.
#[test]
fn the_kept_copy_path_is_usable() {
    let fixture = Fixture::new();
    let runner = ScriptedRunner::failing("syntax error, unexpected end of file");
    let report = validate_fixture(&fixture, &runner).expect("validate");
    let kept: &Path = report.kept_scratch.as_deref().expect("kept");
    assert!(kept.is_absolute(), "{}", kept.display());
    assert!(kept.join("root/home.nix").is_file());
    assert_eq!(
        report.diagnosis.map(|d| d.kind),
        Some(nhs_core::classify::ErrorKind::SyntaxError)
    );
}
