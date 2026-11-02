//! M4.1 acceptance tests: the runner, with **real** child processes.
//!
//! The fake toolchain here is a shell script on a private `PATH`, exactly as in
//! M1: no global environment is mutated, and the parent's `PATH` is untouched.
//! Each test proves something the type system cannot: that output arrives while
//! the process is still running, and that stopping a run takes the whole process
//! group with it — including the grandchild a build spawns.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use nhs_core::env::EnvView;
use nhs_core::proc::{
    is_running, CancelToken, CommandSpec, Outcome, Program, RealRunner, RunError, Runner, Stream,
};

/// A private directory holding fake programs, plus the matching `EnvView`.
struct Harness {
    dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("temp dir"),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Install a script as an allowlisted program.
    fn install(&self, name: &str, body: &str) -> PathBuf {
        let path = self.path().join(name);
        fs::write(&path, format!("#!/bin/sh\nset -u\n{body}\n")).expect("write script");
        let mut permissions = fs::metadata(&path).expect("stat").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod");
        path
    }

    fn env(&self) -> EnvView {
        EnvView {
            home: PathBuf::from("/home/tester"),
            user: Some("tester".to_string()),
            path_entries: vec![self.path().to_path_buf()],
            xdg_config_home: None,
            xdg_state_home: None,
            store_root: PathBuf::from("/nix/store"),
        }
    }

    fn workdir(&self) -> PathBuf {
        let dir = self.path().join("work");
        fs::create_dir_all(&dir).expect("work dir");
        dir
    }
}

/// Run once, retrying a spawn that failed for a transient reason.
///
/// `fork` can fail with `EAGAIN` when the machine is loaded (the full workspace
/// suite runs every test binary in parallel); the application reports that as a
/// spawn error, which is correct, but a test asserting on the *program's* output
/// should not turn that into a flake.
fn run(harness: &Harness, spec: &CommandSpec) -> Result<Outcome, RunError> {
    let env = harness.env();
    let runner = RealRunner::new(&env);
    let mut sink = |_: Stream, _: &str| {};
    let first = runner.run(spec, &CancelToken::new(), &mut sink);
    match first {
        Err(RunError::Spawn { .. }) => {
            thread::sleep(Duration::from_millis(50));
            runner.run(spec, &CancelToken::new(), &mut sink)
        }
        other => other,
    }
}

/// The allowlist is closed: no privilege escalation, no shell, no other program.
#[test]
fn a_forbidden_program_cannot_be_expressed() {
    for forbidden in [
        "nixos-rebuild",
        "sudo",
        "sh",
        "bash",
        "",
        "nix ",
        "NIX",
        "./nix",
        "nix-daemon",
    ] {
        assert_eq!(
            Program::from_name(forbidden),
            None,
            "`{forbidden}` must not be expressible as a Program"
        );
    }
    assert_eq!(Program::from_name("nix"), Some(Program::Nix));
    assert_eq!(
        Program::from_name("home-manager"),
        Some(Program::HomeManager)
    );
    assert_eq!(
        Program::ALL.len(),
        2,
        "the allowlist has exactly two programs"
    );
}

/// A run happens in the directory it was given, with the arguments it was given.
#[test]
fn a_command_runs_in_the_given_directory_with_structured_arguments() {
    let harness = Harness::new();
    let work = harness.workdir();
    harness.install(
        "nix",
        "printf 'pwd=%s\\n' \"$(pwd)\"\nprintf 'argc=%s\\n' \"$#\"\nprintf 'arg1=%s\\n' \"$1\"",
    );

    let spec = CommandSpec::new(Program::Nix, &work).args(["build", "--dry-run"]);
    let outcome = run(&harness, &spec).expect("run");

    assert!(outcome.success(), "{outcome:?}");
    let mut lines = outcome.stdout.lines();
    assert_eq!(
        lines.next(),
        Some(format!("pwd={}", work.display()).as_str())
    );
    assert_eq!(lines.next(), Some("argc=2"));
    assert_eq!(lines.next(), Some("arg1=build"));
    assert!(outcome.stderr.is_empty());
}

/// A non-zero exit is data, not an error: the classifier needs the status.
#[test]
fn a_failing_exit_status_is_returned_with_its_stderr() {
    let harness = Harness::new();
    harness.install("nix", "echo 'error: hash mismatch' >&2\nexit 3");
    let spec = CommandSpec::new(Program::Nix, harness.workdir());

    let outcome =
        run(&harness, &spec).unwrap_or_else(|error| panic!("the run itself failed: {error}"));
    assert!(!outcome.success());
    assert_eq!(outcome.status.code(), Some(3));
    assert_eq!(outcome.stderr, "error: hash mismatch");
    assert!(!outcome.cancelled && !outcome.timed_out);
}

/// A program that is not installed is reported by name, not by a panic.
#[test]
fn a_missing_program_is_reported() {
    let harness = Harness::new();
    let env = EnvView {
        path_entries: vec![harness.path().to_path_buf()],
        ..harness.env()
    };
    let runner = RealRunner::new(&env);
    let spec = CommandSpec::new(Program::Nix, harness.workdir());
    let mut sink = |_: Stream, _: &str| {};
    match runner.run(&spec, &CancelToken::new(), &mut sink) {
        Err(RunError::NotFound { program }) => assert_eq!(program, Program::Nix),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

/// Output must reach the caller while the process is still running — a build log
/// that only appears at the end is useless to a UI.
#[test]
fn output_is_streamed_before_the_process_exits() {
    let harness = Harness::new();
    harness.install("nix", "echo first\nsleep 0.4\necho second");
    let spec = CommandSpec::new(Program::Nix, harness.workdir());

    let env = harness.env();
    let runner = RealRunner::new(&env);
    let (sender, receiver) = mpsc::channel();
    let mut sink = |_: Stream, line: &str| {
        sender.send((line.to_string(), Instant::now())).ok();
    };
    let outcome = runner
        .run(&spec, &CancelToken::new(), &mut sink)
        .expect("run");
    drop(sender);

    let seen: Vec<(String, Instant)> = receiver.iter().collect();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0].0, "first");
    assert_eq!(seen[1].0, "second");
    let gap = seen[1].1.duration_since(seen[0].1);
    assert!(
        gap >= Duration::from_millis(300),
        "the second line arrived after {gap:?}; it was buffered until exit"
    );
    assert_eq!(outcome.stdout, "first\nsecond");
}

/// Cancelling kills the whole group: the program *and* the grandchild it started.
#[test]
fn cancellation_stops_the_child_process_group() {
    let harness = Harness::new();
    harness.install(
        "nix",
        "sleep 30 &\necho \"$!\" > grandchild.pid\necho \"$$\" > child.pid\nsleep 30",
    );
    let work = harness.workdir();
    let spec = CommandSpec::new(Program::Nix, &work)
        .with_timeout(Duration::from_secs(60))
        .with_kill_grace(Duration::from_millis(300));

    let env = harness.env();
    let runner = RealRunner::new(&env);
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        trigger.cancel();
    });

    let mut sink = |_: Stream, _: &str| {};
    let outcome = runner.run(&spec, &cancel, &mut sink).expect("run");
    assert!(outcome.cancelled, "the run must report cancellation");

    let child = read_pid(&work.join("child.pid"));
    let grandchild = read_pid(&work.join("grandchild.pid"));
    assert!(
        wait_until_gone(child, Duration::from_secs(3)),
        "child {child} survived"
    );
    assert!(
        wait_until_gone(grandchild, Duration::from_secs(3)),
        "grandchild {grandchild} survived"
    );
}

/// A cancelled run must not poison the next run of the same session.
#[test]
fn resetting_the_token_allows_another_run() {
    let harness = Harness::new();
    harness.install("nix", "echo again");
    let work = harness.workdir();
    let spec = CommandSpec::new(Program::Nix, &work).with_timeout(Duration::from_secs(10));

    let env = harness.env();
    let runner = RealRunner::new(&env);
    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(cancel.is_cancelled());

    cancel.reset();
    assert!(!cancel.is_cancelled(), "reset must clear the flag");

    let mut sink = |_: Stream, _: &str| {};
    let outcome = runner.run(&spec, &cancel, &mut sink).expect("run");
    assert!(outcome.success(), "the run after reset must complete");
    assert!(!outcome.cancelled);
}

/// A timeout stops the group the same way.
#[test]
fn a_timeout_stops_the_child_process_group() {
    let harness = Harness::new();
    harness.install(
        "nix",
        "sleep 30 &\necho \"$!\" > grandchild.pid\necho \"$$\" > child.pid\nsleep 30",
    );
    let work = harness.workdir();
    let spec = CommandSpec::new(Program::Nix, &work)
        .with_timeout(Duration::from_millis(200))
        .with_kill_grace(Duration::from_millis(300));

    let outcome = run(&harness, &spec).expect("run");
    assert!(outcome.timed_out, "the run must report the timeout");
    assert!(!outcome.cancelled);

    let child = read_pid(&work.join("child.pid"));
    let grandchild = read_pid(&work.join("grandchild.pid"));
    assert!(
        wait_until_gone(child, Duration::from_secs(3)),
        "child survived"
    );
    assert!(
        wait_until_gone(grandchild, Duration::from_secs(3)),
        "grandchild survived"
    );
}

/// A normal exit is reaped: no zombie is left behind and the status is real.
#[test]
fn a_normal_exit_reaps_the_child() {
    let harness = Harness::new();
    harness.install("nix", "echo \"$$\" > child.pid\nexit 0");
    let work = harness.workdir();

    let outcome = run(&harness, &CommandSpec::new(Program::Nix, &work)).expect("run");
    assert!(outcome.success());
    let child = read_pid(&work.join("child.pid"));
    assert!(
        wait_until_gone(child, Duration::from_secs(2)),
        "the child was not reaped: {child} is still in the process table"
    );
}

/// A stopped run still reports what the program had printed, so the UI can show
/// where it was interrupted.
#[test]
fn a_cancelled_run_keeps_the_output_it_already_produced() {
    let harness = Harness::new();
    harness.install("nix", "echo 'starting build'\nsleep 30");
    let spec = CommandSpec::new(Program::Nix, harness.workdir())
        .with_timeout(Duration::from_secs(60))
        .with_kill_grace(Duration::from_millis(300));

    let env = harness.env();
    let runner = RealRunner::new(&env);
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        trigger.cancel();
    });
    let mut sink = |_: Stream, _: &str| {};
    let outcome = runner.run(&spec, &cancel, &mut sink).expect("run");
    assert!(outcome.cancelled);
    assert_eq!(outcome.stdout, "starting build");
}

fn read_pid(path: &Path) -> u32 {
    for _ in 0..100 {
        if let Ok(text) = fs::read_to_string(path) {
            if let Ok(pid) = text.trim().parse::<u32>() {
                return pid;
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("no pid in {}", path.display());
}

fn wait_until_gone(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !is_running(pid) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    !is_running(pid)
}

/// A chatty child must not be able to grow the runner's retained copy without
/// bound, while every line still reaches the sink.
#[test]
fn retained_output_is_bounded_while_streaming_continues() {
    let harness = Harness::new();
    // 2000 lines of ~100 bytes ≈ 200 KiB of output.
    harness.install(
        "nix",
        "i=0\nwhile [ $i -lt 2000 ]; do printf 'line-%04d-0123456789012345678901234567890123456789012345678901234567890123456789012345\\n' $i; i=$((i+1)); done",
    );
    let spec = CommandSpec::new(Program::Nix, harness.workdir()).with_max_capture(4096);

    let env = harness.env();
    let runner = RealRunner::new(&env);
    let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = seen.clone();
    let mut sink = move |_: Stream, _: &str| {
        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    };
    let outcome = runner
        .run(&spec, &CancelToken::new(), &mut sink)
        .expect("run");

    assert!(outcome.success());
    assert!(
        outcome.truncated,
        "the retained copy must report that it was cut"
    );
    assert!(
        outcome.stdout.len() <= 4096,
        "retained stdout grew to {} bytes",
        outcome.stdout.len()
    );
    assert_eq!(
        seen.load(std::sync::atomic::Ordering::Relaxed),
        2000,
        "streaming must be unaffected by the retention limit"
    );
}

/// Without a limit the whole output is kept, which is what the diagnostic path
/// relies on.
#[test]
fn unbounded_capture_is_the_default() {
    let harness = Harness::new();
    harness.install("nix", "printf 'one\\ntwo\\n'");
    let spec = CommandSpec::new(Program::Nix, harness.workdir());
    assert_eq!(spec.max_capture_bytes, None);
    let outcome = run(&harness, &spec).expect("run");
    assert_eq!(outcome.stdout, "one\ntwo");
    assert!(!outcome.truncated);
}
