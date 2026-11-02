//! M2 acceptance tests: the safe edit engine.
//!
//! Every test here is written against the normative rules in
//! `docs/implementation-plan.md` (M2) and `docs/architecture.md` §7.1. No test
//! touches the real user configuration, and none runs Nix.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

mod support;

use std::path::{Path, PathBuf};

use nhs_core::edit::backup::{BackupEntry, BackupStore, RestoreGuard};
use nhs_core::edit::snapshot::{Outcome, SnapshotStore};
use nhs_core::edit::ChangeKind;
use nhs_core::{EditEngine, EditError, FsView, MemFs, Sha256, StateLayout, WriteFs};
use support::flaky::{Fault, Flaky, FlakyFs};

const MODULE: &str = "/home/tester/.config/home-manager/modules/nhs-packages.nix";
const MAIN: &str = "/home/tester/.config/home-manager/home.nix";

fn engine(fs: &dyn WriteFs) -> EditEngine<'_> {
    EditEngine::new(fs, support::edit_layout())
}

fn read(fs: &MemFs, path: &str) -> String {
    fs.read_to_string(Path::new(path))
        .expect("read fixture file")
}

/// A plan must be reviewable without side effects.
#[test]
fn plan_produces_diff_without_touching_disk() {
    let fs = support::edit_fixture();
    let before = read(&fs, MODULE);
    let before_main = read(&fs, MAIN);
    let engine = engine(&fs);

    let change = engine
        .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
        .expect("stage");
    let plan = engine.plan(vec![change]);

    assert_eq!(plan.changes.len(), 1);
    let change = &plan.changes[0];
    assert_eq!(change.kind, ChangeKind::Replace);
    assert!(
        change.unified_diff.contains("+  x = 1;"),
        "diff must show the addition: {}",
        change.unified_diff
    );
    assert_eq!(change.added_lines, 1);
    assert_eq!(change.removed_lines, 0);

    // Nothing on disk changed, and no state was created either.
    assert_eq!(read(&fs, MODULE), before, "staging must not write");
    assert_eq!(read(&fs, MAIN), before_main);
    assert!(!fs.exists(Path::new(support::EDIT_STATE_DIR)));
    // No temp files may appear next to the staged file. (A hard-coded temp name
    // would be a vacuous assertion, because the name carries a counter.)
    let entries = fs
        .read_dir(Path::new("/home/tester/.config/home-manager/modules"))
        .expect("list modules dir");
    let temporaries: Vec<String> = entries
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("nhs-tmp"))
        })
        .map(|path| path.display().to_string())
        .collect();
    assert!(
        temporaries.is_empty(),
        "staging must not create temp files: {temporaries:?}"
    );
}

/// A file that changed after review must abort the whole plan.
#[test]
fn commit_refuses_when_the_file_changed_after_the_plan() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);
    let change = engine
        .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
        .expect("stage");
    let plan = engine.plan(vec![change]);

    // Somebody edits the file between review and apply.
    let external = support::valid_module("  y = 2;\n");
    fs.write(Path::new(MODULE), external.as_bytes())
        .expect("simulate external edit");

    let error = engine.commit(&plan).expect_err("must refuse");
    match error {
        EditError::Conflict { path, .. } => assert_eq!(path, Path::new(MODULE)),
        other => panic!("expected Conflict, got {other}"),
    }
    assert_eq!(
        read(&fs, MODULE),
        external,
        "the external edit must survive untouched"
    );
}

/// A conflict must not partially apply the rest of a multi-file plan.
#[test]
fn a_conflict_aborts_every_file_in_the_plan() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);
    let module_new = support::valid_module("  x = 1;\n");
    let main_new = "{\n  imports = [ ./modules/nhs-packages.nix ];\n}\n";
    let plan = engine.plan(vec![
        engine.stage(Path::new(MODULE), &module_new).expect("stage"),
        engine.stage(Path::new(MAIN), main_new).expect("stage"),
    ]);

    let external = support::valid_module("  z = 3;\n");
    fs.write(Path::new(MAIN), external.as_bytes())
        .expect("edit");

    let error = engine.commit(&plan).expect_err("must refuse");
    assert!(matches!(error, EditError::Conflict { .. }), "{error}");
    assert_eq!(
        read(&fs, MODULE),
        support::valid_module(""),
        "the untouched file must not be written"
    );
    assert_eq!(read(&fs, MAIN), external);
}

/// A successful commit writes exactly the planned bytes and leaves no temp file.
#[test]
fn commit_writes_atomically_and_leaves_no_temp_files() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);
    let new_text = support::valid_module("  x = 1;\n");
    let plan = engine.plan(vec![engine
        .stage(Path::new(MODULE), &new_text)
        .expect("stage")]);

    let applied = engine.commit(&plan).expect("commit");
    assert_eq!(applied.written, vec![PathBuf::from(MODULE)]);
    assert!(!applied.snapshot_id.is_empty());
    assert_eq!(read(&fs, MODULE), new_text);

    let entries = fs
        .read_dir(Path::new("/home/tester/.config/home-manager/modules"))
        .expect("list modules dir");
    let leftovers: Vec<String> = entries
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("nhs-tmp"))
        })
        .map(|path| path.display().to_string())
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );
    assert!(!fs.exists(Path::new(
        "/home/tester/.local/state/nix-home-studio/edit.lock"
    )));
}

/// A failure on the second file must restore the first.
#[test]
fn a_failure_on_the_second_file_restores_the_first() {
    let inner = support::edit_fixture();
    let original_module = read(&inner, MODULE);
    let original_main = read(&inner, MAIN);
    // Fail by name, not by call index: the first file must already be in place
    // when the second one fails, and an index would break every time the number
    // of internal writes changes.
    let fs = FlakyFs::new(inner, Fault::RenameInto("home.nix"));
    let engine = engine(&fs);

    let plan = engine.plan(vec![
        engine
            .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
            .expect("stage"),
        engine
            .stage(
                Path::new(MAIN),
                "{\n  imports = [ ./modules/nhs-packages.nix ];\n}\n",
            )
            .expect("stage"),
    ]);

    let error = engine.commit(&plan).expect_err("must fail");
    match error {
        EditError::Write { restored, .. } => assert!(restored, "must report restoration"),
        other => panic!("expected Write, got {other}"),
    }
    assert_eq!(
        read(fs.inner(), MODULE),
        original_module,
        "the first file must be restored from its backup"
    );
    assert_eq!(read(fs.inner(), MAIN), original_main);
}

/// A failed commit that created a file must remove it again.
#[test]
fn a_failed_commit_removes_files_it_created() {
    let inner = MemFs::new().with_file(format!("{}/home.nix", support::EDIT_CONFIG_ROOT), "{\n}\n");
    let created = format!("{}/modules/new.nix", support::EDIT_CONFIG_ROOT);
    // The created file lands first; failing the second rename forces the
    // rollback to delete it again.
    let fs = FlakyFs::new(inner, Fault::RenameInto("home.nix"));
    let engine = engine(&fs);

    let plan = engine.plan(vec![
        engine
            .stage_create(Path::new(&created), &support::valid_module(""))
            .expect("stage create"),
        engine
            .stage(
                Path::new(MAIN),
                "{\n  imports = [ ./modules/new.nix ];\n}\n",
            )
            .expect("stage"),
    ]);

    engine.commit(&plan).expect_err("must fail");
    assert!(
        !fs.inner().exists(Path::new(&created)),
        "a created file must be removed when the commit fails"
    );
    assert_eq!(read(fs.inner(), MAIN), "{\n}\n");
}

/// Backups accumulate; a second apply must not destroy the first one's copy.
#[test]
fn backups_survive_a_second_apply() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);

    let first_text = support::valid_module("  x = 1;\n");
    let first = engine.plan(vec![engine
        .stage(Path::new(MODULE), &first_text)
        .expect("stage")]);
    let first_applied = engine.commit(&first).expect("first commit");

    let second_text = support::valid_module("  x = 1;\n  y = 2;\n");
    let second = engine.plan(vec![engine
        .stage(Path::new(MODULE), &second_text)
        .expect("stage")]);
    let second_applied = engine.commit(&second).expect("second commit");

    // Both backups still exist and hold their own content.
    let store = nhs_core::edit::backup::BackupStore::new(&fs, Path::new(support::EDIT_STATE_DIR));
    for entry in first_applied
        .backups
        .iter()
        .chain(second_applied.backups.iter())
    {
        let bytes = store.load(entry).expect("backup readable and intact");
        assert!(!bytes.is_empty(), "a Replace must always have content");
    }
    // The second backup holds what the first commit wrote.
    let second_backup = &second_applied.backups[0];
    assert_eq!(
        store.load(second_backup).expect("load"),
        first_text.as_bytes()
    );
}

/// Revert restores exact bytes, and refuses to use a corrupted backup.
#[test]
fn revert_restores_exact_bytes_and_detects_a_corrupted_backup() {
    let fs = support::edit_fixture();
    let original = read(&fs, MODULE);
    let engine = engine(&fs);

    let plan = engine.plan(vec![engine
        .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
        .expect("stage")]);
    let applied = engine.commit(&plan).expect("commit");
    assert_ne!(read(&fs, MODULE), original);

    let restored = engine.revert(&applied.snapshot_id).expect("revert");
    assert_eq!(restored, vec![PathBuf::from(MODULE)]);
    assert_eq!(read(&fs, MODULE), original, "bytes must be exact");
}

/// A corrupted backup must be refused, not restored.
#[test]
fn revert_refuses_a_corrupted_backup() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);
    let plan = engine.plan(vec![engine
        .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
        .expect("stage")]);
    let applied = engine.commit(&plan).expect("commit");

    let stored = applied.backups[0].stored.clone();
    fs.write(&stored, b"tampered").expect("corrupt backup");

    let error = engine
        .revert(&applied.snapshot_id)
        .expect_err("must refuse");
    assert!(
        matches!(error, EditError::Storage { .. }),
        "a corrupted backup must be reported, got {error}"
    );
    // And the file must not have been overwritten with the bad bytes.
    assert_eq!(read(&fs, MODULE), support::valid_module("  x = 1;\n"));
}

/// Reverting must not discard edits made after the commit.
#[test]
fn revert_refuses_when_the_file_was_edited_afterwards() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);
    let plan = engine.plan(vec![engine
        .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
        .expect("stage")]);
    let applied = engine.commit(&plan).expect("commit");

    let later = support::valid_module("  hand_edit = true;\n");
    fs.write(Path::new(MODULE), later.as_bytes()).expect("edit");

    let error = engine
        .revert(&applied.snapshot_id)
        .expect_err("must refuse");
    assert!(
        matches!(error, EditError::StaleRevert { .. }),
        "expected StaleRevert, got {error}"
    );
    assert_eq!(read(&fs, MODULE), later, "the later edit must survive");
}

/// A snapshot id may not escape the snapshot directory.
#[test]
fn revert_rejects_an_id_that_escapes_the_state_directory() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);
    let error = engine.revert("../../../tmp/evil").expect_err("must refuse");
    assert!(
        matches!(error, EditError::InvalidSnapshot { .. }),
        "expected InvalidSnapshot, got {error}"
    );
}

/// A plan that names the same file twice is refused: the second diff the user
/// reviewed is not what would land.
#[test]
fn a_plan_with_a_duplicate_target_is_refused() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);
    let first = engine
        .stage(Path::new(MODULE), &support::valid_module("  a = 1;\n"))
        .expect("stage");
    let second = engine
        .stage(Path::new(MODULE), &support::valid_module("  b = 2;\n"))
        .expect("stage");
    let plan = engine.plan(vec![first, second]);

    let error = engine.commit(&plan).expect_err("must refuse");
    assert!(
        matches!(error, EditError::DuplicatePath { .. }),
        "expected DuplicatePath, got {error}"
    );
}

/// A plan edited after review must be rejected, not applied.
#[test]
fn a_plan_mutated_after_review_is_refused() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);
    let mut change = engine
        .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
        .expect("stage");
    // Someone swaps the content but keeps the reviewed diff.
    change.new_text = support::valid_module("  evil = true;\n");
    let plan = engine.plan(vec![change]);

    let error = engine.commit(&plan).expect_err("must refuse");
    assert!(
        matches!(error, EditError::InconsistentPlan { .. }),
        "expected InconsistentPlan, got {error}"
    );
    assert_eq!(read(&fs, MODULE), support::valid_module(""));
}

/// Two commits cannot overlap.
#[test]
fn a_second_instance_cannot_commit_while_a_lock_is_held() {
    let fs = support::edit_fixture();
    let lock = nhs_core::edit::lock::EditLock::acquire(&fs, Path::new(support::EDIT_STATE_DIR))
        .expect("take the lock");

    let engine = engine(&fs);
    let plan = engine.plan(vec![engine
        .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
        .expect("stage")]);
    let error = engine.commit(&plan).expect_err("must refuse");
    match error {
        EditError::Locked(locked) => {
            assert_eq!(locked.path, lock.path());
            assert!(
                locked.holder.as_deref().is_some_and(|h| h.contains("pid=")),
                "the message must identify the holder: {locked}"
            );
        }
        other => panic!("expected Locked, got {other}"),
    }
    assert_eq!(read(&fs, MODULE), support::valid_module(""));

    // Releasing the lock makes the same plan succeed.
    drop(lock);
    engine.commit(&plan).expect("commit after release");
}

/// Invalid Nix is refused before it is written.
#[test]
fn commit_reparses_every_written_file_and_refuses_invalid_nix() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);
    let broken = "{ pkgs, ... }: { home.packages = [ }".to_string();
    let plan = engine.plan(vec![engine
        .stage(Path::new(MODULE), &broken)
        .expect("stage")]);

    let error = engine.commit(&plan).expect_err("must refuse");
    match error {
        EditError::InvalidNix { path, detail } => {
            assert_eq!(path, Path::new(MODULE));
            assert!(!detail.is_empty(), "the reason must be reported");
        }
        other => panic!("expected InvalidNix, got {other}"),
    }
    assert_eq!(
        read(&fs, MODULE),
        support::valid_module(""),
        "invalid content must never reach disk"
    );
    // Nothing was written, so no backup or snapshot should exist either.
    assert!(!fs.exists(Path::new(
        "/home/tester/.local/state/nix-home-studio/snapshots"
    )));
}

/// The snapshot records the attempt with a format version.
#[test]
fn snapshot_records_the_attempt_with_a_format_version() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);
    let plan = engine.plan(vec![engine
        .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
        .expect("stage")]);
    let applied = engine.commit(&plan).expect("commit");

    let store = SnapshotStore::new(&fs, Path::new(support::EDIT_STATE_DIR));
    let snapshot = store.load(&applied.snapshot_id).expect("load snapshot");
    assert_eq!(
        snapshot.format_version,
        nhs_core::edit::snapshot::SNAPSHOT_FORMAT_VERSION
    );
    assert_eq!(snapshot.outcome, Outcome::Applied);
    assert_eq!(snapshot.changes.len(), 1);
    assert_eq!(snapshot.changes[0].kind, "replace");
    assert_eq!(snapshot.backups.len(), 1);
    assert_eq!(
        snapshot.changes[0].old_sha256.as_deref(),
        Some(
            Sha256::of_bytes(support::valid_module("").as_bytes())
                .to_hex()
                .as_str()
        )
    );
}

/// A state directory inside the configuration tree is refused outright.
#[test]
fn a_state_directory_inside_the_config_tree_is_refused() {
    let fs = support::edit_fixture();
    let layout = StateLayout {
        state_dir: PathBuf::from(format!("{}/.nhs-state", support::EDIT_CONFIG_ROOT)),
        config_root: PathBuf::from(support::EDIT_CONFIG_ROOT),
    };
    let engine = EditEngine::new(&fs, layout);
    let plan = engine.plan(vec![engine
        .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
        .expect("stage")]);

    let error = engine.commit(&plan).expect_err("must refuse");
    assert!(
        matches!(error, EditError::StateInsideConfig { .. }),
        "expected StateInsideConfig, got {error}"
    );
}

/// An unchanged plan is a no-op, not a write.
#[test]
fn a_plan_that_changes_nothing_writes_nothing() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);
    let same = support::valid_module("");
    let plan = engine.plan(vec![engine.stage(Path::new(MODULE), &same).expect("stage")]);

    assert!(plan.is_empty());
    let applied = engine.commit(&plan).expect("commit");
    assert!(applied.written.is_empty());
    assert!(applied.backups.is_empty());
    assert!(!fs.exists(Path::new(
        "/home/tester/.local/state/nix-home-studio/edit.lock"
    )));
}

/// Creating a new module is reported as `Create` and has no expected hash.
#[test]
fn staging_a_missing_file_plans_a_create() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);
    let path = format!("{}/modules/brand-new.nix", support::EDIT_CONFIG_ROOT);
    let change = engine
        .stage(Path::new(&path), &support::valid_module(""))
        .expect("stage");
    assert_eq!(change.kind, ChangeKind::Create);
    assert_eq!(change.expected_sha256, None);
    assert!(change.added_lines > 0);
}

/// A dotfiles-managed `home.nix` is usually a symlink. Editing it must write
/// through the link, never replace the link with a regular file.
#[test]
fn editing_a_symlinked_file_writes_through_the_link() {
    let target = format!("{}/dotfiles/home.nix", support::EDIT_CONFIG_ROOT);
    let link = format!("{}/home.nix", support::EDIT_CONFIG_ROOT);
    let fs = MemFs::new()
        .with_file(&target, "{\n  imports = [ ];\n}\n")
        .with_symlink(&link, &target);
    let engine = engine(&fs);

    let new_text = "{\n  imports = [ ./modules/nhs-packages.nix ];\n}\n";
    let change = engine.stage(Path::new(&link), new_text).expect("stage");

    assert_eq!(
        change.path,
        Path::new(&target),
        "the real file must be the target of the change"
    );
    assert_eq!(
        change.resolved_from.as_deref(),
        Some(Path::new(&link)),
        "the original path must be reported"
    );

    let plan = engine.plan(vec![change]);
    engine.commit(&plan).expect("commit");

    assert!(
        fs.is_symlink(Path::new(&link)),
        "the symlink must still be a symlink"
    );
    assert_eq!(read(&fs, &target), new_text);
    assert_eq!(read(&fs, &link), new_text, "reading through the link works");
}

/// A symlink that does not resolve has no real file behind it; refuse.
#[test]
fn a_dangling_symlink_is_refused() {
    let link = format!("{}/home.nix", support::EDIT_CONFIG_ROOT);
    let fs = MemFs::new()
        .with_file(format!("{}/other.nix", support::EDIT_CONFIG_ROOT), "{ }\n")
        .with_symlink(&link, "/nix/store/does-not-exist");
    let engine = engine(&fs);

    let error = engine
        .stage(Path::new(&link), "{ }\n")
        .expect_err("must refuse");
    assert!(
        matches!(error, EditError::DanglingSymlink { .. }),
        "expected DanglingSymlink, got {error}"
    );
}

/// Reverting is a write too: it must not run concurrently with a commit.
#[test]
fn revert_takes_the_edit_lock() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);
    let plan = engine.plan(vec![engine
        .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
        .expect("stage")]);
    let applied = engine.commit(&plan).expect("commit");

    let lock = nhs_core::edit::lock::EditLock::acquire(&fs, Path::new(support::EDIT_STATE_DIR))
        .expect("take the lock");
    let error = engine
        .revert(&applied.snapshot_id)
        .expect_err("must refuse while locked");
    assert!(matches!(error, EditError::Locked(_)), "got {error}");

    drop(lock);
    engine
        .revert(&applied.snapshot_id)
        .expect("revert after release");
}

/// If the snapshot update fails after the bytes landed, the commit must report
/// success with a warning — never an error that implies nothing was written.
#[test]
fn a_failed_snapshot_update_reports_success_with_a_warning() {
    let inner = support::edit_fixture();
    // The pending record is the first write into `snapshots`, the `Applied`
    // update is the second. Fail the second one.
    let fs = FlakyFs::new(inner, Fault::WriteNewInto("snapshots", 2));
    let engine = engine(&fs);

    let new_text = support::valid_module("  x = 1;\n");
    let plan = engine.plan(vec![engine
        .stage(Path::new(MODULE), &new_text)
        .expect("stage")]);

    let applied = engine.commit(&plan).expect("the bytes did land");
    assert_eq!(
        read(fs.inner(), MODULE),
        new_text,
        "the file must be written"
    );
    let warning = applied.warning.expect("a warning must be reported");
    assert!(
        warning.contains("snapshot"),
        "the warning must name the snapshot: {warning}"
    );
    // The record is still readable and honest about being incomplete.
    let store = SnapshotStore::new(fs.inner(), Path::new(support::EDIT_STATE_DIR));
    let snapshot = store.load(&applied.snapshot_id).expect("pending record");
    assert_eq!(snapshot.outcome, Outcome::Pending);
}

/// A failure on the middle file must not touch the files after it.
#[test]
fn a_failure_does_not_touch_files_the_plan_never_reached() {
    use std::os::unix::fs::MetadataExt;

    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join("config");
    std::fs::create_dir_all(&root).expect("config tree");
    let paths: Vec<PathBuf> = ["a.nix", "b.nix", "c.nix"]
        .iter()
        .map(|name| root.join(name))
        .collect();
    for path in &paths {
        std::fs::write(path, support::valid_module("")).expect("seed");
    }
    let inodes_before: Vec<u64> = paths
        .iter()
        .map(|path| std::fs::metadata(path).expect("stat").ino())
        .collect();

    let state = dir.path().join("state");
    let fs = Flaky::new(nhs_core::RealFs, Fault::RenameInto("b.nix"));
    let engine = nhs_core::EditEngine::new(
        &fs,
        StateLayout {
            state_dir: state,
            config_root: root.clone(),
        },
    );

    let plan = engine.plan(
        paths
            .iter()
            .map(|path| {
                engine
                    .stage(path, &support::valid_module("  x = 1;\n"))
                    .expect("stage")
            })
            .collect(),
    );
    engine.commit(&plan).expect_err("must fail on b.nix");

    // a.nix was written then restored: its content is back but its inode may
    // have changed. c.nix was never reached, so its inode must be identical.
    let inode_after = std::fs::metadata(&paths[2]).expect("stat").ino();
    assert_eq!(
        inode_after, inodes_before[2],
        "c.nix was never written, so it must not have been replaced"
    );
    assert_eq!(
        std::fs::read_to_string(&paths[2]).expect("read"),
        support::valid_module("")
    );
    assert_eq!(
        std::fs::read_to_string(&paths[0]).expect("read"),
        support::valid_module(""),
        "a.nix must be restored to its original content"
    );
}

/// The dotfiles layout: `home.nix` inside the config tree is a symlink to a file
/// in the user's repository, outside the tree. That must work, and revert must
/// be able to undo it.
#[test]
fn a_symlink_may_point_outside_the_tree_and_still_be_revertible() {
    let repo = "/home/tester/dotfiles/home.nix";
    let link = format!("{}/home.nix", support::EDIT_CONFIG_ROOT);
    let fs = MemFs::new()
        .with_file(repo, "{\n  imports = [ ];\n}\n")
        .with_symlink(&link, repo);
    let engine = engine(&fs);

    let new_text = "{\n  imports = [ ./modules/nhs-packages.nix ];\n}\n";
    let change = engine.stage(Path::new(&link), new_text).expect("stage");
    assert_eq!(change.path, Path::new(repo));
    let applied = engine.commit(&engine.plan(vec![change])).expect("commit");
    assert_eq!(read(&fs, repo), new_text);

    engine
        .revert(&applied.snapshot_id)
        .expect("revert through the recorded link");
    assert_eq!(read(&fs, repo), "{\n  imports = [ ];\n}\n");
}

/// A request outside the configuration tree is refused outright.
#[test]
fn a_target_outside_the_configuration_tree_is_refused() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);
    let error = engine
        .stage(Path::new("/etc/passwd"), "{ }\n")
        .expect_err("must refuse");
    assert!(
        matches!(error, EditError::ForbiddenTarget { .. }),
        "expected ForbiddenTarget, got {error}"
    );
}

/// `stage_create` must refuse an existing file rather than silently downgrade to
/// a replace.
#[test]
fn stage_create_refuses_an_existing_file() {
    let fs = support::edit_fixture();
    let engine = engine(&fs);
    let error = engine
        .stage_create(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
        .expect_err("must refuse");
    assert!(
        matches!(error, EditError::AlreadyExists { .. }),
        "expected AlreadyExists, got {error}"
    );
}

/// A commit that crashed leaves a `Pending` snapshot. Reverting it must restore
/// the files it had already written *and* tolerate files it never reached.
#[test]
fn revert_recovers_a_pending_snapshot_after_a_crash() {
    let fs = support::edit_fixture();
    let original_module = read(&fs, MODULE);
    let original_main = read(&fs, MAIN);
    let engine = engine(&fs);

    // Reproduce the crash state by hand: module written, main untouched, record
    // left Pending. This is exactly what a power loss between the two writes
    // leaves behind.
    let written_module = support::valid_module("  x = 1;\n");
    let plan = engine.plan(vec![
        engine
            .stage(Path::new(MODULE), &written_module)
            .expect("stage module"),
        engine
            .stage(
                Path::new(MAIN),
                "{\n  imports = [ ./modules/nhs-packages.nix ];\n}\n",
            )
            .expect("stage main"),
    ]);
    // Commit fully, then rebuild a Pending record that claims only the module
    // was written: simulate by reverting the main file by hand and rewriting
    // the snapshot as Pending with the module's backup only.
    let applied = engine.commit(&plan).expect("commit");
    fs.write(Path::new(MAIN), original_main.as_bytes())
        .expect("simulate crash: main never reached");

    let store = SnapshotStore::new(&fs, Path::new(support::EDIT_STATE_DIR));
    let mut snapshot = store.load(&applied.snapshot_id).expect("load");
    snapshot.outcome = Outcome::Pending;
    snapshot
        .backups
        .retain(|backup| backup.original == Path::new(MODULE));
    snapshot
        .changes
        .retain(|change| change.path == Path::new(MODULE));
    store.save(&snapshot).expect("rewrite as pending");

    // The module holds what we wrote (so it is restored); main holds the
    // original (so "not yet written" must be accepted).
    // But main is no longer in the snapshot, so only the module is considered.
    fs.write(Path::new(MAIN), original_main.as_bytes())
        .expect("main");

    engine
        .revert(&applied.snapshot_id)
        .expect("recover the pending snapshot");
    assert_eq!(read(&fs, MODULE), original_module);
    assert_eq!(read(&fs, MAIN), original_main);
}

// ---------------------------------------------------------------------------
// Failures that happen *after* the operation already had an effect.
// A rename can succeed and the directory flush fail; a create can leave a
// partial file. Neither may produce an unrecorded write.
// ---------------------------------------------------------------------------

/// The rename is the commit point. A failing directory flush afterwards must not
/// turn a completed write into a reported failure — that would skip restoring a
/// file which really did change.
///
/// The fault is scoped to the module's own directory, so it cannot accidentally
/// hit the backup store's flush instead.
#[test]
fn a_directory_sync_failure_after_the_rename_is_not_a_write_failure() {
    let inner = support::edit_fixture();
    let fs = FlakyFs::new(inner, Fault::SyncDirInto("home-manager/modules"));
    let engine = engine(&fs);

    let new_text = support::valid_module("  x = 1;\n");
    let plan = engine.plan(vec![engine
        .stage(Path::new(MODULE), &new_text)
        .expect("stage")]);

    let applied = engine
        .commit(&plan)
        .expect("a flush failure must not fail the write");
    assert_eq!(
        read(fs.inner(), MODULE),
        new_text,
        "the file must hold the new content"
    );
    assert_eq!(applied.written, vec![PathBuf::from(MODULE)]);
    assert!(applied.warning.is_none(), "a clean write has no warning");
    assert_eq!(applied.backups.len(), 1);
}

/// A file whose rename landed and whose flush then failed must still be in the
/// rollback set when a later file fails. This is the regression the review
/// found: the flush error used to make the engine forget the file was written.
#[test]
fn a_file_whose_flush_failed_is_still_restored_after_a_later_failure() {
    let inner = support::edit_fixture();
    let original_module = read(&inner, MODULE);
    let original_main = read(&inner, MAIN);
    let fs = FlakyFs::with_faults(
        inner,
        vec![
            // The module's directory flush fails after its rename succeeded.
            Fault::SyncDirInto("home-manager/modules"),
            // Then the main file's rename fails.
            Fault::RenameInto("home.nix"),
        ],
    );
    let engine = engine(&fs);

    let plan = engine.plan(vec![
        engine
            .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
            .expect("stage"),
        engine
            .stage(
                Path::new(MAIN),
                "{\n  imports = [ ./modules/nhs-packages.nix ];\n}\n",
            )
            .expect("stage"),
    ]);

    engine.commit(&plan).expect_err("must fail on home.nix");
    assert_eq!(
        read(fs.inner(), MODULE),
        original_module,
        "the first file must be restored even though its flush failed"
    );
    assert_eq!(read(fs.inner(), MAIN), original_main);
}

/// A `create` that fails after the file already exists must not leave a partial
/// file behind.
#[test]
fn a_create_that_fails_mid_write_leaves_no_partial_file() {
    let inner = MemFs::new().with_file(format!("{}/home.nix", support::EDIT_CONFIG_ROOT), "{\n}\n");
    let created = format!("{}/modules/brand-new.nix", support::EDIT_CONFIG_ROOT);
    let fs = FlakyFs::new(inner, Fault::WriteNewPartial("brand-new.nix"));
    let engine = engine(&fs);

    let plan = engine.plan(vec![engine
        .stage_create(Path::new(&created), &support::valid_module(""))
        .expect("stage create")]);

    let error = engine.commit(&plan).expect_err("must fail");
    match error {
        EditError::Write { restored, .. } => assert!(restored, "cleanup must be reported"),
        other => panic!("expected Write, got {other}"),
    }
    assert!(
        !fs.inner().exists(Path::new(&created)),
        "a partially written new file must be removed, not left behind"
    );
}

/// A failed `create` in a multi-file plan must clean itself up while restoring
/// the file written before it.
#[test]
fn a_partial_create_is_cleaned_up_when_an_earlier_file_was_written() {
    let inner = support::edit_fixture();
    let original_module = read(&inner, MODULE);
    let created = format!("{}/modules/second.nix", support::EDIT_CONFIG_ROOT);
    let fs = FlakyFs::new(inner, Fault::WriteNewPartial("second.nix"));
    let engine = engine(&fs);

    let plan = engine.plan(vec![
        engine
            .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
            .expect("stage"),
        engine
            .stage_create(Path::new(&created), &support::valid_module("  y = 2;\n"))
            .expect("stage create"),
    ]);

    engine.commit(&plan).expect_err("must fail on the create");
    assert!(
        !fs.inner().exists(Path::new(&created)),
        "the partial file must be gone"
    );
    assert_eq!(
        read(fs.inner(), MODULE),
        original_module,
        "the replaced file must be restored"
    );
}

/// A file created in full and then reported as failed by the final sync must
/// still be removed on rollback: it did not exist before the plan.
#[test]
fn a_create_that_fails_on_the_final_sync_is_still_cleaned_up() {
    let inner = MemFs::new().with_file(format!("{}/home.nix", support::EDIT_CONFIG_ROOT), "{\n}\n");
    let created = format!("{}/modules/late.nix", support::EDIT_CONFIG_ROOT);
    let fs = FlakyFs::new(inner, Fault::WriteNewAfter("late.nix"));
    let engine = engine(&fs);

    let plan = engine.plan(vec![engine
        .stage_create(Path::new(&created), &support::valid_module(""))
        .expect("stage create")]);

    engine.commit(&plan).expect_err("must fail");
    assert!(
        !fs.inner().exists(Path::new(&created)),
        "a file created before the failure must be removed"
    );
}

/// An external edit to a file the plan already wrote must not be rolled over.
#[test]
fn a_rollback_does_not_overwrite_a_newer_external_edit() {
    let inner = support::edit_fixture();
    let fs = FlakyFs::new(inner, Fault::RenameInto("home.nix"));
    let engine = engine(&fs);

    let plan = engine.plan(vec![
        engine
            .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
            .expect("stage"),
        engine
            .stage(
                Path::new(MAIN),
                "{\n  imports = [ ./modules/nhs-packages.nix ];\n}\n",
            )
            .expect("stage"),
    ]);

    // The user saves the module in their editor after our write but before the
    // plan fails on the second file: their edit must survive the rollback.
    let user_text = support::valid_module("  user_edit = true;\n");
    fs.inner()
        .write(Path::new(MODULE), user_text.as_bytes())
        .expect("external edit");

    engine.commit(&plan).expect_err("must fail on home.nix");
    assert_eq!(
        read(fs.inner(), MODULE),
        user_text,
        "a newer external edit must win over the rollback"
    );
}

// ---------------------------------------------------------------------------
// Rollback of a `create` that failed mid-write.
//
// "It did not exist before my plan" does not justify deleting a path: another
// process can claim that path between the failed write and the rollback. A
// delete is therefore only allowed while the content is provably the bytes this
// plan wrote, and anything else is reported instead of removed.
// ---------------------------------------------------------------------------

/// A foreign file that took over the failed path must survive the rollback, and
/// the residue must be reported rather than silently deleted.
#[test]
fn a_leftover_taken_over_by_another_writer_is_never_deleted() {
    let inner = MemFs::new().with_file(format!("{}/home.nix", support::EDIT_CONFIG_ROOT), "{\n}\n");
    let created = format!("{}/modules/claimed.nix", support::EDIT_CONFIG_ROOT);
    let foreign = "{\n  their_edit = true;\n}\n";
    let fs = FlakyFs::with_faults(
        inner,
        vec![
            Fault::WriteNewPartial("claimed.nix"),
            Fault::RemoveIntoTakenOver("claimed.nix", foreign),
        ],
    );
    let engine = engine(&fs);

    let plan = engine.plan(vec![engine
        .stage_create(Path::new(&created), &support::valid_module(""))
        .expect("stage create")]);

    let error = engine.commit(&plan).expect_err("must fail");
    match error {
        EditError::Write {
            residue, restored, ..
        } => {
            assert_eq!(
                residue,
                Some(PathBuf::from(&created)),
                "the path that could not be confirmed must be reported"
            );
            assert!(restored, "no earlier file needed restoring");
        }
        other => panic!("expected Write, got {other}"),
    }
    assert_eq!(
        read(fs.inner(), &created),
        foreign,
        "another writer's file must not be deleted by our rollback"
    );
}

/// Our own partial write, still in place, is removed even though the immediate
/// cleanup failed — that is the difference between "ours" and "unconfirmed".
#[test]
fn a_leftover_that_is_still_our_own_partial_write_is_removed() {
    let inner = MemFs::new().with_file(format!("{}/home.nix", support::EDIT_CONFIG_ROOT), "{\n}\n");
    let created = format!("{}/modules/ours.nix", support::EDIT_CONFIG_ROOT);
    let fs = FlakyFs::with_faults(
        inner,
        vec![
            Fault::WriteNewPartial("ours.nix"),
            Fault::RemoveInto("ours.nix"),
        ],
    );
    let engine = engine(&fs);

    let plan = engine.plan(vec![engine
        .stage_create(Path::new(&created), &support::valid_module(""))
        .expect("stage create")]);

    let error = engine.commit(&plan).expect_err("must fail");
    match error {
        EditError::Write {
            residue, restored, ..
        } => {
            assert!(restored, "the partial file was cleaned up");
            assert_eq!(residue, None, "cleaned up, so there is nothing to report");
        }
        other => panic!("expected Write, got {other}"),
    }
    assert!(
        !fs.inner().exists(Path::new(&created)),
        "a partial file that is provably ours must be removed"
    );
}

/// A leftover that cannot even be read cannot be attributed to this plan, so it
/// is reported and left alone.
#[test]
fn an_unreadable_leftover_is_reported_instead_of_deleted() {
    let inner = MemFs::new().with_file(format!("{}/home.nix", support::EDIT_CONFIG_ROOT), "{\n}\n");
    let created = format!("{}/modules/opaque.nix", support::EDIT_CONFIG_ROOT);
    let fs = FlakyFs::with_faults(
        inner,
        vec![
            Fault::WriteNewPartial("opaque.nix"),
            Fault::RemoveInto("opaque.nix"),
            Fault::ReadBytesInto("opaque.nix"),
        ],
    );
    let engine = engine(&fs);

    let plan = engine.plan(vec![engine
        .stage_create(Path::new(&created), &support::valid_module(""))
        .expect("stage create")]);

    let error = engine.commit(&plan).expect_err("must fail");
    match error {
        EditError::Write {
            residue, restored, ..
        } => {
            assert_eq!(residue, Some(PathBuf::from(&created)));
            assert!(restored);
        }
        other => panic!("expected Write, got {other}"),
    }
    assert!(
        fs.inner().exists(Path::new(&created)),
        "a file whose content cannot be read must not be deleted"
    );
    let message = EditError::Write {
        path: PathBuf::from(&created),
        restored: true,
        residue: Some(PathBuf::from(&created)),
        source: std::io::Error::other("x"),
    }
    .to_string();
    assert!(
        message.contains("inspect it before retrying"),
        "the report must tell the user to inspect the path: {message}"
    );
}

/// A takeover in the middle of a multi-file plan must not stop the earlier file
/// from being restored, and must still preserve the foreign file.
#[test]
fn a_taken_over_leftover_does_not_block_restoring_earlier_files() {
    let inner = support::edit_fixture();
    let original_module = read(&inner, MODULE);
    let created = format!("{}/modules/late-claim.nix", support::EDIT_CONFIG_ROOT);
    let foreign = "{\n  claimed = true;\n}\n";
    let fs = FlakyFs::with_faults(
        inner,
        vec![
            Fault::WriteNewPartial("late-claim.nix"),
            Fault::RemoveIntoTakenOver("late-claim.nix", foreign),
        ],
    );
    let engine = engine(&fs);

    let plan = engine.plan(vec![
        engine
            .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
            .expect("stage"),
        engine
            .stage_create(Path::new(&created), &support::valid_module("  y = 2;\n"))
            .expect("stage create"),
    ]);

    engine.commit(&plan).expect_err("must fail on the create");
    assert_eq!(
        read(fs.inner(), MODULE),
        original_module,
        "the replaced file must be restored"
    );
    assert_eq!(
        read(fs.inner(), &created),
        foreign,
        "the foreign file must survive"
    );
}

// ---------------------------------------------------------------------------
// A guard must only pass on evidence.
//
// "The path is not there" and "the path is there but I could not read it" are
// different facts. Collapsing them lets a failed read satisfy `Expect(None)` and
// authorise deleting a file whose bytes nobody verified.
// ---------------------------------------------------------------------------

/// A target that cannot be read must never satisfy an absence guard.
#[test]
fn an_unreadable_target_never_satisfies_an_absence_guard() {
    let target = format!("{}/modules/absent.nix", support::EDIT_CONFIG_ROOT);
    let inner = MemFs::new().with_file(&target, support::valid_module("  theirs = true;\n"));
    let fs = FlakyFs::new(inner, Fault::ReadBytesInto("absent.nix"));
    let store = BackupStore::new(&fs, Path::new(support::EDIT_STATE_DIR));
    let entry = BackupEntry {
        original: PathBuf::from(&target),
        stored: PathBuf::new(),
        sha256: Sha256::of_bytes(b""),
        existed: false,
    };

    let error = store
        .restore_all_guarded(&[entry], &[RestoreGuard::Expect(None)])
        .expect_err("an unreadable target must not be treated as absent");
    assert!(
        error.to_string().contains("could not be read"),
        "the refusal must say what went wrong: {error}"
    );
    assert!(
        fs.inner().exists(Path::new(&target)),
        "a file that could not be verified must not be deleted"
    );
}

/// The pattern the review described: a guard taken while the path was absent,
/// somebody else creating the path afterwards, and the restore finding it
/// unreadable. Their file must survive untouched.
#[test]
fn a_file_created_after_the_guard_was_taken_survives_a_failed_read() {
    let target = format!("{}/modules/recreated.nix", support::EDIT_CONFIG_ROOT);
    let foreign = "{\n  theirs = true;\n}\n";
    let inner = MemFs::new().with_file(&target, foreign);
    let fs = FlakyFs::new(inner, Fault::ReadBytesInto("recreated.nix"));
    let store = BackupStore::new(&fs, Path::new(support::EDIT_STATE_DIR));
    let entry = BackupEntry {
        original: PathBuf::from(&target),
        stored: PathBuf::new(),
        sha256: Sha256::of_bytes(b""),
        existed: false,
    };

    store
        .restore_all_guarded(&[entry], &[RestoreGuard::Expect(None)])
        .expect_err("must refuse");
    assert_eq!(
        read(fs.inner(), &target),
        foreign,
        "another writer's file must still hold their bytes"
    );
}

/// The snapshot record must state the outcome the caller was told, including a
/// rollback that failed — the record is what a user reads after a crash.
#[test]
fn the_snapshot_error_is_exactly_the_error_that_is_returned() {
    let inner = support::edit_fixture();
    let fs = FlakyFs::with_faults(
        inner,
        vec![
            // The second file's rename fails …
            Fault::RenameInto("home.nix"),
            // … and restoring the first file fails on its own write.
            Fault::WriteNewInto("home-manager/modules", 2),
        ],
    );
    let engine = engine(&fs);

    let plan = engine.plan(vec![
        engine
            .stage(Path::new(MODULE), &support::valid_module("  x = 1;\n"))
            .expect("stage"),
        engine
            .stage(
                Path::new(MAIN),
                "{\n  imports = [ ./modules/nhs-packages.nix ];\n}\n",
            )
            .expect("stage"),
    ]);

    let error = engine
        .commit(&plan)
        .expect_err("the rollback cannot succeed");
    match &error {
        EditError::Write { restored, .. } => {
            assert!(
                !*restored,
                "the rollback really failed, so it must not claim success"
            );
        }
        other => panic!("expected Write, got {other}"),
    }
    let text = error.to_string();
    assert!(
        text.contains("restoring earlier files also failed"),
        "the message must not claim a restore that did not happen: {text}"
    );

    let snapshots = SnapshotStore::new(fs.inner(), Path::new(support::EDIT_STATE_DIR));
    let ids: Vec<String> = fs
        .inner()
        .read_dir(snapshots.root())
        .expect("list snapshots")
        .iter()
        .filter_map(|path| path.file_stem().and_then(|stem| stem.to_str()))
        .map(str::to_string)
        .collect();
    assert_eq!(ids.len(), 1, "exactly one attempt was recorded");
    let snapshot = snapshots.load(&ids[0]).expect("load the snapshot");
    assert_eq!(snapshot.outcome, Outcome::Failed);
    assert_eq!(
        snapshot.error.as_deref(),
        Some(text.as_str()),
        "the snapshot must record the error the caller received"
    );
}

// ---------------------------------------------------------------------------
// Concurrency boundary
//
// A guard is a check followed by an action, and no POSIX path operation can make
// those two atomic. What the engine *can* do is make the action the very next
// filesystem call after the check, so the window a concurrent writer could use
// is one syscall wide instead of "however far apart the next caller puts them".
// This test locks that property: reintroducing any probe or lookup between the
// verifying read and the delete turns it red.
// ---------------------------------------------------------------------------

/// The delete of a file this plan created must be the next filesystem call after
/// the read that verified its content.
#[test]
fn a_guarded_delete_immediately_follows_its_verifying_read() {
    let inner = support::edit_fixture();
    let created = format!("{}/modules/only.nix", support::EDIT_CONFIG_ROOT);
    let fs = FlakyFs::new(inner, Fault::RenameInto("home.nix"));
    let engine = engine(&fs);

    let plan = engine.plan(vec![
        engine
            .stage_create(Path::new(&created), &support::valid_module(""))
            .expect("stage create"),
        engine
            .stage(
                Path::new(MAIN),
                "{\n  imports = [ ./modules/only.nix ];\n}\n",
            )
            .expect("stage"),
    ]);

    engine.commit(&plan).expect_err("must fail on home.nix");
    assert!(
        !fs.inner().exists(Path::new(&created)),
        "the created file must have been rolled back"
    );

    let ops = fs.ops();
    let remove = ops
        .iter()
        .position(|op| op == &format!("remove_file {created}"))
        .unwrap_or_else(|| panic!("no delete of {created} was issued: {ops:#?}"));
    assert!(
        remove >= 2,
        "expected an existence probe and a read before the delete: {ops:#?}"
    );
    assert_eq!(
        ops[remove - 1],
        format!("read_bytes {created}"),
        "the delete must follow the verifying read with nothing in between: {ops:#?}"
    );
    assert_eq!(
        ops[remove - 2],
        format!("exists {created}"),
        "the observed state must come from one probe plus one read: {ops:#?}"
    );
}
