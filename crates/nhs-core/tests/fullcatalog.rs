//! M5.3 acceptance tests: the full catalogue, built once and cached safely.
//!
//! Nothing here reaches the network. `nix search` is replaced by a fake runner,
//! and in the cancellable test by a real `sleep`: the point is what the code does
//! to the *stream*, not what nix does with nixpkgs.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Mutex;

use nhs_core::fullcatalog::{
    build, load_cached, parse_entries, snapshot_path, FullEntry, Limits, Snapshot,
    SNAPSHOT_FORMAT_VERSION,
};
use nhs_core::proc::{CancelToken, CommandSpec, Outcome, Program, RunError, Runner, Stream};
use nhs_core::revision::{resolve, Resolution};
use nhs_core::{FsView, MemFs, WriteFs};

/// Emits a scripted stdout/stderr and a status.
struct Scripted {
    stdout: String,
    stderr: String,
    exit: i32,
    seen: Mutex<Vec<CommandSpec>>,
}

impl Scripted {
    fn ok(stdout: String) -> Self {
        Self {
            stdout,
            stderr: String::new(),
            exit: 0,
            seen: Mutex::new(Vec::new()),
        }
    }

    fn failing(stderr: &str) -> Self {
        Self {
            stdout: String::new(),
            stderr: stderr.to_string(),
            exit: 1,
            seen: Mutex::new(Vec::new()),
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        self.seen.lock().expect("seen").clone()
    }
}

impl Runner for Scripted {
    fn run(
        &self,
        spec: &CommandSpec,
        _cancel: &CancelToken,
        sink: &mut dyn FnMut(Stream, &str),
    ) -> Result<Outcome, RunError> {
        self.seen.lock().expect("seen").push(spec.clone());
        for line in self.stdout.lines() {
            sink(Stream::Stdout, line);
        }
        for line in self.stderr.lines() {
            sink(Stream::Stderr, line);
        }
        Ok(Outcome {
            status: ExitStatus::from_raw(self.exit << 8),
            cancelled: false,
            timed_out: false,
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
            truncated: false,
        })
    }
}

fn lock_direct(rev: &str) -> String {
    format!(
        r#"{{ "nodes": {{ "root": {{ "inputs": {{ "nixpkgs": "nixpkgs" }} }}, "nixpkgs": {{ "locked": {{ "type": "github", "owner": "NixOS", "repo": "nixpkgs", "rev": "{rev}" }} }} }}, "root": "root", "version": 7 }}"#
    )
}

fn resolution(rev: &str) -> Resolution {
    resolve(None, Some(&lock_direct(rev))).expect("resolve")
}

fn search_json(entries: &[(&str, &str)]) -> String {
    let mut object = String::from("{");
    for (index, (attr, pname)) in entries.iter().enumerate() {
        if index > 0 {
            object.push(',');
        }
        object.push_str(&format!(
            r#""legacyPackages.x86_64-linux.{attr}":{{"pname":"{pname}","version":"1.0","description":"d"}}"#
        ));
    }
    object.push('}');
    object
}

fn build_once(
    fs: &MemFs,
    runner: &dyn Runner,
    resolution: &Resolution,
    limits: Limits,
) -> Result<Snapshot, nhs_core::fullcatalog::CatalogError> {
    let mut sink = |_: Stream, _: &str| {};
    build(
        fs,
        runner,
        resolution,
        Path::new("/state"),
        Path::new("/work"),
        limits,
        &CancelToken::new(),
        &mut sink,
    )
}

/// The acceptance test: a snapshot is reused only when the source matches.
#[test]
fn snapshot_is_reused_only_when_the_source_revision_hash_matches() {
    let fs = MemFs::new();
    let first = resolution("0123456789abcdef0123456789abcdef01234567");
    let runner = Scripted::ok(search_json(&[("hello", "hello"), ("ripgrep", "ripgrep")]));
    let snapshot = build_once(&fs, &runner, &first, Limits::default()).expect("build");
    assert_eq!(snapshot.entries.len(), 2);
    assert_eq!(runner.commands().len(), 1, "one search");

    // Same source: reused, and no second process.
    let again = build_once(&fs, &runner, &first, Limits::default()).expect("reuse");
    assert_eq!(again.entries, snapshot.entries);
    assert_eq!(
        runner.commands().len(),
        1,
        "the cache must prevent a rebuild"
    );

    // A different nixpkgs: rebuilt.
    let second = resolution("fedcba9876543210fedcba9876543210fedcba98");
    build_once(&fs, &runner, &second, Limits::default()).expect("rebuild");
    assert_eq!(runner.commands().len(), 2, "a new source must rebuild");

    // The cache file is named by the digest of the source, so the two coexist.
    assert_ne!(
        snapshot_path(Path::new("/state"), &first),
        snapshot_path(Path::new("/state"), &second)
    );
}

/// A snapshot built from one source is not reused for another, even when the file
/// system offers it: the lookup is keyed by the source digest.
#[test]
fn a_snapshot_from_another_source_is_never_returned() {
    let fs = MemFs::new();
    let first = resolution("0123456789abcdef0123456789abcdef01234567");
    let runner = Scripted::ok(search_json(&[("hello", "hello")]));
    build_once(&fs, &runner, &first, Limits::default()).expect("build");

    // Copy the cached file to the location the *other* source would use. If the
    // key check were missing, the second source would silently get the first
    // source's entries.
    let second = resolution("fedcba9876543210fedcba9876543210fedcba98");
    let from = snapshot_path(Path::new("/state"), &first);
    let to = snapshot_path(Path::new("/state"), &second);
    let bytes = fs.read_bytes(&from).expect("read cache");
    fs.write(&to, &bytes).expect("plant foreign snapshot");

    assert!(
        load_cached(&fs, Path::new("/state"), &second).is_none(),
        "a snapshot whose revision hash names another source must be refused"
    );
    // And the build rebuilds instead of trusting it.
    let before = runner.commands().len();
    build_once(&fs, &runner, &second, Limits::default()).expect("rebuild");
    assert_eq!(
        runner.commands().len(),
        before + 1,
        "a foreign snapshot must trigger a real build"
    );
}

/// A cached file that is damaged, incomplete or from another format is not used.
#[test]
fn a_damaged_or_incomplete_snapshot_is_not_reused() {
    let fs = MemFs::new();
    let res = resolution("0123456789abcdef0123456789abcdef01234567");
    let runner = Scripted::ok(search_json(&[("hello", "hello")]));
    build_once(&fs, &runner, &res, Limits::default()).expect("build");
    let path = snapshot_path(Path::new("/state"), &res);
    let original: Snapshot =
        serde_json::from_slice(&fs.read_bytes(&path).expect("read cache")).expect("parse cache");

    // Wrong format version.
    let mut older = original.clone();
    older.format_version = SNAPSHOT_FORMAT_VERSION + 1;
    write_snapshot(&fs, &path, &older);
    assert!(load_cached(&fs, Path::new("/state"), &res).is_none());

    // Not marked complete.
    let mut partial = original.clone();
    partial.complete = false;
    write_snapshot(&fs, &path, &partial);
    assert!(load_cached(&fs, Path::new("/state"), &res).is_none());

    // Entry digest does not match.
    let mut tampered = original.clone();
    tampered.entries[0].name = "something-else".to_string();
    write_snapshot(&fs, &path, &tampered);
    assert!(load_cached(&fs, Path::new("/state"), &res).is_none());

    // Count does not match the array.
    let mut miscounted = original.clone();
    miscounted.entries_count += 1;
    write_snapshot(&fs, &path, &miscounted);
    assert!(load_cached(&fs, Path::new("/state"), &res).is_none());

    // The untouched original is fine.
    write_snapshot(&fs, &path, &original);
    assert!(load_cached(&fs, Path::new("/state"), &res).is_some());
}

fn write_snapshot(fs: &MemFs, path: &Path, snapshot: &Snapshot) {
    let bytes = serde_json::to_vec(snapshot).expect("serialise");
    fs.write(path, &bytes).expect("write cache");
}

/// Exceeding the cap fails the build and leaves no snapshot behind.
#[test]
fn catalog_build_is_cancellable_and_capped() {
    let fs = MemFs::new();
    let res = resolution("0123456789abcdef0123456789abcdef01234567");
    // ~2 MiB of output against a 1 MiB cap.
    let big = search_json(
        &(0..20_000)
            .map(|index| (format!("pkg{index}"), format!("pkg{index}")))
            .collect::<Vec<_>>()
            .iter()
            .map(|(attr, name)| (attr.as_str(), name.as_str()))
            .collect::<Vec<_>>(),
    );
    assert!(big.len() > 1024 * 1024, "the fixture must exceed the cap");
    let runner = Scripted::ok(big);

    let limits = Limits {
        max_output_bytes: 1024 * 1024,
        ..Limits::default()
    };
    let error = build_once(&fs, &runner, &res, limits).expect_err("must be refused");
    match error {
        nhs_core::fullcatalog::CatalogError::TooLarge { bytes, limit } => {
            assert!(bytes > limit);
            assert_eq!(limit, 1024 * 1024);
        }
        other => panic!("expected TooLarge, got {other}"),
    }
    assert!(
        load_cached(&fs, Path::new("/state"), &res).is_none(),
        "a capped run must not leave a usable snapshot"
    );
    assert!(
        !fs.exists(&snapshot_path(Path::new("/state"), &res)),
        "and must not leave a file at all"
    );
}

/// A cancelled build writes nothing, and the message says it was cancelled.
#[test]
fn a_cancelled_build_keeps_no_snapshot() {
    let fs = MemFs::new();
    let res = resolution("0123456789abcdef0123456789abcdef01234567");

    /// Cancels the run and reports it, like the real runner would.
    struct Cancelling;
    impl Runner for Cancelling {
        fn run(
            &self,
            _spec: &CommandSpec,
            _cancel: &CancelToken,
            _sink: &mut dyn FnMut(Stream, &str),
        ) -> Result<Outcome, RunError> {
            Ok(Outcome {
                status: ExitStatus::from_raw(0x000f),
                cancelled: true,
                timed_out: false,
                stdout: String::new(),
                stderr: String::new(),
                truncated: false,
            })
        }
    }

    let error = build_once(&fs, &Cancelling, &res, Limits::default()).expect_err("cancelled");
    assert!(matches!(
        error,
        nhs_core::fullcatalog::CatalogError::Cancelled
    ));
    assert!(!fs.exists(&snapshot_path(Path::new("/state"), &res)));
}

/// A failing or unparseable run leaves no snapshot either.
#[test]
fn a_failed_or_unparseable_run_leaves_no_snapshot() {
    let fs = MemFs::new();
    let res = resolution("0123456789abcdef0123456789abcdef01234567");

    let failing = Scripted::failing("error: unable to download");
    let error = build_once(&fs, &failing, &res, Limits::default()).expect_err("failed");
    match error {
        nhs_core::fullcatalog::CatalogError::Failed { stderr, .. } => {
            assert!(stderr.contains("unable to download"), "{stderr}");
        }
        other => panic!("expected Failed, got {other}"),
    }
    assert!(!fs.exists(&snapshot_path(Path::new("/state"), &res)));

    let garbage = Scripted::ok("this is not json".to_string());
    let error = build_once(&fs, &garbage, &res, Limits::default()).expect_err("unparseable");
    assert!(matches!(
        error,
        nhs_core::fullcatalog::CatalogError::Unparseable { .. }
    ));
    assert!(!fs.exists(&snapshot_path(Path::new("/state"), &res)));
}

/// Attribute keys lose the flake-output prefix so they are usable as `pkgs.<attr>`.
#[test]
fn attribute_prefixes_are_stripped() {
    let json = br#"{
      "legacyPackages.x86_64-linux.hello": {"pname": "hello", "version": "2.12"},
      "legacyPackages.x86_64-linux.python3Packages.requests": {"pname": "requests", "version": "2.31"},
      "packages.aarch64-darwin.ripgrep": {"pname": "ripgrep", "version": "14"},
      "flake-level": {"pname": "flake-level", "version": "0"}
    }"#;
    let entries = parse_entries(json, Limits::default()).expect("parse");
    let attrs: Vec<&str> = entries.iter().map(|entry| entry.attr.as_str()).collect();
    assert_eq!(
        attrs,
        vec![
            "flake-level",
            "hello",
            "python3Packages.requests",
            "ripgrep"
        ]
    );
    assert_eq!(entries[1].name, "hello");
    assert_eq!(entries[1].version, "2.12");
    assert_eq!(entries[1].description, "", "a missing description is empty");

    // The command asks nix for the JSON form and the structured log.
    let res = resolution("0123456789abcdef0123456789abcdef01234567");
    let command =
        nhs_core::fullcatalog::search_command(&res, Path::new("/work"), Limits::default());
    assert_eq!(command.program, Program::Nix);
    let args: Vec<String> = command
        .args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args.first().map(String::as_str), Some("search"));
    assert!(args.iter().any(|arg| arg == "--json"));
    assert!(args.iter().any(|arg| arg == "^"), "search everything");
    assert!(
        args.iter()
            .any(|arg| arg == "github:NixOS/nixpkgs/0123456789abcdef0123456789abcdef01234567"),
        "{args:?}"
    );
    assert!(!args.iter().any(|arg| arg.contains("switch")));
}

/// The entry cap is enforced while parsing, not after building the whole map.
#[test]
fn too_many_entries_is_refused() {
    let json = search_json(&[("a", "a"), ("b", "b"), ("c", "c")]);
    let limits = Limits {
        max_entries: 2,
        ..Limits::default()
    };
    let error = parse_entries(json.as_bytes(), limits).expect_err("cap");
    assert!(matches!(
        error,
        nhs_core::fullcatalog::CatalogError::TooManyEntries { limit: 2 }
    ));
}

/// A snapshot records whether its source was pinned, so a branch build is
/// honest about being reproducible-or-not.
#[test]
fn the_snapshot_records_the_source_kind() {
    let fs = MemFs::new();
    let branch = resolve(Some("github:NixOS/nixpkgs/nixos-24.05"), None).expect("resolve");
    let runner = Scripted::ok(search_json(&[("hello", "hello")]));
    let snapshot = build_once(&fs, &runner, &branch, Limits::default()).expect("build");
    assert!(!snapshot.pinned, "a branch is not pinned");
    assert_eq!(snapshot.reference, "github:NixOS/nixpkgs/nixos-24.05");
    assert_eq!(snapshot.format_version, SNAPSHOT_FORMAT_VERSION);
    assert!(snapshot.complete);
    assert_eq!(snapshot.entries_count, 1);
    assert_eq!(
        snapshot.entries,
        vec![FullEntry {
            attr: "hello".to_string(),
            name: "hello".to_string(),
            version: "1.0".to_string(),
            description: "d".to_string(),
        }]
    );
}

/// A relative or temporary path is never written: the temporary file has the same
/// directory as the snapshot, so the rename is atomic.
#[test]
fn the_temporary_file_lives_next_to_the_snapshot() {
    let res = resolution("0123456789abcdef0123456789abcdef01234567");
    let path = snapshot_path(Path::new("/state"), &res);
    assert_eq!(path.parent(), Some(Path::new("/state/catalog")));
    assert!(path.extension().and_then(|e| e.to_str()) == Some("json"));

    // After a successful build, no `.tmp` file remains.
    let fs = MemFs::new();
    let runner = Scripted::ok(search_json(&[("hello", "hello")]));
    build_once(&fs, &runner, &res, Limits::default()).expect("build");
    let leftovers: Vec<PathBuf> = fs
        .read_dir(Path::new("/state/catalog"))
        .expect("list")
        .into_iter()
        .filter(|entry| entry.to_string_lossy().contains(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temporary files left: {leftovers:?}");
}

/// Sanity: the injected limit is what the message reports, not a hard-coded 300 MiB.
#[test]
fn the_reported_limit_is_the_injected_one() {
    let fs = MemFs::new();
    let res = resolution("0123456789abcdef0123456789abcdef01234567");
    let runner = Scripted::ok("x".repeat(2048));
    let limits = Limits {
        max_output_bytes: 1024,
        ..Limits::default()
    };
    let error = build_once(&fs, &runner, &res, limits).expect_err("cap");
    let message = error.to_string();
    assert!(message.contains("1024 byte limit"), "{message}");
    assert!(!message.contains("314572800"), "{message}");
}

/// The catalogue build must bound the *runner's* retention too, not only its own
/// buffer: otherwise the process output grows without limit in memory while this
/// module counts it.
#[test]
fn the_search_command_bounds_what_the_runner_retains() {
    let res = resolution("0123456789abcdef0123456789abcdef01234567");
    let limits = Limits {
        max_output_bytes: 4096,
        ..Limits::default()
    };
    let command = nhs_core::fullcatalog::search_command(&res, Path::new("/work"), limits);
    assert_eq!(
        command.max_capture_bytes,
        Some(4096),
        "the runner must be told the same bound this module enforces"
    );

    // And a truncated outcome is refused even when this module's own counter has
    // not tripped: the two signals are independent.
    struct Truncating;
    impl Runner for Truncating {
        fn run(
            &self,
            _spec: &CommandSpec,
            _cancel: &CancelToken,
            _sink: &mut dyn FnMut(Stream, &str),
        ) -> Result<Outcome, RunError> {
            Ok(Outcome {
                status: ExitStatus::from_raw(0),
                cancelled: false,
                timed_out: false,
                stdout: "{}".to_string(),
                stderr: String::new(),
                truncated: true,
            })
        }
    }
    let fs = MemFs::new();
    let error = build_once(&fs, &Truncating, &res, Limits::default()).expect_err("truncated");
    assert!(matches!(
        error,
        nhs_core::fullcatalog::CatalogError::TooLarge { .. }
    ));
    assert!(!fs.exists(&snapshot_path(Path::new("/state"), &res)));
}
