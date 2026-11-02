//! Snapshot records.
//!
//! A snapshot is written **before** the risky step and updated **after** it, so
//! a crash mid-commit leaves evidence of an unfinished attempt instead of a
//! silently half-applied configuration. NixBox records its journal *after* the
//! writes (`engine.rs:288-297`), which is exactly the ordering that loses
//! evidence; this module deliberately inverts it (audit risk R2/§8.1).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::hash::Sha256;
use crate::fs::WriteFs;

/// Bumped whenever the on-disk shape changes. Read back and checked on load.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;

const SNAPSHOT_DIR: &str = "snapshots";

/// How an apply attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// Files are being (or were being) written; the record is incomplete.
    Pending,
    /// Every file was written and verified.
    Applied,
    /// Something failed; `error` explains it. Files may have been restored.
    Failed,
}

/// One file the attempt touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotChange {
    pub path: PathBuf,
    /// `create` or `replace`.
    pub kind: String,
    pub old_sha256: Option<String>,
    pub new_sha256: String,
    pub added_lines: usize,
    pub removed_lines: usize,
    /// The path originally requested when it was a symlink inside the config
    /// tree. Recorded so a later revert can prove the out-of-tree target was
    /// reached through a link the user owns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_from: Option<PathBuf>,
}

/// A stored pre-write copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotBackup {
    pub original: PathBuf,
    pub stored: PathBuf,
    pub sha256: String,
    pub existed: bool,
}

/// Persisted record of one apply attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub format_version: u32,
    pub id: String,
    pub created_unix_secs: u64,
    pub outcome: Outcome,
    pub changes: Vec<SnapshotChange>,
    pub backups: Vec<SnapshotBackup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Snapshot {
    /// A pending record; `outcome` is set later by the caller.
    #[must_use]
    pub fn pending(id: String, created_unix_secs: u64) -> Self {
        Self {
            format_version: SNAPSHOT_FORMAT_VERSION,
            id,
            created_unix_secs,
            outcome: Outcome::Pending,
            changes: Vec::new(),
            backups: Vec::new(),
            error: None,
        }
    }
}

/// Reads and writes snapshots under the state directory.
pub struct SnapshotStore<'a> {
    fs: &'a dyn WriteFs,
    root: PathBuf,
}

impl<'a> SnapshotStore<'a> {
    #[must_use]
    pub fn new(fs: &'a dyn WriteFs, state_dir: &Path) -> Self {
        Self {
            fs,
            root: state_dir.join(SNAPSHOT_DIR),
        }
    }

    /// Directory holding every snapshot.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path a snapshot with `id` is stored at.
    #[must_use]
    pub fn path_for(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    /// Create a snapshot, failing if a record with this id already exists.
    ///
    /// Used for the initial `Pending` record. `write_new` is `O_EXCL`, so an id
    /// collision aborts instead of silently destroying an earlier attempt's
    /// evidence — a rename would have overwritten it.
    pub fn save_new(&self, snapshot: &Snapshot) -> io::Result<PathBuf> {
        self.fs.create_dir_all(&self.root)?;
        let path = self.path_for(&snapshot.id);
        let json = serde_json::to_vec_pretty(snapshot)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.fs.write_new(&path, &json)?;
        self.fs.set_mode(&path, 0o600)?;
        Ok(path)
    }

    /// Write (or overwrite) a snapshot.
    pub fn save(&self, snapshot: &Snapshot) -> io::Result<PathBuf> {
        self.fs.create_dir_all(&self.root)?;
        let path = self.path_for(&snapshot.id);
        let json = serde_json::to_vec_pretty(snapshot)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        // Atomic: a crash mid-write must not leave a truncated snapshot that
        // looks like a valid record.
        super::atomic_replace(self.fs, &path, &json)?;
        self.fs.set_mode(&path, 0o600)?;
        Ok(path)
    }

    /// Read a snapshot back, refusing a record from another format version.
    pub fn load(&self, id: &str) -> io::Result<Snapshot> {
        let path = self.path_for(id);
        let bytes = self.fs.read_bytes(&path)?;
        let snapshot: Snapshot = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if snapshot.format_version != SNAPSHOT_FORMAT_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "snapshot {} has format version {} but this build reads {}",
                    snapshot.id, snapshot.format_version, SNAPSHOT_FORMAT_VERSION
                ),
            ));
        }
        Ok(snapshot)
    }

    /// Most recent snapshot that reached `Applied`, by `created_unix_secs`.
    pub fn last_applied(&self) -> io::Result<Option<Snapshot>> {
        let Ok(entries) = self.fs.read_dir(&self.root) else {
            return Ok(None);
        };
        let mut best: Option<Snapshot> = None;
        for entry in entries {
            let Some(name) = entry.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(id) = name.strip_suffix(".json") else {
                continue;
            };
            let Ok(snapshot) = self.load(id) else {
                continue;
            };
            if snapshot.outcome != Outcome::Applied {
                continue;
            }
            let better = best.as_ref().is_none_or(|current| {
                (snapshot.created_unix_secs, &snapshot.id)
                    > (current.created_unix_secs, &current.id)
            });
            if better {
                best = Some(snapshot);
            }
        }
        Ok(best)
    }
}

/// Convert a recorded hex digest back into a hash.
#[must_use]
pub fn parse_sha256(text: &str) -> Option<Sha256> {
    Sha256::parse_hex(text)
}
