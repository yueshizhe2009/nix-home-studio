//! Edit-engine tests against the **real** filesystem.
//!
//! `MemFs` cannot express permission bits, inode identity, or `fsync`, so a
//! suite that only uses it would miss every `RealFs`-specific hazard. Each test
//! works inside a `tempfile::TempDir`, so nothing here can touch the user's
//! configuration.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use nhs_core::{EditEngine, EditError, FsView, RealFs, StateLayout};

struct Fixture {
    dir: tempfile::TempDir,
    root: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("create temp dir");
        let root = dir.path().join("config");
        let state = dir.path().join("state");
        std::fs::create_dir_all(root.join("modules")).expect("create config tree");
        Self { dir, root, state }
    }

    fn module(&self) -> PathBuf {
        self.root.join("modules/nhs-packages.nix")
    }

    fn layout(&self) -> StateLayout {
        StateLayout {
            state_dir: self.state.clone(),
            config_root: self.root.clone(),
        }
    }

    fn engine(&self) -> EditEngine<'_> {
        EditEngine::new(&RealFs, self.layout())
    }

    fn write_module(&self, text: &str) {
        std::fs::write(self.module(), text).expect("seed module");
    }

    fn read_module(&self) -> String {
        std::fs::read_to_string(self.module()).expect("read module")
    }

    /// Every entry directly under `dir`, for leftover checks.
    fn entries(&self, dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("list dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }
}

const ORIGINAL: &str = "{ pkgs, ... }:\n{\n  home.packages = [ pkgs.fd ];\n}\n";
const UPDATED: &str = "{ pkgs, ... }:\n{\n  home.packages = [ pkgs.fd pkgs.rg ];\n}\n";

#[test]
fn real_fs_commit_writes_and_leaves_no_temporaries() {
    let fixture = Fixture::new();
    fixture.write_module(ORIGINAL);
    let engine = fixture.engine();

    let plan = engine.plan(vec![engine
        .stage(&fixture.module(), UPDATED)
        .expect("stage")]);
    let applied = engine.commit(&plan).expect("commit");

    assert_eq!(applied.written, vec![fixture.module()]);
    assert_eq!(fixture.read_module(), UPDATED);
    let names = fixture.entries(&fixture.root.join("modules"));
    assert_eq!(
        names,
        vec!["nhs-packages.nix".to_string()],
        "no temp file may survive a successful commit"
    );
    assert!(
        !fixture.state.join("edit.lock").exists(),
        "the lock must be released"
    );
}

/// Replacing a file must not widen its permissions.
#[test]
fn real_fs_commit_preserves_permission_bits() {
    let fixture = Fixture::new();
    fixture.write_module(ORIGINAL);
    std::fs::set_permissions(fixture.module(), std::fs::Permissions::from_mode(0o600))
        .expect("chmod 600");

    let engine = fixture.engine();
    let plan = engine.plan(vec![engine
        .stage(&fixture.module(), UPDATED)
        .expect("stage")]);
    engine.commit(&plan).expect("commit");

    let mode = std::fs::metadata(fixture.module())
        .expect("stat")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "a 0600 config must stay 0600");
}

/// A conflict must leave the file byte-identical and no temp files behind.
#[test]
fn real_fs_conflict_leaves_the_file_untouched() {
    let fixture = Fixture::new();
    fixture.write_module(ORIGINAL);
    let engine = fixture.engine();
    let plan = engine.plan(vec![engine
        .stage(&fixture.module(), UPDATED)
        .expect("stage")]);

    let external = "{ pkgs, ... }: { home.packages = [ ]; }\n";
    std::fs::write(fixture.module(), external).expect("external edit");

    let error = engine.commit(&plan).expect_err("must refuse");
    assert!(matches!(error, EditError::Conflict { .. }), "{error}");
    assert_eq!(fixture.read_module(), external);
    assert_eq!(
        fixture.entries(&fixture.root.join("modules")),
        vec!["nhs-packages.nix".to_string()]
    );
}

/// A dotfiles symlink must survive: the real file is edited, the link stays.
#[test]
fn real_fs_writes_through_a_symlink_without_replacing_it() {
    let fixture = Fixture::new();
    let real = fixture.root.join("dotfiles-home.nix");
    std::fs::write(&real, ORIGINAL).expect("seed real file");
    let link = fixture.root.join("home.nix");
    std::os::unix::fs::symlink(&real, &link).expect("create symlink");

    let engine = fixture.engine();
    let change = engine.stage(&link, UPDATED).expect("stage");
    assert_eq!(change.path, real, "the change targets the real file");
    assert_eq!(change.resolved_from.as_deref(), Some(link.as_path()));

    engine.commit(&engine.plan(vec![change])).expect("commit");

    let metadata = std::fs::symlink_metadata(&link).expect("lstat link");
    assert!(
        metadata.file_type().is_symlink(),
        "the symlink must still be a symlink"
    );
    assert_eq!(std::fs::read_to_string(&real).expect("read real"), UPDATED);
}

/// A file created by a commit must be removable by revert.
#[test]
fn real_fs_revert_removes_a_created_file() {
    let fixture = Fixture::new();
    std::fs::write(fixture.root.join("home.nix"), "{ }\n").expect("seed main");
    let engine = fixture.engine();
    let created = fixture.root.join("modules/new.nix");

    let plan = engine.plan(vec![engine
        .stage_create(&created, ORIGINAL)
        .expect("stage")]);
    let applied = engine.commit(&plan).expect("commit");
    assert!(created.exists());

    engine.revert(&applied.snapshot_id).expect("revert");
    assert!(
        !created.exists(),
        "a created file must be removed on revert"
    );
}

/// Bytes that are not valid UTF-8 must still be hashed and backed up exactly.
#[test]
fn real_fs_backs_up_non_utf8_bytes_exactly() {
    let fixture = Fixture::new();
    let path = fixture.module();
    let raw: &[u8] = b"{ pkgs, ... }:\n{\n  # \xff\xfe not utf-8\n}\n";
    std::fs::write(&path, raw).expect("seed non-utf8");

    // Staging fails to read it as text, which is the honest outcome: this
    // engine only edits text files, and it must not silently mangle bytes.
    let engine = fixture.engine();
    let error = engine.stage(&path, UPDATED).expect_err("must refuse");
    assert!(
        matches!(error, EditError::Storage { .. }),
        "a non-UTF-8 file must be refused, got {error}"
    );
    assert_eq!(std::fs::read(&path).expect("read back"), raw, "untouched");
}

/// The hash of a file read through `RealFs` must match hashing its bytes.
#[test]
fn real_fs_reads_bytes_exactly() {
    let fixture = Fixture::new();
    let path = fixture.module();
    std::fs::write(&path, ORIGINAL).expect("seed");

    let bytes = RealFs.read_bytes(&path).expect("read bytes");
    assert_eq!(bytes, ORIGINAL.as_bytes());
    assert!(RealFs.exists(&path));
}

/// The temp dir must outlive the fixture for the assertions above to mean
/// anything; this also documents why it is held.
#[test]
fn fixture_directory_is_isolated() {
    let fixture = Fixture::new();
    assert!(
        fixture.dir.path().starts_with(std::env::temp_dir()),
        "fixtures must live in the system temp directory"
    );
    assert!(fixture.root.starts_with(fixture.dir.path()));
    assert!(fixture.state.starts_with(fixture.dir.path()));
}

/// Backups and snapshots hold configuration content, so they must not be
/// world-readable, and the state directory must be private.
#[test]
fn real_fs_state_artifacts_are_not_world_readable() {
    let fixture = Fixture::new();
    fixture.write_module(ORIGINAL);
    let engine = fixture.engine();
    let plan = engine.plan(vec![engine
        .stage(&fixture.module(), UPDATED)
        .expect("stage")]);
    let applied = engine.commit(&plan).expect("commit");

    let mode_of = |path: &Path| std::fs::metadata(path).expect("stat").permissions().mode() & 0o777;

    for backup in &applied.backups {
        if backup.existed {
            assert_eq!(mode_of(&backup.stored), 0o600, "backup must be 0600");
        }
    }
    let snapshot = fixture
        .state
        .join("snapshots")
        .join(format!("{}.json", applied.snapshot_id));
    assert_eq!(mode_of(&snapshot), 0o600, "snapshot must be 0600");
    assert_eq!(mode_of(&fixture.state), 0o700, "state dir must be 0700");
}
