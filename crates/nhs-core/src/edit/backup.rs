//! Content-addressed backups.
//!
//! Backups live **outside** the configuration tree, under the application state
//! directory, so a backup can never be mistaken for a Nix file or imported by
//! accident. They are named by content hash and are never overwritten: two
//! applies that see the same bytes share one backup, and two different byte
//! strings never collide.

use std::io;
use std::path::{Path, PathBuf};

use super::hash::Sha256;
use crate::fs::WriteFs;

/// Sub-directory of the state directory that holds backups.
const BACKUP_DIR: &str = "backups";

/// How a restore decides whether it may touch a target.
///
/// There is deliberately no "restore regardless" option. Every rollback — of a
/// replaced file *and* of a file this plan created — is guarded by the exact
/// content the plan believes it put there, so a target that somebody else has
/// since claimed is left alone. "It did not exist before my plan" is not enough
/// to justify deleting a path: another process can create that path between our
/// failed write and our rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreGuard {
    /// Restore only when the target still holds this content. `None` means the
    /// target must currently be absent.
    Expect(Option<Sha256>),
}

impl RestoreGuard {
    /// Whether this guard admits what was actually observed at the target.
    ///
    /// An unreadable target is admitted by **no** guard. "I could not read it"
    /// is not evidence of absence: an `Expect(None)` guard must only pass when
    /// the path really is gone, otherwise a failed read would authorise deleting
    /// a file whose bytes nobody verified.
    fn permits(self, observed: Observed) -> bool {
        match (self, observed) {
            (Self::Expect(Some(expected)), Observed::Present(current)) => current == expected,
            (Self::Expect(None), Observed::Absent) => true,
            // Everything else refuses, including `Unreadable`: an unverified
            // file is never overwritten or deleted.
            _ => false,
        }
    }
}

/// What a guarded restore saw at a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Observed {
    /// The path does not exist.
    Absent,
    /// The path exists and holds these bytes.
    Present(Sha256),
    /// The path may exist, but its content could not be read. Nothing can be
    /// concluded about ownership, so the target must not be touched.
    Unreadable,
}

/// Stores pre-write copies of files.
pub struct BackupStore<'a> {
    fs: &'a dyn WriteFs,
    root: PathBuf,
}

/// A stored copy of a file's previous bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupEntry {
    /// The file this backup restores.
    pub original: PathBuf,
    /// Where the copy lives.
    pub stored: PathBuf,
    /// Hash of the bytes in `stored`.
    pub sha256: Sha256,
    /// Whether the original existed. `false` means "restore by deleting".
    pub existed: bool,
}

impl<'a> BackupStore<'a> {
    /// `state_dir` is the application state directory, never a config directory.
    #[must_use]
    pub fn new(fs: &'a dyn WriteFs, state_dir: &Path) -> Self {
        Self {
            fs,
            root: state_dir.join(BACKUP_DIR),
        }
    }

    /// Directory holding every backup.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Save the current bytes of `original`, or record that it did not exist.
    pub fn save(&self, original: &Path) -> io::Result<BackupEntry> {
        if !self.fs.exists(original) {
            return Ok(BackupEntry {
                original: original.to_path_buf(),
                stored: PathBuf::new(),
                sha256: Sha256::of_bytes(b""),
                existed: false,
            });
        }
        let bytes = self.fs.read_bytes(original)?;
        let sha256 = Sha256::of_bytes(&bytes);
        let stored = self.root.join(sha256.to_hex());
        self.fs.create_dir_all(&self.root)?;
        // Content-addressed: an existing copy already holds these exact bytes.
        if !self.fs.exists(&stored) {
            super::atomic_replace(self.fs, &stored, &bytes)?;
            // A backup holds configuration content; it must not be world
            // readable even if the umask would have allowed it.
            self.fs.set_mode(&stored, 0o600)?;
        }
        Ok(BackupEntry {
            original: original.to_path_buf(),
            stored,
            sha256,
            existed: true,
        })
    }

    /// Read a backup back, verifying it still hashes to what was recorded.
    ///
    /// A corrupted backup is reported rather than silently restored.
    pub fn load(&self, entry: &BackupEntry) -> io::Result<Vec<u8>> {
        if !entry.existed {
            return Ok(Vec::new());
        }
        let bytes = self.fs.read_bytes(&entry.stored)?;
        if Sha256::of_bytes(&bytes) != entry.sha256 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("backup is corrupted: {}", entry.stored.display()),
            ));
        }
        Ok(bytes)
    }

    /// Look at what a target currently holds, keeping "absent" and "unreadable"
    /// apart. Collapsing the two would let a failed read pass an
    /// `Expect(None)` guard and authorise a delete nobody verified.
    fn observe(&self, path: &Path) -> Observed {
        if !self.fs.exists(path) {
            return Observed::Absent;
        }
        match self.fs.read_bytes(path) {
            Ok(bytes) => Observed::Present(Sha256::of_bytes(&bytes)),
            Err(_) => Observed::Unreadable,
        }
    }

    /// Restore only when each target still holds the content the caller expects.
    ///
    /// `guards[i]` decides whether `entries[i]` may be restored. A mismatch is a
    /// refusal for that file: somebody changed it after we wrote, and their edit
    /// must win over our rollback. The same applies when the target exists but
    /// cannot be read — an unverifiable file is never overwritten or deleted.
    /// Restores continue for the other files, and the first refusal is reported.
    ///
    /// `entries` must be ordered the same way as `guards`.
    pub fn restore_all_guarded(
        &self,
        entries: &[BackupEntry],
        guards: &[RestoreGuard],
    ) -> io::Result<()> {
        if entries.len() != guards.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "restore needs one guard per entry",
            ));
        }
        let mut failure: Option<io::Error> = None;
        for (entry, guard) in entries.iter().zip(guards.iter()).rev() {
            let observed = self.observe(&entry.original);
            if !guard.permits(observed) {
                let error = io::Error::new(
                    io::ErrorKind::InvalidData,
                    match observed {
                        Observed::Unreadable => format!(
                            "{} exists but could not be read; refusing to overwrite or delete \
                             a file that cannot be verified",
                            entry.original.display()
                        ),
                        Observed::Absent | Observed::Present(_) => format!(
                            "{} changed after this plan wrote it; refusing to restore over it",
                            entry.original.display()
                        ),
                    },
                );
                if failure.is_none() {
                    failure = Some(error);
                }
                continue;
            }
            let result = if entry.existed {
                self.load(entry)
                    .and_then(|bytes| super::atomic_replace(self.fs, &entry.original, &bytes))
            } else {
                // Decide from what was observed, not from a fresh `exists` call:
                // re-probing would reopen the window the guard just closed.
                // `Unreadable` cannot occur here — no guard admits it.
                match observed {
                    Observed::Present(_) => self.fs.remove_file(&entry.original),
                    Observed::Absent | Observed::Unreadable => Ok(()),
                }
            };
            if let Err(error) = result {
                // Keep restoring the rest; report the first failure.
                if failure.is_none() {
                    failure = Some(error);
                }
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
