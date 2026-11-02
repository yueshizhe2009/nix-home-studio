//! Failure-injecting filesystem for the edit-engine tests.
//!
//! Deterministic failure injection is the only reliable way to prove that a
//! partially failed multi-file commit restores what it already wrote, and that
//! files it never reached are left alone. Doing it with real permissions would
//! depend on the user id and on the filesystem.
//!
//! Generic over the inner filesystem so the same faults can be injected into
//! `MemFs` (fast, no I/O) and `RealFs` (honours inodes and permissions), and it
//! can hold several faults at once so one test can describe a compound failure
//! (for example: the first file's directory flush fails *and* the second file's
//! rename fails).

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};

use nhs_core::fs::{FsView, WriteFs};

/// Which operation should fail. Every fault fires once and then disarms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Fail the n-th `write_new` (1-based), counting every call.
    WriteNew(usize),
    /// Fail the n-th `write_new` whose path contains this text (1-based among
    /// matching calls).
    WriteNewInto(&'static str, usize),
    /// Perform `write_new` successfully, then report an error — models a file
    /// that was created and fully written, but whose final `fsync` failed.
    WriteNewAfter(&'static str),
    /// Write a truncated copy of the bytes, then report an error — models
    /// `write_all` failing after the file was created. The partial content stays.
    WriteNewPartial(&'static str),
    /// Fail `rename` when the destination has this file name.
    RenameInto(&'static str),
    /// Fail the n-th `sync_dir` (1-based), counting every call.
    SyncDir(usize),
    /// Fail `sync_dir` for every directory whose path contains this text.
    /// Use this to target the flush of one specific file's directory rather than
    /// whichever flush happens to be first.
    SyncDirInto(&'static str),
    /// Fail `remove_file` for a matching path.
    RemoveInto(&'static str),
    /// Fail `remove_file` for a matching path *after* replacing the file's
    /// content with these bytes. Models the race a rollback has to survive: our
    /// cleanup of a partial write fails, and another process has meanwhile
    /// claimed that path.
    RemoveIntoTakenOver(&'static str, &'static str),
    /// Fail `read_bytes` for a matching path. Models a target that exists but
    /// cannot be inspected, so nothing can be proven about its content.
    ReadBytesInto(&'static str),
}

/// Wraps a filesystem and fails chosen operations.
pub struct Flaky<F> {
    inner: F,
    faults: Vec<Fault>,
    /// One flag per fault, so two faults can be armed for a compound scenario.
    armed: RefCell<Vec<bool>>,
    writes: RefCell<usize>,
    renames: RefCell<usize>,
    syncs: RefCell<usize>,
    matching_writes: RefCell<usize>,
    /// Every filesystem call this wrapper forwarded, in order, as
    /// `"<operation> <path>"`. Lets a test assert how far apart a decision and
    /// the destructive call it authorises are — the window a concurrent writer
    /// could exploit is exactly the number of calls in between.
    ops: RefCell<Vec<String>>,
}

/// Convenience alias for the common in-memory case.
pub type FlakyFs = Flaky<nhs_core::MemFs>;

impl<F> Flaky<F> {
    #[must_use]
    pub fn new(inner: F, fault: Fault) -> Self {
        Self::with_faults(inner, vec![fault])
    }

    #[must_use]
    pub fn with_faults(inner: F, faults: Vec<Fault>) -> Self {
        let armed = vec![true; faults.len()];
        Self {
            inner,
            faults,
            armed: RefCell::new(armed),
            writes: RefCell::new(0),
            renames: RefCell::new(0),
            syncs: RefCell::new(0),
            matching_writes: RefCell::new(0),
            ops: RefCell::new(Vec::new()),
        }
    }

    /// The underlying filesystem, for assertions.
    #[must_use]
    pub fn inner(&self) -> &F {
        &self.inner
    }

    /// Every forwarded operation, in order.
    #[must_use]
    pub fn ops(&self) -> Vec<String> {
        self.ops.borrow().clone()
    }

    fn note(&self, operation: &str, path: &Path) {
        self.ops
            .borrow_mut()
            .push(format!("{operation} {}", path.display()));
    }

    /// Take (and disarm) the first armed fault matching `predicate`.
    fn take(&self, predicate: impl Fn(&Fault) -> bool) -> bool {
        let mut armed = self.armed.borrow_mut();
        for (index, fault) in self.faults.iter().enumerate() {
            let is_armed = armed.get(index).copied().unwrap_or(false);
            if is_armed && predicate(fault) {
                if let Some(flag) = armed.get_mut(index) {
                    *flag = false;
                }
                return true;
            }
        }
        false
    }

    fn path_text(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }
}

impl<F: FsView> FsView for Flaky<F> {
    fn exists(&self, p: &Path) -> bool {
        self.note("exists", p);
        self.inner.exists(p)
    }
    fn is_dir(&self, p: &Path) -> bool {
        self.inner.is_dir(p)
    }
    fn is_symlink(&self, p: &Path) -> bool {
        self.inner.is_symlink(p)
    }
    fn read_to_string(&self, p: &Path) -> io::Result<String> {
        self.note("read_to_string", p);
        self.inner.read_to_string(p)
    }
    fn read_bytes(&self, p: &Path) -> io::Result<Vec<u8>> {
        self.note("read_bytes", p);
        let text = Self::path_text(p);
        if self.take(|fault| matches!(fault, Fault::ReadBytesInto(needle) if text.contains(needle)))
        {
            return Err(io::Error::other("injected read failure"));
        }
        self.inner.read_bytes(p)
    }
    fn read_link(&self, p: &Path) -> io::Result<PathBuf> {
        self.inner.read_link(p)
    }
    fn read_dir(&self, p: &Path) -> io::Result<Vec<PathBuf>> {
        self.inner.read_dir(p)
    }
    fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
        self.note("canonicalize", p);
        self.inner.canonicalize(p)
    }
    fn metadata(&self, p: &Path) -> io::Result<nhs_core::fs::FileMeta> {
        self.note("metadata", p);
        self.inner.metadata(p)
    }
}

impl<F: WriteFs> WriteFs for Flaky<F> {
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.note("create_dir_all", path);
        self.inner.create_dir_all(path)
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.note("write", path);
        self.inner.write(path, bytes)
    }

    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.note("write_new", path);
        *self.writes.borrow_mut() += 1;
        let total = *self.writes.borrow();
        let text = Self::path_text(path);

        let mut into_count = self.matching_writes.borrow_mut();
        let matching_now =
            if self.faults.iter().any(
                |fault| matches!(fault, Fault::WriteNewInto(needle, _) if text.contains(needle)),
            ) {
                *into_count += 1;
                Some(*into_count)
            } else {
                None
            };
        drop(into_count);

        if self.take(|fault| match fault {
            Fault::WriteNew(n) => *n == total,
            Fault::WriteNewInto(needle, occurrence) => {
                text.contains(needle) && Some(*occurrence) == matching_now
            }
            _ => false,
        }) {
            return Err(io::Error::other("injected write failure"));
        }

        if self.take(|fault| matches!(fault, Fault::WriteNewAfter(needle) if text.contains(needle)))
        {
            // The file really is created and complete; only the report fails.
            self.inner.write_new(path, bytes)?;
            return Err(io::Error::other("injected post-create failure"));
        }

        if self
            .take(|fault| matches!(fault, Fault::WriteNewPartial(needle) if text.contains(needle)))
        {
            let cut = bytes.len() / 2;
            self.inner
                .write_new(path, bytes.get(..cut).unwrap_or_default())?;
            return Err(io::Error::other("injected partial write failure"));
        }

        self.inner.write_new(path, bytes)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.note("rename", to);
        *self.renames.borrow_mut() += 1;
        let name = to.file_name().and_then(|value| value.to_str());
        if self.take(|fault| match fault {
            Fault::RenameInto(expected) => name == Some(*expected),
            _ => false,
        }) {
            return Err(io::Error::other("injected rename failure"));
        }
        self.inner.rename(from, to)
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        self.note("remove_dir_all", path);
        self.inner.remove_dir_all(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.note("remove_file", path);
        let text = Self::path_text(path);
        if self.take(
            |fault| matches!(fault, Fault::RemoveIntoTakenOver(needle, _) if text.contains(needle)),
        ) {
            let foreign = self.faults.iter().find_map(|fault| match fault {
                Fault::RemoveIntoTakenOver(needle, foreign) if text.contains(needle) => {
                    Some(*foreign)
                }
                _ => None,
            });
            if let Some(foreign) = foreign {
                self.inner.write(path, foreign.as_bytes())?;
            }
            return Err(io::Error::other("injected remove failure after a takeover"));
        }
        if self.take(|fault| matches!(fault, Fault::RemoveInto(needle) if text.contains(needle))) {
            return Err(io::Error::other("injected remove failure"));
        }
        self.inner.remove_file(path)
    }

    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        self.note("sync_dir", path);
        *self.syncs.borrow_mut() += 1;
        let total = *self.syncs.borrow();
        let text = Self::path_text(path);
        if self.take(|fault| match fault {
            Fault::SyncDir(n) => *n == total,
            Fault::SyncDirInto(needle) => text.contains(needle),
            _ => false,
        }) {
            return Err(io::Error::other("injected sync failure"));
        }
        self.inner.sync_dir(path)
    }

    fn set_mode(&self, path: &Path, mode: u32) -> io::Result<()> {
        self.note("set_mode", path);
        self.inner.set_mode(path, mode)
    }

    fn copy_mode(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.note("copy_mode", to);
        self.inner.copy_mode(from, to)
    }
}
