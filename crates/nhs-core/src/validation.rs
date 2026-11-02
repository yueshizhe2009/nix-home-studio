//! Validating a plan in a scratch copy, never in the live tree.
//!
//! The contract (architecture §7.1, M4 decisions):
//!
//! 1. the plan is applied to a **copy** of the configuration tree;
//! 2. the copy is materialised — every symlink is dereferenced — so nothing
//!    inside it can write through to a real file;
//! 3. the build runs with the copy as its working directory;
//! 4. a successful run removes the copy, a failed run keeps it and returns the
//!    path, because the failure is the thing worth inspecting;
//! 5. a failed attempt is recorded in the state directory, so "we tried and it
//!    failed" survives the terminal scrollback.
//!
//! Everything here goes through [`WriteFs`], never `std::fs`. That is not
//! ceremony: it keeps gate G3 ("raw writes only in `fs.rs`") meaningful, and it
//! lets the same code run against `MemFs` in a unit test.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::classify::Diagnosis;
use crate::discovery::Layout;
use crate::edit::{EditEngine, Plan, StateLayout};
use crate::fs::WriteFs;
use crate::proc::{CancelToken, CommandSpec, Outcome, Program, RunError, Runner, Stream};

/// Maximum number of files a scratch copy may contain.
pub const MAX_FILES: usize = 20_000;
/// Maximum total size of a scratch copy.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Format version of [`ValidationRecord`].
pub const VALIDATION_FORMAT_VERSION: u32 = 1;

/// Bounds on a scratch copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_files: usize,
    pub max_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_files: MAX_FILES,
            max_bytes: MAX_BYTES,
        }
    }
}

/// What a validation produced.
#[derive(Debug, Clone)]
pub struct ValidationReport {
    /// True when the build would succeed.
    pub success: bool,
    /// The command that ran, as it would be shown to the user.
    pub command: String,
    /// Set when the run failed and can be explained.
    pub diagnosis: Option<Diagnosis>,
    /// Where the copy was kept. `Some` only for a failed run.
    pub kept_scratch: Option<PathBuf>,
    /// The record written for a failed attempt.
    pub record: Option<PathBuf>,
    /// Wall-clock time of the build.
    pub elapsed: Duration,
}

/// Why validation could not be carried out at all.
#[derive(Debug)]
pub enum ValidationError {
    /// This layout has no build command in v1 (`ProfileOnly`, `Unknown`) or is
    /// read-only (`FlakeNixos`).
    UnsupportedLayout { kind: String, reason: String },
    /// The copy would exceed the size or file-count limits.
    TooLarge {
        files: usize,
        bytes: u64,
        limits: Limits,
    },
    /// A file or directory could not be copied.
    Copy {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A symlink points back at one of its own parents.
    SymlinkCycle { path: PathBuf },
    /// A planned file is outside the configuration tree and not reachable
    /// through a symlink recorded at stage time.
    OutsideRoot { path: PathBuf },
    /// Applying the plan to the copy failed. This is a bug or a concurrent edit,
    /// never a build problem.
    Apply { detail: String },
    /// The attempt record could not be written.
    Record { source: std::io::Error },
    /// The runner could not start or observe the build.
    Run(RunError),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedLayout { kind, reason } => {
                write!(f, "cannot validate a `{kind}` configuration: {reason}")
            }
            Self::TooLarge {
                files,
                bytes,
                limits,
            } => write!(
                f,
                "the configuration is too large to copy ({files} files, {bytes} bytes; the limit \
                 is {} files and {} bytes)",
                limits.max_files, limits.max_bytes
            ),
            Self::Copy { path, source } => {
                write!(f, "could not copy {}: {source}", path.display())
            }
            Self::SymlinkCycle { path } => write!(
                f,
                "the symlink at {} points back at a directory that contains it; refusing to copy",
                path.display()
            ),
            Self::OutsideRoot { path } => write!(
                f,
                "{} is outside the configuration tree and was not reached through a symlink, so \
                 it cannot be validated",
                path.display()
            ),
            Self::Apply { detail } => write!(
                f,
                "the plan could not be applied to the scratch copy ({detail}); nothing was \
                 validated and nothing was written"
            ),
            Self::Record { source } => write!(f, "the attempt could not be recorded: {source}"),
            Self::Run(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ValidationError {}

/// Record of one failed validation, kept in the state directory.
///
/// This is **not** an edit snapshot: it records a build that never touched the
/// configuration, so it is never offered as something to revert to.
#[derive(Debug, Clone, Serialize)]
pub struct ValidationRecord {
    pub id: String,
    pub format_version: u32,
    pub created_unix_secs: u64,
    /// The command that ran.
    pub command: String,
    /// The scratch directory that was kept for inspection.
    pub scratch: PathBuf,
    /// Machine-readable kind from the classifier.
    pub kind: Option<String>,
    pub summary: String,
    pub hint: Option<String>,
    pub location: Option<String>,
    /// Files the plan would have changed.
    pub planned: Vec<PathBuf>,
}

/// Validate `plan` against a scratch copy of the layout's configuration tree.
///
/// `state_dir` holds the scratch tree, its engine state and the failure record.
/// The live tree is only ever read.
pub fn validate(
    fs: &dyn WriteFs,
    layout: &Layout,
    plan: &Plan,
    state_dir: &Path,
    runner: &dyn Runner,
    cancel: &CancelToken,
    sink: &mut dyn FnMut(Stream, &str),
) -> Result<ValidationReport, ValidationError> {
    validate_with_limits(
        fs,
        layout,
        plan,
        state_dir,
        runner,
        cancel,
        sink,
        Limits::default(),
    )
}

/// [`validate`] with explicit limits, for tests and for a future setting.
#[allow(clippy::too_many_arguments)]
pub fn validate_with_limits(
    fs: &dyn WriteFs,
    layout: &Layout,
    plan: &Plan,
    state_dir: &Path,
    runner: &dyn Runner,
    cancel: &CancelToken,
    sink: &mut dyn FnMut(Stream, &str),
    limits: Limits,
) -> Result<ValidationReport, ValidationError> {
    let (real_root, build) = build_command(layout)?;
    let id = new_attempt_id();
    let scratch = state_dir.join("scratch").join(&id);
    let copy_root = scratch.join("root");
    fs.create_dir_all(&copy_root)
        .map_err(|source| ValidationError::Copy {
            path: copy_root.clone(),
            source,
        })?;

    // 1. Copy the tree, dereferencing symlinks.
    let mut stats = Stats::default();
    let mut stack = Vec::new();
    if let Err(error) = copy_tree(
        fs, &real_root, &copy_root, &limits, &mut stats, &mut stack, 0,
    ) {
        let _ = fs.remove_dir_all(&scratch);
        return Err(error);
    }

    // 2. Apply the plan to the copy through the same engine that will apply it
    //    for real: what is built is exactly what would be written.
    let translated = translate_plan(&real_root, &copy_root, plan)?;
    let engine = EditEngine::new(
        fs,
        StateLayout {
            state_dir: scratch.join("state"),
            config_root: copy_root.clone(),
        },
    );
    if let Err(error) = engine.commit(&translated) {
        let _ = fs.remove_dir_all(&scratch);
        return Err(ValidationError::Apply {
            detail: error.to_string(),
        });
    }

    // 3. Run the build inside the copy.
    let command = build.in_copy(&copy_root);
    let started = Instant::now();
    let outcome = runner
        .run(&command, cancel, sink)
        .map_err(ValidationError::Run)?;
    let elapsed = started.elapsed();
    let diagnosis = crate::classify::diagnose(&outcome);

    // 4. Teardown: success removes the copy, failure keeps it.
    let (kept_scratch, record) = if outcome.success() {
        let _ = fs.remove_dir_all(&scratch);
        (None, None)
    } else {
        let record = write_record(
            fs,
            state_dir,
            &id,
            &command,
            &scratch,
            &outcome,
            diagnosis.as_ref(),
            plan,
        )?;
        (Some(scratch), Some(record))
    };

    Ok(ValidationReport {
        success: outcome.success(),
        command: command.display(),
        diagnosis,
        kept_scratch,
        record,
        elapsed,
    })
}

/// One piece of a command line.
#[derive(Debug, Clone)]
enum Arg {
    /// Passed through unchanged.
    Literal(String),
    /// A path inside the configuration tree, rewritten to the copy.
    PathInRoot(PathBuf),
}

/// A build command before it is pointed at a scratch directory.
#[derive(Debug, Clone)]
struct Build {
    program: Program,
    args: Vec<Arg>,
    timeout: Duration,
}

impl Build {
    /// Point the command at `copy_root` and build the spec.
    fn in_copy(&self, copy_root: &Path) -> CommandSpec {
        let mut spec = CommandSpec::new(self.program, copy_root).with_timeout(self.timeout);
        for arg in &self.args {
            match arg {
                Arg::Literal(text) => spec = spec.arg(text),
                Arg::PathInRoot(relative) => spec = spec.arg(copy_root.join(relative)),
            }
        }
        spec
    }
}

/// The command for a layout, and the configuration root it reads.
fn build_command(layout: &Layout) -> Result<(PathBuf, Build), ValidationError> {
    match layout {
        Layout::FlakeHome { root, attr, .. } => Ok((
            root.clone(),
            Build {
                program: Program::Nix,
                // `--dry-run` evaluates and plans without building; the
                // structured log format is what the classifier prefers.
                args: vec![
                    Arg::Literal("build".to_string()),
                    Arg::Literal("--no-link".to_string()),
                    Arg::Literal("--dry-run".to_string()),
                    Arg::Literal("--log-format".to_string()),
                    Arg::Literal("internal-json".to_string()),
                    Arg::Literal("-v".to_string()),
                    Arg::Literal(format!(
                        ".{}#homeConfigurations.{attr}.activationPackage",
                        ""
                    )),
                ],
                timeout: Duration::from_mins(15),
            },
        )),
        Layout::ClassicHome { main_file } => {
            let root = main_file
                .parent()
                .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
            let relative = main_file
                .strip_prefix(&root)
                .unwrap_or(main_file)
                .to_path_buf();
            Ok((
                root,
                Build {
                    program: Program::HomeManager,
                    // `home-manager build` does not forward a log format, so the
                    // plain one is used here; structured output is only reliable
                    // for `nix build`.
                    args: vec![
                        Arg::Literal("build".to_string()),
                        Arg::Literal("-f".to_string()),
                        Arg::PathInRoot(relative),
                    ],
                    timeout: Duration::from_mins(15),
                },
            ))
        }
        Layout::FlakeNixos { .. } => Err(ValidationError::UnsupportedLayout {
            kind: "flake-nixos".to_string(),
            reason: "NixOS integration is read-only in this version".to_string(),
        }),
        Layout::ProfileOnly { .. } => Err(ValidationError::UnsupportedLayout {
            kind: "profile-only".to_string(),
            reason: "there is no configuration file to build".to_string(),
        }),
        Layout::Unknown { reason } => Err(ValidationError::UnsupportedLayout {
            kind: "unknown".to_string(),
            reason: reason.clone(),
        }),
    }
}

/// The build command for a layout, as text, without running it.
///
/// Used by the CLI to show what `validate` would do and by the tests that pin
/// "build and switch are different commands".
pub fn build_preview(layout: &Layout) -> Result<String, ValidationError> {
    let (root, build) = build_command(layout)?;
    Ok(build.in_copy(&root).display())
}

/// Map a plan's real paths onto the copy.
///
/// A change staged through a symlink records both the link and its target; in the
/// copy the link is a real file, so the *link's* relative path is the right one.
fn translate_plan(
    real_root: &Path,
    copy_root: &Path,
    plan: &Plan,
) -> Result<Plan, ValidationError> {
    let mut changes = Vec::with_capacity(plan.changes.len());
    for change in &plan.changes {
        let mut translated = change.clone();
        let relative = translated
            .resolved_from
            .as_ref()
            .and_then(|link| link.strip_prefix(real_root).ok())
            .or_else(|| translated.path.strip_prefix(real_root).ok())
            .ok_or_else(|| ValidationError::OutsideRoot {
                path: translated.path.clone(),
            })?;
        translated.path = copy_root.join(relative);
        translated.resolved_from = None;
        changes.push(translated);
    }
    Ok(Plan {
        changes,
        notes: plan.notes.clone(),
    })
}

#[derive(Default)]
struct Stats {
    files: usize,
    bytes: u64,
}

/// Copy `src` into `dest`, following symlinks so the copy is self-contained.
fn copy_tree(
    fs: &dyn WriteFs,
    src: &Path,
    dest: &Path,
    limits: &Limits,
    stats: &mut Stats,
    stack: &mut Vec<PathBuf>,
    depth: usize,
) -> Result<(), ValidationError> {
    const MAX_DEPTH: usize = 64;
    if depth > MAX_DEPTH {
        return Err(ValidationError::SymlinkCycle {
            path: src.to_path_buf(),
        });
    }
    let entries = fs.read_dir(src).map_err(|source| ValidationError::Copy {
        path: src.to_path_buf(),
        source,
    })?;
    for entry in entries {
        if entry.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        let name = entry.file_name().ok_or_else(|| ValidationError::Copy {
            path: entry.clone(),
            source: std::io::Error::from(std::io::ErrorKind::InvalidInput),
        })?;
        let target = dest.join(name);
        let metadata = fs
            .metadata(&entry)
            .map_err(|source| ValidationError::Copy {
                path: entry.clone(),
                source,
            })?;
        if metadata.is_dir {
            let canonical = fs
                .canonicalize(&entry)
                .map_err(|source| ValidationError::Copy {
                    path: entry.clone(),
                    source,
                })?;
            if stack.contains(&canonical) {
                return Err(ValidationError::SymlinkCycle { path: entry });
            }
            fs.create_dir_all(&target)
                .map_err(|source| ValidationError::Copy {
                    path: target.clone(),
                    source,
                })?;
            stack.push(canonical);
            copy_tree(fs, &entry, &target, limits, stats, stack, depth + 1)?;
            stack.pop();
        } else if metadata.is_file {
            stats.files += 1;
            stats.bytes = stats.bytes.saturating_add(metadata.len);
            if stats.files > limits.max_files || stats.bytes > limits.max_bytes {
                return Err(ValidationError::TooLarge {
                    files: stats.files,
                    bytes: stats.bytes,
                    limits: *limits,
                });
            }
            let bytes = fs
                .read_bytes(&entry)
                .map_err(|source| ValidationError::Copy {
                    path: entry.clone(),
                    source,
                })?;
            fs.write_new(&target, &bytes)
                .map_err(|source| ValidationError::Copy {
                    path: target.clone(),
                    source,
                })?;
            let _ = fs.set_mode(&target, metadata.mode);
        } else {
            // A socket, fifo or device has no meaning in a configuration tree.
            return Err(ValidationError::Copy {
                path: entry,
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "not a regular file or directory",
                ),
            });
        }
    }
    Ok(())
}

/// Write the record for a failed attempt.
#[allow(clippy::too_many_arguments)]
fn write_record(
    fs: &dyn WriteFs,
    state_dir: &Path,
    id: &str,
    command: &CommandSpec,
    scratch: &Path,
    outcome: &Outcome,
    diagnosis: Option<&Diagnosis>,
    plan: &Plan,
) -> Result<PathBuf, ValidationError> {
    let dir = state_dir.join("validations");
    fs.create_dir_all(&dir)
        .map_err(|source| ValidationError::Record { source })?;
    let record = ValidationRecord {
        id: id.to_string(),
        format_version: VALIDATION_FORMAT_VERSION,
        created_unix_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs()),
        command: command.display(),
        scratch: scratch.to_path_buf(),
        kind: diagnosis.map(|d| d.kind.label().to_string()),
        summary: diagnosis.as_ref().map_or_else(
            || format!("the build failed with {}", outcome.status),
            |d| d.summary.clone(),
        ),
        hint: diagnosis.as_ref().map(|d| d.hint.to_string()),
        location: diagnosis.as_ref().and_then(|d| d.location.clone()),
        planned: plan
            .changes
            .iter()
            .map(|change| change.path.clone())
            .collect(),
    };
    let path = dir.join(format!("{id}.json"));
    let bytes = serde_json::to_vec_pretty(&record).map_err(|error| ValidationError::Record {
        source: std::io::Error::other(error.to_string()),
    })?;
    fs.write_new(&path, &bytes)
        .map_err(|source| ValidationError::Record { source })?;
    Ok(path)
}

fn new_attempt_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("v{nanos:x}-{counter}-{}", std::process::id())
}
