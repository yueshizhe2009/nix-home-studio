//! Filesystem abstraction.
//!
//! Discovery and toolchain resolution are written against the read-only
//! [`FsView`] so they can run against the real machine ([`RealFs`]) or against an
//! in-memory fixture tree ([`MemFs`]) in tests.
//!
//! M2 adds [`WriteFs`], the only trait through which anything is mutated. It is
//! deliberately separate from [`FsView`]: read-only callers cannot accidentally
//! gain write access, and the orchestration that decides *whether* a write is
//! allowed lives in [`crate::edit`], not here. This module only knows how to
//! move bytes; it contains no policy.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Maximum number of symlink expansions before giving up. Mirrors the Linux
/// kernel's `MAXSYMLINKS`; it keeps [`MemFs::canonicalize`] from looping.
const MAX_SYMLINK_HOPS: usize = 40;

/// What a path is, following symlinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMeta {
    /// Size in bytes.
    pub len: u64,
    /// Permission bits (`0o755`), or `0o644` when the filesystem cannot say.
    pub mode: u32,
    pub is_file: bool,
    pub is_dir: bool,
}

/// Read-only view over a filesystem.
///
/// `exists` reports the presence of a directory entry, including a *dangling*
/// symlink (like `lstat`), while `is_dir`/`canonicalize` follow symlinks.
/// `read_dir` returns the full path of every direct child, sorted.
pub trait FsView {
    fn exists(&self, p: &Path) -> bool;
    fn is_dir(&self, p: &Path) -> bool;
    fn is_symlink(&self, p: &Path) -> bool;
    fn read_to_string(&self, p: &Path) -> io::Result<String>;
    /// Exact bytes, for hashing. Distinct from [`FsView::read_to_string`] so the
    /// integrity guard never depends on UTF-8 validity.
    fn read_bytes(&self, p: &Path) -> io::Result<Vec<u8>>;
    fn read_link(&self, p: &Path) -> io::Result<PathBuf>;
    fn read_dir(&self, p: &Path) -> io::Result<Vec<PathBuf>>;
    fn canonicalize(&self, p: &Path) -> io::Result<PathBuf>;
    /// Metadata of `p`, following symlinks. Used by the scratch copy, which must
    /// know a file's size and mode without ever linking to the original.
    fn metadata(&self, p: &Path) -> io::Result<FileMeta>;
}

/// [`FsView`] backed by the real filesystem.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealFs;

impl FsView for RealFs {
    fn exists(&self, p: &Path) -> bool {
        fs::symlink_metadata(p).is_ok()
    }

    fn is_dir(&self, p: &Path) -> bool {
        fs::metadata(p).is_ok_and(|m| m.is_dir())
    }

    fn is_symlink(&self, p: &Path) -> bool {
        fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
    }

    fn read_to_string(&self, p: &Path) -> io::Result<String> {
        fs::read_to_string(p)
    }

    fn read_bytes(&self, p: &Path) -> io::Result<Vec<u8>> {
        fs::read(p)
    }

    fn read_link(&self, p: &Path) -> io::Result<PathBuf> {
        fs::read_link(p)
    }

    fn read_dir(&self, p: &Path) -> io::Result<Vec<PathBuf>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(p)? {
            entries.push(entry?.path());
        }
        entries.sort();
        Ok(entries)
    }

    fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(p)
    }

    fn metadata(&self, p: &Path) -> io::Result<FileMeta> {
        let metadata = fs::metadata(p)?;
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o777
        };
        #[cfg(not(unix))]
        let mode = 0o644;
        Ok(FileMeta {
            len: metadata.len(),
            mode,
            is_file: metadata.is_file(),
            is_dir: metadata.is_dir(),
        })
    }
}

/// Mutating view over a filesystem.
///
/// Only [`crate::edit`] uses this. The primitives are intentionally low level:
/// policy (hash guards, backups, validation, rollback) is *not* here, so it
/// cannot be bypassed by a new caller.
///
/// [`WriteFs::write_new`] must **fail when the path already exists**; it backs
/// the lock file and the temporary file used for atomic replacement. Implementors
/// must not follow a final symlink when replacing.
pub trait WriteFs: FsView {
    /// Create every missing directory in `path`.
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    /// Write `bytes` to `path`, creating it, or truncating it if it exists.
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// Write `bytes` to `path`, failing with [`io::ErrorKind::AlreadyExists`] if
    /// a directory entry is already there.
    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// Atomically replace `to` with `from`. Both must be on the same filesystem.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Remove a file entry. Fails if it does not exist.
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    /// Remove a directory and everything under it.
    ///
    /// Used only to tear down a scratch copy this application created; a
    /// filesystem that cannot express it must fail rather than silently leave
    /// the tree behind.
    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this filesystem cannot remove a directory tree",
        ))
    }
    /// Flush a directory's entries to stable storage. A no-op in memory.
    fn sync_dir(&self, path: &Path) -> io::Result<()>;
    /// Set permission bits on `path`, when the filesystem can express them.
    fn set_mode(&self, path: &Path, mode: u32) -> io::Result<()> {
        let _ = (path, mode);
        Ok(())
    }

    /// Copy permission bits from `from` to `to` when `from` exists.
    ///
    /// Best effort by contract: a caller replacing a file should preserve its
    /// mode, but a filesystem that cannot express one must not fail the write.
    fn copy_mode(&self, from: &Path, to: &Path) -> io::Result<()> {
        let _ = (from, to);
        Ok(())
    }
}

impl WriteFs for RealFs {
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        fs::write(path, bytes)
    }

    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write as _;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        fs::File::open(path)?.sync_all()
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        fs::remove_dir_all(path)
    }

    fn set_mode(&self, path: &Path, mode: u32) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }

    fn copy_mode(&self, from: &Path, to: &Path) -> io::Result<()> {
        let metadata = match fs::metadata(from) {
            Ok(metadata) => metadata,
            // A new file has no mode to inherit.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        // `Permissions` only carries the mode on Unix; on other platforms this
        // is a no-op and the default mode stands.
        fs::set_permissions(to, metadata.permissions())
    }
}

impl MemFs {
    /// Replace a node wholesale. Used by [`WriteFs`] and by fixtures.
    fn set_node(&self, path: &Path, node: Node) {
        self.insert_node(path, node);
    }
}

impl WriteFs for MemFs {
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        let normalized = normalize(path);
        let mut missing = Vec::new();
        let mut cursor = Some(normalized.clone());
        while let Some(current) = cursor {
            if current.as_os_str().is_empty() {
                break;
            }
            match self.nodes.borrow().get(&current) {
                Some(Node::Dir) => break,
                Some(Node::File(_) | Node::Symlink(_)) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "not a directory",
                    ))
                }
                None => missing.push(current.clone()),
            }
            cursor = current.parent().map(Path::to_path_buf);
        }
        for dir in missing.into_iter().rev() {
            self.set_node(&dir, Node::Dir);
        }
        Ok(())
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let normalized = normalize(path);
        if matches!(self.nodes.borrow().get(&normalized), Some(Node::Dir)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "is a directory",
            ));
        }
        self.set_node(&normalized, Node::File(bytes_to_string(bytes)));
        Ok(())
    }

    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let normalized = normalize(path);
        if self.nodes.borrow().contains_key(&normalized) {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        self.set_node(&normalized, Node::File(bytes_to_string(bytes)));
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let from = normalize(from);
        let to = normalize(to);
        let node = self
            .nodes
            .borrow_mut()
            .remove(&from)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        self.set_node(&to, node);
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        let normalized = normalize(path);
        match self.nodes.borrow_mut().remove(&normalized) {
            Some(Node::Dir) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "is a directory",
            )),
            Some(Node::File(_) | Node::Symlink(_)) => Ok(()),
            None => Err(io::Error::from(io::ErrorKind::NotFound)),
        }
    }

    fn sync_dir(&self, _path: &Path) -> io::Result<()> {
        Ok(())
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        let normalized = normalize(path);
        if !matches!(self.nodes.borrow().get(&normalized), Some(Node::Dir)) {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        self.nodes
            .borrow_mut()
            .retain(|candidate, _| !candidate.starts_with(&normalized));
        Ok(())
    }
}

/// Fixtures store text; bytes that are not UTF-8 cannot be represented. Every
/// file this crate writes is UTF-8 by construction (Nix source and JSON).
fn bytes_to_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    File(String),
    Dir,
    Symlink(PathBuf),
}

/// In-memory fixture filesystem for tests.
///
/// Build a tree with [`MemFs::with_file`], [`MemFs::with_dir`] and
/// [`MemFs::with_symlink`]; missing parent directories are created
/// automatically. Paths are normalized lexically (`//`, `.`, `..`).
#[derive(Debug, Default, Clone)]
pub struct MemFs {
    /// `RefCell` because [`WriteFs`] takes `&self`, like [`FsView`]; fixtures are
    /// shared immutably by the engine under test.
    nodes: RefCell<BTreeMap<PathBuf, Node>>,
}

impl MemFs {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_file(self, path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        self.insert_node(&path.into(), Node::File(contents.into()));
        self
    }

    #[must_use]
    pub fn with_dir(self, path: impl Into<PathBuf>) -> Self {
        self.insert_node(&path.into(), Node::Dir);
        self
    }

    #[must_use]
    pub fn with_symlink(self, path: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        self.insert_node(&path.into(), Node::Symlink(target.into()));
        self
    }

    fn insert_node(&self, path: &std::path::Path, node: Node) {
        let normalized = normalize(path);
        let mut missing = Vec::new();
        let mut cursor = normalized.parent().map(Path::to_path_buf);
        while let Some(parent) = cursor {
            if parent.as_os_str().is_empty() {
                break;
            }
            if !self.nodes.borrow().contains_key(&parent) {
                missing.push(parent.clone());
            }
            cursor = parent.parent().map(Path::to_path_buf);
        }
        let mut nodes = self.nodes.borrow_mut();
        for dir in missing.into_iter().rev() {
            nodes.insert(dir, Node::Dir);
        }
        nodes.insert(normalized, node);
    }

    /// Resolve `p` component by component, following symlinks (including
    /// intermediate ones). Returns the normalized absolute path if every
    /// component exists.
    fn resolve(&self, p: &Path) -> Option<PathBuf> {
        let normalized = normalize(p);
        if normalized.as_os_str().is_empty() {
            return None;
        }
        let mut parts: VecDeque<PathBuf> = normalized.components().map(component_to_path).collect();
        let mut current = PathBuf::new();
        let mut hops = 0_usize;

        while let Some(part) = parts.pop_front() {
            let part_str = part.as_os_str();
            if part_str == "/" {
                current = PathBuf::from("/");
                continue;
            }
            if part_str.is_empty() {
                continue;
            }
            if part_str == ".." {
                current.pop();
                continue;
            }

            let candidate = current.join(&part);
            let nodes = self.nodes.borrow();
            let node = nodes.get(&candidate);
            match node {
                Some(Node::Symlink(target)) => {
                    hops = hops.saturating_add(1);
                    if hops > MAX_SYMLINK_HOPS {
                        return None;
                    }
                    let base = if target.is_absolute() {
                        PathBuf::new()
                    } else {
                        current.clone()
                    };
                    let combined = normalize(&base.join(target));
                    let mut rebuilt: VecDeque<PathBuf> =
                        combined.components().map(component_to_path).collect();
                    rebuilt.extend(parts);
                    parts = rebuilt;
                    current = PathBuf::new();
                }
                Some(Node::File(_) | Node::Dir) => current = candidate,
                None => {
                    if candidate.as_os_str() == "/" {
                        current = candidate;
                    } else {
                        return None;
                    }
                }
            }
        }

        if current.as_os_str() == "/" || self.nodes.borrow().contains_key(&current) {
            Some(current)
        } else {
            None
        }
    }
}

impl FsView for MemFs {
    fn exists(&self, p: &Path) -> bool {
        self.nodes.borrow().contains_key(&normalize(p)) || self.resolve(p).is_some()
    }

    fn is_dir(&self, p: &Path) -> bool {
        match self.resolve(p) {
            Some(path) => {
                path.as_os_str() == "/" || matches!(self.nodes.borrow().get(&path), Some(Node::Dir))
            }
            None => false,
        }
    }

    fn is_symlink(&self, p: &Path) -> bool {
        matches!(
            self.nodes.borrow().get(&normalize(p)),
            Some(Node::Symlink(_))
        )
    }

    fn metadata(&self, p: &Path) -> io::Result<FileMeta> {
        let resolved = self
            .resolve(p)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        match self.nodes.borrow().get(&resolved) {
            Some(Node::File(contents)) => Ok(FileMeta {
                len: u64::try_from(contents.len()).unwrap_or(u64::MAX),
                mode: 0o644,
                is_file: true,
                is_dir: false,
            }),
            Some(Node::Dir) => Ok(FileMeta {
                len: 0,
                mode: 0o755,
                is_file: false,
                is_dir: true,
            }),
            Some(Node::Symlink(_)) | None => Err(io::Error::from(io::ErrorKind::NotFound)),
        }
    }

    fn read_to_string(&self, p: &Path) -> io::Result<String> {
        let resolved = self
            .resolve(p)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        match self.nodes.borrow().get(&resolved) {
            Some(Node::File(contents)) => Ok(contents.clone()),
            Some(Node::Dir) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "is a directory",
            )),
            Some(Node::Symlink(_)) | None => Err(io::Error::from(io::ErrorKind::NotFound)),
        }
    }

    fn read_bytes(&self, p: &Path) -> io::Result<Vec<u8>> {
        Ok(self.read_to_string(p)?.into_bytes())
    }

    fn read_link(&self, p: &Path) -> io::Result<PathBuf> {
        match self.nodes.borrow().get(&normalize(p)) {
            Some(Node::Symlink(target)) => Ok(target.clone()),
            Some(Node::File(_) | Node::Dir) => {
                Err(io::Error::new(io::ErrorKind::InvalidInput, "not a symlink"))
            }
            None => Err(io::Error::from(io::ErrorKind::NotFound)),
        }
    }

    fn read_dir(&self, p: &Path) -> io::Result<Vec<PathBuf>> {
        let resolved = self
            .resolve(p)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        if resolved.as_os_str() != "/"
            && !matches!(self.nodes.borrow().get(&resolved), Some(Node::Dir))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a directory",
            ));
        }
        let mut entries = Vec::new();
        for path in self.nodes.borrow().keys() {
            if path
                .parent()
                .is_some_and(|parent| parent == resolved.as_path())
            {
                entries.push(path.clone());
            }
        }
        entries.sort();
        Ok(entries)
    }

    fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
        self.resolve(p)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
}

fn component_to_path(component: Component<'_>) -> PathBuf {
    match component {
        Component::RootDir => PathBuf::from("/"),
        Component::CurDir | Component::Prefix(_) => PathBuf::new(),
        Component::ParentDir => PathBuf::from(".."),
        Component::Normal(part) => PathBuf::from(part),
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => out.push("/"),
            Component::CurDir | Component::Prefix(_) => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

    use super::{FsView, MemFs};
    use std::path::{Path, PathBuf};

    fn fs() -> MemFs {
        MemFs::new()
            .with_file("/home/tester/flake.nix", "{}")
            .with_symlink("/home/tester/current", "/home/tester/target")
            .with_file("/home/tester/target/home.nix", "{ }")
            .with_symlink("/home/tester/dangling", "/nix/store/missing")
    }

    #[test]
    fn read_dir_returns_sorted_full_paths() {
        let fs = fs();
        let Ok(entries) = fs.read_dir(Path::new("/home/tester")) else {
            panic!("read_dir failed");
        };
        let mut expected = vec![
            PathBuf::from("/home/tester/current"),
            PathBuf::from("/home/tester/dangling"),
            PathBuf::from("/home/tester/flake.nix"),
            PathBuf::from("/home/tester/target"),
        ];
        expected.sort();
        assert_eq!(entries, expected);
    }

    #[test]
    fn canonicalize_follows_intermediate_symlinks() {
        let fs = fs();
        let Ok(resolved) = fs.canonicalize(Path::new("/home/tester/current/home.nix")) else {
            panic!("canonicalize failed");
        };
        assert_eq!(resolved, PathBuf::from("/home/tester/target/home.nix"));
    }

    #[test]
    fn dangling_symlink_exists_but_does_not_canonicalize() {
        let fs = fs();
        assert!(fs.exists(Path::new("/home/tester/dangling")));
        assert!(fs.is_symlink(Path::new("/home/tester/dangling")));
        assert!(!fs.is_dir(Path::new("/home/tester/dangling")));
        assert!(fs.canonicalize(Path::new("/home/tester/dangling")).is_err());
    }

    #[test]
    fn read_link_returns_the_stored_target() {
        let fs = fs();
        let Ok(target) = fs.read_link(Path::new("/home/tester/current")) else {
            panic!("read_link failed");
        };
        assert_eq!(target, PathBuf::from("/home/tester/target"));
        assert!(fs.read_link(Path::new("/home/tester/flake.nix")).is_err());
    }
}
