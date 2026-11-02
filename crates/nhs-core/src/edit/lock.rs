//! Single-instance edit lock.
//!
//! Two instances applying plans at the same time would interleave reads and
//! writes and defeat the hash guard, so a lock is taken for the whole commit.
//!
//! A lock is created with `create_new`, which is atomic. A lock left behind by a
//! crash is **reported, never silently deleted** — deleting somebody's lock is
//! how two writers end up running at once. The message names the file so the
//! user can inspect and remove it deliberately.

use std::io;
use std::path::{Path, PathBuf};

use crate::fs::WriteFs;

/// File name inside the state directory.
const LOCK_FILE: &str = "edit.lock";

/// Held for the duration of one commit; removes the lock on drop.
pub struct EditLock<'a> {
    fs: &'a dyn WriteFs,
    path: PathBuf,
    /// Random token written into the lock, so release can prove the lock is
    /// still ours. Without it, a lock deleted by hand and re-taken by another
    /// instance would be removed by the first instance's `Drop`.
    token: String,
    released: bool,
}

impl<'a> EditLock<'a> {
    /// Take the lock, or explain who holds it.
    ///
    /// The body written into the file is informational only: it records the
    /// process id so a human can tell a live holder from a stale one.
    pub fn acquire(fs: &'a dyn WriteFs, state_dir: &Path) -> Result<Self, Locked> {
        fs.create_dir_all(state_dir).map_err(|error| Locked {
            path: state_dir.join(LOCK_FILE),
            holder: None,
            source: error,
        })?;
        // State holds backups of configuration; keep it private to the user.
        let _ = fs.set_mode(state_dir, 0o700);
        let path = state_dir.join(LOCK_FILE);
        let token = new_token();
        let body = format!("pid={}\ntoken={token}\n", std::process::id());
        match fs.write_new(&path, body.as_bytes()) {
            Ok(()) => Ok(Self {
                fs,
                path,
                token,
                released: false,
            }),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let holder = fs.read_to_string(&path).ok();
                Err(Locked {
                    path,
                    holder,
                    source: error,
                })
            }
            Err(source) => Err(Locked {
                path,
                holder: None,
                source,
            }),
        }
    }

    /// Path of the lock file, for reporting.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for EditLock<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditLock")
            .field("path", &self.path)
            .field("released", &self.released)
            .finish()
    }
}

impl Drop for EditLock<'_> {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        // Only remove the lock if it is still ours.
        let ours = self
            .fs
            .read_to_string(&self.path)
            .is_ok_and(|body| body.contains(&self.token));
        if ours {
            // Best effort: a failed removal is not actionable at drop time, and
            // the next commit will report the stale lock explicitly.
            let _ = self.fs.remove_file(&self.path);
        }
    }
}

/// A token that is unique per lock acquisition within this process and across
/// processes started in the same nanosecond.
fn new_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.subsec_nanos());
    format!("{}-{counter}-{nanos}", std::process::id())
}

/// Why the lock could not be taken.
#[derive(Debug)]
pub struct Locked {
    pub path: PathBuf,
    /// Contents of the existing lock file, when readable.
    pub holder: Option<String>,
    pub source: io::Error,
}

impl std::fmt::Display for Locked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "another edit is in progress (lock: {})",
            self.path.display()
        )?;
        if let Some(holder) = self.holder.as_deref() {
            let holder = holder.trim();
            if !holder.is_empty() {
                write!(f, "; lock says: {holder}")?;
            }
        }
        write!(
            f,
            ". If no other instance is running, inspect and remove that file."
        )
    }
}

impl std::error::Error for Locked {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}
