//! The safe edit engine.
//!
//! This is the only module allowed to write files, and the only entry point that
//! writes is [`EditEngine::commit`] (and its inverse, [`EditEngine::revert`]).
//! Everything else produces a [`Plan`], which is a plain value the UI renders
//! and the user approves.
//!
//! Normative rules (architecture §7.1, gates G3/G5/G6):
//!
//! 1. a commit accepts a [`Plan`], never a path plus text, and it re-derives
//!    every hash and diff from `old_text`/`new_text` so a plan edited after
//!    review is rejected rather than trusted;
//! 2. no two changes may target the same file;
//! 3. every change's hash is checked up front *and again immediately before its
//!    own write*; a mismatch aborts and restores only what was already written;
//! 4. every change is backed up before any change is written, and a backup whose
//!    bytes do not match the reviewed hash is a conflict;
//! 5. each file is replaced atomically (temporary file, then rename), and a
//!    failure restores every file already written — files not yet written are
//!    never touched. A restore is always content-guarded, including the removal
//!    of a file this plan created: a path whose bytes cannot be attributed to
//!    this write is reported as a residue and left alone, never deleted;
//! 6. every written `.nix` file is re-parsed, and an unparseable planned text is
//!    refused *before* it is written;
//! 7. a snapshot records the attempt before the writes and is updated after; if
//!    the update fails the commit still reports success, with a warning, because
//!    the bytes did land.

pub mod backup;
pub mod diff;
pub mod hash;
pub mod lock;
pub mod snapshot;
pub mod validate;

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use self::backup::{BackupEntry, BackupStore, RestoreGuard};
use self::hash::Sha256;
use self::lock::{EditLock, Locked};
use self::snapshot::{Outcome, Snapshot, SnapshotBackup, SnapshotChange, SnapshotStore};
use crate::fs::WriteFs;

/// How a change relates to the file on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeKind {
    /// The file does not exist and will be created.
    Create,
    /// The file exists and will be replaced with new content.
    Replace,
}

impl ChangeKind {
    /// Stable lowercase name, used in snapshots and JSON.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Replace => "replace",
        }
    }
}

/// One file in a plan, with the diff the user reviews.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: PathBuf,
    pub kind: ChangeKind,
    /// Hash of the file when the plan was made. `None` only for
    /// [`ChangeKind::Create`].
    pub expected_sha256: Option<Sha256>,
    /// Current content, empty for a create.
    pub old_text: String,
    /// Content that will be written.
    pub new_text: String,
    /// Unified diff, empty when nothing changes.
    pub unified_diff: String,
    pub added_lines: usize,
    pub removed_lines: usize,
    /// Set when `path` came from resolving a symlink, so the UI can say
    /// "this writes through your symlink to <real path>".
    pub resolved_from: Option<PathBuf>,
}

impl FileChange {
    /// True when the change only inserts lines and modifies nothing.
    ///
    /// Derived from the diff, so a caller cannot claim "one added line" while
    /// rewriting the file.
    #[must_use]
    pub fn is_pure_insertion(&self) -> bool {
        diff::is_pure_insertion(&self.old_text, &self.new_text)
    }

    /// True when the planned text is identical to the staged text.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.kind == ChangeKind::Replace && self.old_text == self.new_text
    }
}

/// A reviewed set of changes. The only value [`EditEngine::commit`] accepts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub changes: Vec<FileChange>,
    /// Human-readable warnings shown next to the diff (e.g. system-only options).
    pub notes: Vec<String>,
}

impl Plan {
    /// True when the plan would not change any file's content.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.iter().all(FileChange::is_noop)
    }

    /// Every path the plan would write, in order.
    #[must_use]
    pub fn paths(&self) -> Vec<&Path> {
        self.changes
            .iter()
            .filter(|change| !change.is_noop())
            .map(|change| change.path.as_path())
            .collect()
    }
}

/// What a successful commit produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub snapshot_id: String,
    pub written: Vec<PathBuf>,
    /// Backups kept for a later [`EditEngine::revert`].
    pub backups: Vec<BackupEntry>,
    /// Set when the bytes landed but the snapshot could not be updated.
    /// The commit did happen; only the record is incomplete.
    pub warning: Option<String>,
}

impl Applied {
    fn nothing() -> Self {
        Self {
            snapshot_id: String::new(),
            written: Vec::new(),
            backups: Vec::new(),
            warning: None,
        }
    }
}

/// Everything that can stop a commit.
#[derive(Debug)]
pub enum EditError {
    /// A file changed after the plan was made. Nothing was written.
    Conflict {
        path: PathBuf,
        expected: Option<Sha256>,
        actual: Option<Sha256>,
    },
    /// The planned text is not valid Nix. Nothing was written.
    InvalidNix { path: PathBuf, detail: String },
    /// Another instance holds the edit lock.
    Locked(Box<Locked>),
    /// A snapshot or backup could not be read or written.
    Storage { path: PathBuf, source: io::Error },
    /// A write failed.
    ///
    /// `restored` says whether every file this attempt had already replaced was
    /// put back. `residue` names a path the failed write may have left content at
    /// and that the rollback deliberately did not touch, because its content
    /// could not be attributed to this plan; it needs inspection by the user.
    Write {
        path: PathBuf,
        restored: bool,
        residue: Option<PathBuf>,
        source: io::Error,
    },
    /// The state directory would sit inside the configuration tree.
    StateInsideConfig {
        state_dir: PathBuf,
        config_root: PathBuf,
    },
    /// The target is a symlink that does not resolve, so there is no real file
    /// to edit.
    DanglingSymlink { path: PathBuf },
    /// Two changes in one plan target the same file.
    DuplicatePath { path: PathBuf },
    /// A staged change is internally inconsistent — e.g. the hash does not match
    /// `old_text`, or the diff does not match the texts. The plan was not
    /// reviewed as claimed.
    InconsistentPlan { path: PathBuf, detail: String },
    /// The plan is empty of real work but a file it refers to is not in the
    /// staged state any more.
    ///
    /// (A no-op change is still a claim about the file on disk.)
    NoopNotOnDisk { path: PathBuf },
    /// Reverting would overwrite content that this snapshot did not write.
    StaleRevert { path: PathBuf, detail: String },
    /// A snapshot id or a path inside a snapshot is not acceptable.
    InvalidSnapshot { detail: String },
    /// The request targets a path outside the configuration tree.
    ForbiddenTarget { path: PathBuf, config_root: PathBuf },
    /// `stage_create` was asked to create a file that already exists.
    AlreadyExists { path: PathBuf },
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict { path, .. } => write!(
                f,
                "{} changed since the plan was reviewed; nothing was written. \
                 Reload and review again.",
                path.display()
            ),
            Self::InvalidNix { path, detail } => write!(
                f,
                "the planned content for {} is not valid Nix ({detail}); nothing was written",
                path.display()
            ),
            Self::Locked(locked) => write!(f, "{locked}"),
            Self::Storage { path, source } => {
                write!(f, "cannot use {}: {source}", path.display())
            }
            Self::Write {
                path,
                restored,
                residue,
                source,
            } => {
                write!(f, "writing {} failed ({source})", path.display())?;
                if *restored && residue.is_none() {
                    write!(f, "; every already-written file was restored")?;
                }
                if !*restored {
                    write!(
                        f,
                        "; restoring earlier files also failed — see the snapshot backups"
                    )?;
                }
                if let Some(residue) = residue {
                    write!(
                        f,
                        "; {} may hold content left by the failed write and was left \
                         untouched, because its content could not be confirmed as this plan's \
                         own — inspect it before retrying",
                        residue.display()
                    )?;
                }
                Ok(())
            }
            Self::StateInsideConfig {
                state_dir,
                config_root,
            } => write!(
                f,
                "state directory {} is inside the configuration tree {}; backups must live \
                 outside it",
                state_dir.display(),
                config_root.display()
            ),
            Self::DanglingSymlink { path } => write!(
                f,
                "{} is a symlink that does not resolve; refusing to replace it",
                path.display()
            ),
            Self::DuplicatePath { path } => write!(
                f,
                "the plan changes {} more than once; combine the changes into one",
                path.display()
            ),
            Self::InconsistentPlan { path, detail } => write!(
                f,
                "the plan entry for {} is inconsistent ({detail}); it was not applied",
                path.display()
            ),
            Self::NoopNotOnDisk { path } => write!(
                f,
                "{} no longer matches what the plan was built from, even though the plan \
                 would not change it",
                path.display()
            ),
            Self::StaleRevert { path, detail } => {
                write!(f, "refusing to revert {}: {detail}", path.display())
            }
            Self::InvalidSnapshot { detail } => write!(f, "unusable snapshot: {detail}"),
            Self::ForbiddenTarget { path, config_root } => write!(
                f,
                "{} is outside the configuration tree {}; only files inside it may be \
                 changed (a symlink inside the tree may point outside)",
                path.display(),
                config_root.display()
            ),
            Self::AlreadyExists { path } => write!(
                f,
                "{} already exists; use stage() to plan replacing it",
                path.display()
            ),
        }
    }
}

impl std::error::Error for EditError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Locked(locked) => Some(locked),
            Self::Storage { source, .. } | Self::Write { source, .. } => Some(source),
            Self::Conflict { .. }
            | Self::InvalidNix { .. }
            | Self::StateInsideConfig { .. }
            | Self::DanglingSymlink { .. }
            | Self::DuplicatePath { .. }
            | Self::InconsistentPlan { .. }
            | Self::NoopNotOnDisk { .. }
            | Self::StaleRevert { .. }
            | Self::InvalidSnapshot { .. }
            | Self::ForbiddenTarget { .. }
            | Self::AlreadyExists { .. } => None,
        }
    }
}

/// Where backups, snapshots and the lock live, plus the tree they must stay out of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateLayout {
    /// Application state directory, e.g. `~/.local/state/nix-home-studio`.
    pub state_dir: PathBuf,
    /// The configuration tree the app edits. Backups must not live inside it.
    pub config_root: PathBuf,
}

/// Stages and commits changes.
pub struct EditEngine<'a> {
    fs: &'a dyn WriteFs,
    layout: StateLayout,
}

impl<'a> EditEngine<'a> {
    #[must_use]
    pub fn new(fs: &'a dyn WriteFs, layout: StateLayout) -> Self {
        Self { fs, layout }
    }

    /// Plan replacing `path` with `new_text`, reading the current content.
    ///
    /// Reads only: the file is not touched. A missing file becomes
    /// [`ChangeKind::Create`] with no expected hash.
    pub fn stage(&self, path: &Path, new_text: &str) -> Result<FileChange, EditError> {
        let (path, resolved_from) = self.resolve_symlink(path)?;
        let path = path.as_path();
        let existed = self.fs.exists(path);
        let old_text = if existed {
            self.fs
                .read_to_string(path)
                .map_err(|source| EditError::Storage {
                    path: path.to_path_buf(),
                    source,
                })?
        } else {
            String::new()
        };
        let expected_sha256 = if existed {
            Some(Sha256::of_bytes(old_text.as_bytes()))
        } else {
            None
        };
        let kind = if existed {
            ChangeKind::Replace
        } else {
            ChangeKind::Create
        };
        Ok(build_change(
            path,
            resolved_from,
            kind,
            expected_sha256,
            old_text,
            new_text,
        ))
    }

    /// Plan creating `path`, refusing if it already exists.
    ///
    /// Use this when overwriting would be a mistake (a generated module that
    /// the app does not own yet). To replace an existing file, use [`EditEngine::stage`].
    pub fn stage_create(&self, path: &Path, new_text: &str) -> Result<FileChange, EditError> {
        let (path, resolved_from) = self.resolve_symlink(path)?;
        let path = path.as_path();
        if self.fs.exists(path) {
            return Err(EditError::AlreadyExists {
                path: path.to_path_buf(),
            });
        }
        Ok(build_change(
            path,
            resolved_from,
            ChangeKind::Create,
            None,
            String::new(),
            new_text,
        ))
    }

    /// Follow a final symlink so a dotfiles-managed `home.nix` is edited in
    /// place instead of being replaced by a regular file.
    ///
    /// Replacing the link itself would silently disconnect the file from the
    /// user's repository, so the real target is returned and the original path
    /// is remembered for reporting.
    fn resolve_symlink(&self, path: &Path) -> Result<(PathBuf, Option<PathBuf>), EditError> {
        // Only paths inside the configuration tree may be changed. A symlink
        // *inside* the tree is the user's explicit grant to write its target
        // (the normal dotfiles layout), so resolution may leave the tree — but
        // the request itself never may.
        //
        // The check canonicalizes the *parent* only: following the final
        // component would turn "a link inside the tree" into "a path outside the
        // tree" and reject the very layout we mean to support.
        let root = self.canonical_prefix(&self.layout.config_root);
        let requested = self.canonical_parent_joined(path);
        if !requested.starts_with(&root) {
            return Err(EditError::ForbiddenTarget {
                path: path.to_path_buf(),
                config_root: self.layout.config_root.clone(),
            });
        }
        if !self.fs.is_symlink(path) {
            return Ok((path.to_path_buf(), None));
        }
        match self.fs.canonicalize(path) {
            Ok(target) => Ok((target, Some(path.to_path_buf()))),
            Err(_) => Err(EditError::DanglingSymlink {
                path: path.to_path_buf(),
            }),
        }
    }

    /// Assemble a plan from already-staged changes.
    #[must_use]
    pub fn plan(&self, changes: Vec<FileChange>) -> Plan {
        Plan {
            changes,
            notes: Vec::new(),
        }
    }

    /// Write a plan to disk.
    ///
    /// Every step either completes for all files or leaves the tree as it was:
    ///
    /// 1. refuse a state directory inside the config tree;
    /// 2. reject an internally inconsistent plan or duplicate targets;
    /// 3. take the lock;
    /// 4. verify every hash and parse every planned text (no writes yet);
    /// 5. back up every file, and require the backup to match the reviewed hash;
    /// 6. record a `Pending` snapshot;
    /// 7. for each file: re-verify its hash, then write and verify it; on any
    ///    failure restore exactly the files already written;
    /// 8. record `Applied`.
    pub fn commit(&self, plan: &Plan) -> Result<Applied, EditError> {
        self.check_layout()?;
        self.validate_plan(plan)?;

        let pending: Vec<&FileChange> = plan
            .changes
            .iter()
            .filter(|change| !change.is_noop())
            .collect();

        let _lock = EditLock::acquire(self.fs, &self.layout.state_dir)
            .map_err(|locked| EditError::Locked(Box::new(locked)))?;

        // A no-op entry is still a claim about the file on disk. Check it, so a
        // deleted or edited file cannot be reported as "already applied".
        for change in plan.changes.iter().filter(|change| change.is_noop()) {
            if !self.matches_expected(change)? {
                return Err(EditError::NoopNotOnDisk {
                    path: change.path.clone(),
                });
            }
        }

        if pending.is_empty() {
            return Ok(Applied::nothing());
        }

        // 4. Nothing is written until every file has passed both guards.
        for change in &pending {
            self.verify_unchanged(change)?;
            Self::verify_parses(change)?;
        }

        // 5. Back up everything before writing anything, and insist that what we
        //    backed up is exactly what the user reviewed.
        let store = BackupStore::new(self.fs, &self.layout.state_dir);
        let mut entries: Vec<BackupEntry> = Vec::with_capacity(pending.len());
        for change in &pending {
            let entry = store
                .save(&change.path)
                .map_err(|source| EditError::Storage {
                    path: change.path.clone(),
                    source,
                })?;
            let expected = change.expected_sha256;
            let matches = match (expected, entry.existed) {
                (Some(expected), true) => entry.sha256 == expected,
                (None, false) => true,
                _ => false,
            };
            if !matches {
                return Err(EditError::Conflict {
                    path: change.path.clone(),
                    expected,
                    actual: entry.existed.then_some(entry.sha256),
                });
            }
            entries.push(entry);
        }

        // 6. Record the attempt before the risky step.
        let snapshots = SnapshotStore::new(self.fs, &self.layout.state_dir);
        let id = new_snapshot_id();
        let mut snapshot = Snapshot::pending(id.clone(), unix_now());
        snapshot.changes = pending
            .iter()
            .map(|change| snapshot_change(change))
            .collect();
        snapshot.backups = entries.iter().map(snapshot_backup).collect();
        snapshots
            .save_new(&snapshot)
            .map_err(|source| EditError::Storage {
                path: snapshots.path_for(&id),
                source,
            })?;

        // 7. Write, restoring exactly what was written if anything fails.
        //
        // `touched` records a file as soon as its bytes may exist on disk,
        // *before* the read-back check and even when the last step of the write
        // reported an error. Each entry carries the content the rollback must
        // still find in place before it touches the path.
        let mut touched: Vec<(usize, RestoreGuard)> = Vec::new();
        let mut residue: Option<PathBuf> = None;
        let mut failure: Option<EditError> = None;
        for (index, change) in pending.iter().enumerate() {
            // Re-check immediately before this file's write: the initial sweep
            // and the backup both take time, and the user's editor does not
            // wait for us.
            if let Err(error) = self.verify_unchanged(change) {
                failure = Some(error);
                break;
            }
            if let Err(error) = self.land_one(change) {
                match error.leftover {
                    // Our own partial write: delete it, but only while it still
                    // holds exactly these bytes.
                    Leftover::Ours(written) => {
                        touched.push((index, RestoreGuard::Expect(Some(written))));
                    }
                    Leftover::Nothing => {}
                    // Something we cannot claim as ours: report it, never touch it.
                    Leftover::Unattributed => residue = Some(change.path.clone()),
                }
                failure = Some(EditError::Write {
                    path: change.path.clone(),
                    restored: true,
                    residue: residue.clone(),
                    source: error.source,
                });
                break;
            }
            touched.push((
                index,
                RestoreGuard::Expect(Some(Sha256::of_bytes(change.new_text.as_bytes()))),
            ));
            // The bytes are on disk now. A failed read-back is a failure, but the
            // file is already in `touched` so it will be restored.
            if let Err(source) = self.verify_landed(change) {
                failure = Some(EditError::Write {
                    path: change.path.clone(),
                    restored: true,
                    residue: None,
                    source,
                });
                break;
            }
        }

        if let Some(error) = failure {
            // Only the files this attempt touched are restored; files it never
            // reached keep whatever the user did to them. Every restore is
            // guarded by the content we believe we put there, so a path that
            // somebody else has claimed since — restored or created — is left
            // alone rather than overwritten or deleted.
            let mut restore_entries: Vec<BackupEntry> = Vec::with_capacity(touched.len());
            let mut guards: Vec<RestoreGuard> = Vec::with_capacity(touched.len());
            for (index, guard) in &touched {
                if let Some(entry) = entries.get(*index) {
                    restore_entries.push(entry.clone());
                    guards.push(*guard);
                }
            }
            let restore_ok = store.restore_all_guarded(&restore_entries, &guards).is_ok();
            // Build the final error *before* recording it: the snapshot must say
            // what actually happened, so its text has to be the text the caller
            // receives — including whether the rollback succeeded.
            let error = match error {
                EditError::Write {
                    path,
                    source,
                    residue,
                    restored: _,
                } => EditError::Write {
                    path,
                    restored: restore_ok,
                    residue,
                    source,
                },
                other => other,
            };
            snapshot.outcome = Outcome::Failed;
            snapshot.error = Some(error.to_string());
            let _ = snapshots.save(&snapshot);
            return Err(error);
        }

        // 8. Record success. A failure here does not undo real work.
        snapshot.outcome = Outcome::Applied;
        let warning = snapshots.save(&snapshot).err().map(|error| {
            format!(
                "the files were written, but the snapshot {} could not be updated: {error}",
                snapshots.path_for(&id).display()
            )
        });

        Ok(Applied {
            snapshot_id: id,
            written: touched
                .iter()
                .filter_map(|(index, _)| pending.get(*index).map(|change| change.path.clone()))
                .collect(),
            backups: entries,
            warning,
        })
    }

    /// Restore the files recorded in a snapshot.
    ///
    /// Before writing anything, every target is checked: it must still hold what
    /// this snapshot wrote (or still be absent, for a created file). Otherwise
    /// the revert would silently discard somebody's later edit, so it refuses.
    pub fn revert(&self, snapshot_id: &str) -> Result<Vec<PathBuf>, EditError> {
        self.check_layout()?;
        validate_snapshot_id(snapshot_id)?;

        // A revert is a write like any other: it must not race a commit.
        let _lock = EditLock::acquire(self.fs, &self.layout.state_dir)
            .map_err(|locked| EditError::Locked(Box::new(locked)))?;

        let snapshots = SnapshotStore::new(self.fs, &self.layout.state_dir);
        let snapshot = snapshots
            .load(snapshot_id)
            .map_err(|source| EditError::Storage {
                path: snapshots.path_for(snapshot_id),
                source,
            })?;
        if snapshot.id != snapshot_id {
            return Err(EditError::InvalidSnapshot {
                detail: format!(
                    "record id {} does not match the requested id {snapshot_id}",
                    snapshot.id
                ),
            });
        }

        // A snapshot is data on disk. Never let it name a path outside the tree
        // this engine owns — unless it records a symlink inside that tree as the
        // way in, which is the user's own grant (dotfiles layout).
        let config_root = self.canonical_prefix(&self.layout.config_root);
        let backup_root = normalize_for_compare(&self.layout.state_dir.join("backups"));
        let mut entries: Vec<BackupEntry> = Vec::with_capacity(snapshot.backups.len());
        for backup in &snapshot.backups {
            let change = snapshot
                .changes
                .iter()
                .find(|change| change.path == backup.original)
                .ok_or_else(|| EditError::InvalidSnapshot {
                    detail: format!(
                        "there is a backup for {} but no recorded change",
                        backup.original.display()
                    ),
                })?;

            let original = self.canonical_prefix(&backup.original);
            let via_owned_symlink = change.resolved_from.as_ref().is_some_and(|link| {
                // Check where the *link* lives, not where it points.
                self.canonical_parent_joined(link).starts_with(&config_root)
            });
            if !original.starts_with(&config_root) && !via_owned_symlink {
                return Err(EditError::InvalidSnapshot {
                    detail: format!(
                        "recorded path {} is outside the configuration tree {}",
                        backup.original.display(),
                        self.layout.config_root.display()
                    ),
                });
            }
            if backup.existed && !normalize_for_compare(&backup.stored).starts_with(&backup_root) {
                return Err(EditError::InvalidSnapshot {
                    detail: format!(
                        "recorded backup {} is outside {}",
                        backup.stored.display(),
                        backup_root.display()
                    ),
                });
            }
            let sha256 =
                Sha256::parse_hex(&backup.sha256).ok_or_else(|| EditError::InvalidSnapshot {
                    detail: format!(
                        "backup digest for {} is not valid hex",
                        backup.original.display()
                    ),
                })?;
            entries.push(BackupEntry {
                original: backup.original.clone(),
                stored: backup.stored.clone(),
                sha256,
                existed: backup.existed,
            });
        }

        // Refuse a stale revert: the file must still hold what we wrote, or (for
        // a commit that crashed before reaching it) exactly what it held before.
        let mut guards: Vec<RestoreGuard> = Vec::with_capacity(entries.len());
        for entry in &entries {
            let change = snapshot
                .changes
                .iter()
                .find(|change| change.path == entry.original)
                .ok_or_else(|| EditError::InvalidSnapshot {
                    detail: format!(
                        "there is a backup for {} but no recorded change",
                        entry.original.display()
                    ),
                })?;
            let written = Sha256::parse_hex(&change.new_sha256).ok_or_else(|| {
                EditError::InvalidSnapshot {
                    detail: format!(
                        "the recorded hash for {} is not valid hex",
                        change.path.display()
                    ),
                }
            })?;
            let before = change
                .old_sha256
                .as_deref()
                .map(|text| {
                    Sha256::parse_hex(text).ok_or_else(|| EditError::InvalidSnapshot {
                        detail: format!(
                            "the recorded previous hash for {} is not valid hex",
                            change.path.display()
                        ),
                    })
                })
                .transpose()?;

            let current = self.current_hash(&entry.original)?;
            // Written, or (crash recovery) never reached.
            let acceptable = current == Some(written) || current == before;
            if !acceptable {
                return Err(EditError::StaleRevert {
                    path: entry.original.clone(),
                    detail: "the file no longer matches what this snapshot wrote; \
                             it was edited or replaced afterwards"
                        .to_string(),
                });
            }
            guards.push(RestoreGuard::Expect(current));
        }

        let store = BackupStore::new(self.fs, &self.layout.state_dir);
        // Verify every backup before restoring any of them.
        for entry in &entries {
            store.load(entry).map_err(|source| EditError::Storage {
                path: entry.stored.clone(),
                source,
            })?;
        }
        store
            .restore_all_guarded(&entries, &guards)
            .map_err(|source| EditError::Storage {
                path: self.layout.state_dir.clone(),
                source,
            })?;
        Ok(entries.iter().map(|entry| entry.original.clone()).collect())
    }

    /// Reject a plan that could not have been reviewed as presented.
    fn validate_plan(&self, plan: &Plan) -> Result<(), EditError> {
        let mut seen: Vec<&Path> = Vec::with_capacity(plan.changes.len());
        let root = self.canonical_prefix(&self.layout.config_root);
        for change in &plan.changes {
            if seen.contains(&change.path.as_path()) {
                return Err(EditError::DuplicatePath {
                    path: change.path.clone(),
                });
            }
            seen.push(change.path.as_path());

            // Defence in depth: `stage` already enforces this, but a plan is a
            // plain value and could be built by hand.
            let inside = self
                .canonical_parent_joined(&change.path)
                .starts_with(&root);
            let via_link = change
                .resolved_from
                .as_ref()
                .is_some_and(|link| self.canonical_parent_joined(link).starts_with(&root));
            if !inside && !via_link {
                return Err(EditError::ForbiddenTarget {
                    path: change.path.clone(),
                    config_root: self.layout.config_root.clone(),
                });
            }

            // The plan is a value; a UI could mutate it after review. Re-derive
            // the invariants from `old_text`/`new_text` instead of trusting them.
            let name = change
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("file");
            let expected_diff = diff::unified(&change.old_text, &change.new_text, name);
            if expected_diff != change.unified_diff {
                return Err(EditError::InconsistentPlan {
                    path: change.path.clone(),
                    detail: "the diff does not match the old and new text".to_string(),
                });
            }
            let old_hash = Sha256::of_bytes(change.old_text.as_bytes());
            let (added, removed) = diff::counts(&change.old_text, &change.new_text);
            if added != change.added_lines || removed != change.removed_lines {
                return Err(EditError::InconsistentPlan {
                    path: change.path.clone(),
                    detail: "the line counts do not match the diff".to_string(),
                });
            }
            match change.kind {
                ChangeKind::Replace => {
                    if change.expected_sha256 != Some(old_hash) {
                        return Err(EditError::InconsistentPlan {
                            path: change.path.clone(),
                            detail: "the expected hash does not match the old text".to_string(),
                        });
                    }
                }
                ChangeKind::Create => {
                    if change.expected_sha256.is_some() || !change.old_text.is_empty() {
                        return Err(EditError::InconsistentPlan {
                            path: change.path.clone(),
                            detail: "a create must have no expected hash and no old text"
                                .to_string(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Refuse a layout the engine cannot work with.
    ///
    /// `commit` calls this itself, and `flow::prepare` calls it so a plan is
    /// refused before it is shown rather than when the user presses apply.
    pub fn check_layout(&self) -> Result<(), EditError> {
        // Compare canonically when the paths exist, so a symlinked `~/.config`
        // cannot smuggle the state directory into the configuration tree.
        let state = self.canonical_prefix(&self.layout.state_dir);
        let root = self.canonical_prefix(&self.layout.config_root);
        if state.starts_with(&root) {
            return Err(EditError::StateInsideConfig {
                state_dir: self.layout.state_dir.clone(),
                config_root: self.layout.config_root.clone(),
            });
        }
        Ok(())
    }

    /// Canonicalize the parent directory and re-append the final component.
    ///
    /// Used to decide whether a *request* is inside the configuration tree: the
    /// final component may legitimately be a symlink pointing elsewhere, but its
    /// own location must be inside the tree.
    fn canonical_parent_joined(&self, path: &Path) -> PathBuf {
        let normalized = normalize_for_compare(path);
        match (normalized.parent(), normalized.file_name()) {
            (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
                self.canonical_prefix(parent).join(name)
            }
            _ => normalized,
        }
    }

    /// Canonicalize the deepest existing ancestor and re-append the rest.
    fn canonical_prefix(&self, path: &Path) -> PathBuf {
        let normalized = normalize_for_compare(path);
        let mut existing = normalized.as_path();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        loop {
            if let Ok(real) = self.fs.canonicalize(existing) {
                let mut out = real;
                for part in tail.iter().rev() {
                    out.push(part);
                }
                return out;
            }
            let Some(parent) = existing.parent() else {
                return normalized;
            };
            if let Some(name) = existing.file_name() {
                tail.push(name.to_os_string());
            }
            existing = parent;
            if existing.as_os_str().is_empty() {
                return normalized;
            }
        }
    }

    fn current_hash(&self, path: &Path) -> Result<Option<Sha256>, EditError> {
        if !self.fs.exists(path) {
            return Ok(None);
        }
        let bytes = self
            .fs
            .read_bytes(path)
            .map_err(|source| EditError::Storage {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Some(Sha256::of_bytes(&bytes)))
    }

    /// True when the file on disk still has the hash the change was staged from.
    fn matches_expected(&self, change: &FileChange) -> Result<bool, EditError> {
        Ok(self.current_hash(&change.path)? == change.expected_sha256)
    }

    fn verify_unchanged(&self, change: &FileChange) -> Result<(), EditError> {
        let actual = self.current_hash(&change.path)?;
        if actual == change.expected_sha256 {
            return Ok(());
        }
        Err(EditError::Conflict {
            path: change.path.clone(),
            expected: change.expected_sha256,
            actual,
        })
    }

    fn verify_parses(change: &FileChange) -> Result<(), EditError> {
        validate::check(&change.path, &change.new_text).map_err(|error| EditError::InvalidNix {
            path: change.path.clone(),
            detail: error.detail,
        })
    }

    /// Put the planned bytes on disk.
    ///
    /// `Create` uses `write_new` directly, because `O_EXCL` is the only way to
    /// create a file without racing somebody else's creation. `Replace` writes a
    /// sibling temporary file and renames it over the target, preserving mode.
    ///
    /// On failure the error says what was left at the target ([`Leftover`]), so
    /// the caller can tell a cleaned-up failure from one that left bytes behind.
    fn land_one(&self, change: &FileChange) -> Result<(), LandingError> {
        let parent = parent_for(&change.path);
        if change.kind == ChangeKind::Create {
            self.fs
                .create_dir_all(parent)
                .map_err(LandingError::clean)?;
            match self.fs.write_new(&change.path, change.new_text.as_bytes()) {
                Ok(()) => {
                    // Durability only; the file exists and is complete.
                    let _ = self.fs.sync_dir(parent);
                    Ok(())
                }
                Err(source) => Err(LandingError {
                    source,
                    leftover: self.classify_leftover(change),
                }),
            }
        } else {
            atomic_replace(self.fs, &change.path, change.new_text.as_bytes())
                .map_err(LandingError::clean)
        }
    }

    /// Decide what a failed `create` left at its target, and whether it is ours.
    ///
    /// A delete is attempted first: if the path is gone, nothing is left behind
    /// and no rollback entry is needed. If it cannot be deleted, the surviving
    /// bytes are read back and compared against the planned text — an
    /// interrupted sequential write leaves a *prefix* of it. Anything else
    /// (unreadable, longer, or different) may belong to another process that
    /// claimed the path, and is never deleted.
    fn classify_leftover(&self, change: &FileChange) -> Leftover {
        match self.fs.remove_file(&change.path) {
            Ok(()) => return Leftover::Nothing,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Leftover::Nothing,
            Err(_) => {}
        }
        let Ok(bytes) = self.fs.read_bytes(&change.path) else {
            return Leftover::Unattributed;
        };
        let planned = change.new_text.as_bytes();
        if bytes.len() <= planned.len() && planned.starts_with(&bytes) {
            Leftover::Ours(Sha256::of_bytes(&bytes))
        } else {
            Leftover::Unattributed
        }
    }

    /// Read the file back and confirm it holds exactly what was planned.
    fn verify_landed(&self, change: &FileChange) -> io::Result<()> {
        let written = self.fs.read_bytes(&change.path)?;
        if written != change.new_text.as_bytes() {
            return Err(io::Error::other("written content does not match the plan"));
        }
        Ok(())
    }
}

/// Why a single file could not be landed, and what it left behind.
struct LandingError {
    source: io::Error,
    /// What the failed operation may have left at the target path.
    leftover: Leftover,
}

impl LandingError {
    /// A failure that provably left the target untouched.
    fn clean(source: io::Error) -> Self {
        Self {
            source,
            leftover: Leftover::Nothing,
        }
    }
}

/// What a failed write left at its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Leftover {
    /// Provably nothing: the target does not exist (any more).
    Nothing,
    /// The target holds exactly these bytes, and they are this plan's own write.
    /// A rollback may delete it, but only while it still holds them.
    Ours(Sha256),
    /// The target exists, but its content cannot be attributed to this plan.
    ///
    /// Another process may have claimed the path between our failed write and
    /// the rollback. Deleting it could destroy their file, so the rollback must
    /// leave it alone and report it instead.
    Unattributed,
}

/// Snapshot ids are used in a file name and must not be able to escape it.
fn validate_snapshot_id(id: &str) -> Result<(), EditError> {
    let acceptable = !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');
    if acceptable {
        Ok(())
    } else {
        Err(EditError::InvalidSnapshot {
            detail: format!("{id:?} is not an acceptable snapshot id"),
        })
    }
}

fn build_change(
    path: &Path,
    resolved_from: Option<PathBuf>,
    kind: ChangeKind,
    expected_sha256: Option<Sha256>,
    old_text: String,
    new_text: &str,
) -> FileChange {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let unified_diff = diff::unified(&old_text, new_text, name);
    let (added_lines, removed_lines) = diff::counts(&old_text, new_text);
    FileChange {
        path: path.to_path_buf(),
        kind,
        expected_sha256,
        old_text,
        new_text: new_text.to_string(),
        unified_diff,
        added_lines,
        removed_lines,
        resolved_from,
    }
}

fn snapshot_change(change: &FileChange) -> SnapshotChange {
    SnapshotChange {
        path: change.path.clone(),
        kind: change.kind.as_str().to_string(),
        old_sha256: change.expected_sha256.map(Sha256::to_hex),
        new_sha256: Sha256::of_bytes(change.new_text.as_bytes()).to_hex(),
        added_lines: change.added_lines,
        removed_lines: change.removed_lines,
        resolved_from: change.resolved_from.clone(),
    }
}

fn snapshot_backup(entry: &BackupEntry) -> SnapshotBackup {
    SnapshotBackup {
        original: entry.original.clone(),
        stored: entry.stored.clone(),
        sha256: entry.sha256.to_hex(),
        existed: entry.existed,
    }
}

/// Replace `path` with `bytes` atomically: write a sibling temporary file, then
/// rename it over the target. The rename is the commit point.
///
/// The target's permission bits are carried over, so a `0600` config file does
/// not become world-readable.
pub(crate) fn atomic_replace(fs: &dyn WriteFs, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = parent_for(path);
    fs.create_dir_all(parent)?;
    let temp = temp_path(path);
    fs.write_new(&temp, bytes)?;
    // A 0600 file must not become 0644 because the copy failed, so this is not
    // best effort. A missing source (a Create) has no mode to carry over.
    fs.copy_mode(path, &temp)?;
    if let Err(error) = fs.rename(&temp, path) {
        let _ = fs.remove_file(&temp);
        return Err(error);
    }
    // The rename is the commit point: the file has been replaced. Durability is
    // a separate concern, and a failed flush must not be reported as "the write
    // did not happen" — the caller would then skip restoring a file that in fact
    // changed.
    let _ = fs.sync_dir(parent);
    Ok(())
}

/// Directory to fsync after replacing `path`.
///
/// A bare file name has an empty `parent()`, and `fsync("")` fails with
/// `ENOENT` — after the rename already succeeded. "." is the correct answer
/// there.
fn parent_for(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// `.<name>.nhs-tmp-<pid>-<counter>-<nonce>` beside the target, so the rename
/// stays on one filesystem and a recycled pid cannot collide with a leftover.
fn temp_path(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.subsec_nanos());
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let temp = format!(".{name}.nhs-tmp-{}-{counter}-{nonce}", std::process::id());
    path.with_file_name(temp)
}

fn new_snapshot_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.subsec_nanos());
    format!(
        "{}-{}-{:06}-{nonce}",
        unix_now(),
        std::process::id(),
        counter
    )
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Lexical comparison helper: remove `.` and resolve `..` without touching disk,
/// so the layout check works for paths that do not exist yet.
fn normalize_for_compare(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::RootDir => out.push("/"),
            std::path::Component::CurDir | std::path::Component::Prefix(_) => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(part) => out.push(part),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)]

    use super::{normalize_for_compare, parent_for, validate_snapshot_id};
    use std::path::Path;

    #[test]
    fn a_bare_file_name_syncs_the_current_directory() {
        assert_eq!(parent_for(Path::new("home.nix")), Path::new("."));
        assert_eq!(parent_for(Path::new("/a/b.nix")), Path::new("/a"));
    }

    #[test]
    fn layout_comparison_resolves_dot_segments() {
        assert_eq!(
            normalize_for_compare(Path::new("/home/u/.config/../.local/state")),
            Path::new("/home/u/.local/state")
        );
    }

    #[test]
    fn snapshot_ids_cannot_escape_a_directory() {
        assert!(validate_snapshot_id("1700000000-1234-000001-42").is_ok());
        assert!(validate_snapshot_id("../../../tmp/evil").is_err());
        assert!(validate_snapshot_id("").is_err());
        assert!(validate_snapshot_id("a/b").is_err());
    }
}
