//! The full catalogue: `nix search` streamed into a snapshot, once.
//!
//! The curated catalogue (M5.1) is a few hundred entries that ship with the app.
//! This is the other tier: every attribute in a nixpkgs revision, which costs
//! minutes and hundreds of megabytes. It is opt-in, it goes through the one
//! process runner, and its result is cached so the second search is instant.
//!
//! Three properties matter more than completeness:
//!
//! * **bounded**: the stream is cut off at a byte cap, and the cap is injectable
//!   so a test can prove the cut-off without generating 300 MiB.
//! * **all-or-nothing**: a capped, cancelled, failing or unparseable run writes
//!   no snapshot and removes its temporary file. A later run must never mistake a
//!   half-written catalogue for a complete one (architecture §5.2).
//! * **reproducible or labelled**: the header records the source reference and
//!   whether it was pinned, so a list built from a branch says so.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::edit::hash::Sha256;
use crate::fs::WriteFs;
use crate::proc::{CommandSpec, Program, RunError, Runner, Stream};
use crate::revision::Resolution;

/// Snapshot layout version. Bumping it invalidates every cached snapshot.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;

/// Production cap on the size of `nix search --json` output.
pub const MAX_OUTPUT_BYTES: u64 = 300 * 1024 * 1024;

/// How many entries a snapshot may hold, so a pathological store cannot exhaust
/// memory. Well above a real nixpkgs (tens of thousands).
pub const MAX_ENTRIES: usize = 400_000;

/// One package from the full catalogue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FullEntry {
    /// Attribute path relative to `pkgs`, e.g. `python3Packages.requests`.
    pub attr: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
}

/// The on-disk snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub format_version: u32,
    /// The flake ref the entries came from.
    pub reference: String,
    /// The commit, when the reference names one.
    pub revision: Option<String>,
    /// False when the source is a branch or the unqualified default.
    pub pinned: bool,
    /// `sha256(reference ‖ 0 ‖ format_version)`; the cache key.
    pub revision_hash: String,
    pub created_unix_secs: u64,
    /// Always `true` in a written snapshot; a file without it is rejected.
    pub complete: bool,
    pub entries_count: usize,
    /// Digest over the canonical serialisation of `entries`.
    pub entries_sha256: String,
    pub entries: Vec<FullEntry>,
}

impl Snapshot {
    /// Wrap entries into a complete, digested snapshot.
    pub fn new(resolution: &Resolution, entries: Vec<FullEntry>) -> Result<Self, CatalogError> {
        let entries_sha256 = digest_entries(&entries)?;
        Ok(Self {
            format_version: SNAPSHOT_FORMAT_VERSION,
            reference: resolution.reference.clone(),
            revision: resolution.revision.clone(),
            pinned: resolution.pinned,
            revision_hash: resolution.cache_key(SNAPSHOT_FORMAT_VERSION),
            created_unix_secs: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_secs()),
            complete: true,
            entries_count: entries.len(),
            entries_sha256,
            entries,
        })
    }

    /// Is this snapshot usable for `resolution` here and now?
    ///
    /// Reuse requires **all** of: the known format version, a matching cache key
    /// for the source as resolved today, `complete: true`, a recomputing entry
    /// digest and a matching count (architecture §5.2). Anything else rebuilds.
    pub fn is_usable_for(&self, resolution: &Resolution) -> Result<(), StaleReason> {
        if self.format_version != SNAPSHOT_FORMAT_VERSION {
            return Err(StaleReason::FormatVersion {
                found: self.format_version,
            });
        }
        if self.revision_hash != resolution.cache_key(SNAPSHOT_FORMAT_VERSION) {
            return Err(StaleReason::DifferentSource);
        }
        if !self.complete {
            return Err(StaleReason::Incomplete);
        }
        if self.entries_count != self.entries.len() {
            return Err(StaleReason::CountMismatch {
                recorded: self.entries_count,
                actual: self.entries.len(),
            });
        }
        match digest_entries(&self.entries) {
            Ok(digest) if digest == self.entries_sha256 => Ok(()),
            Ok(_) => Err(StaleReason::DigestMismatch),
            Err(error) => Err(StaleReason::DigestMismatch2 {
                detail: error.to_string(),
            }),
        }
    }
}

/// Why a cached snapshot cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleReason {
    FormatVersion { found: u32 },
    DifferentSource,
    Incomplete,
    CountMismatch { recorded: usize, actual: usize },
    DigestMismatch,
    DigestMismatch2 { detail: String },
}

impl std::fmt::Display for StaleReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FormatVersion { found } => write!(
                f,
                "snapshot format {found} is not {SNAPSHOT_FORMAT_VERSION}"
            ),
            Self::DifferentSource => write!(f, "the snapshot was built from a different source"),
            Self::Incomplete => write!(f, "the snapshot is marked incomplete"),
            Self::CountMismatch { recorded, actual } => {
                write!(f, "the snapshot says {recorded} entries but holds {actual}")
            }
            Self::DigestMismatch | Self::DigestMismatch2 { .. } => {
                write!(f, "the snapshot's entries do not match its digest")
            }
        }
    }
}

/// Why the full catalogue could not be produced.
#[derive(Debug)]
pub enum CatalogError {
    /// The command could not be started or observed.
    Run(RunError),
    /// Cancelled by the caller.
    Cancelled,
    /// Still running at the deadline.
    TimedOut,
    /// Output exceeded the cap.
    TooLarge { bytes: u64, limit: u64 },
    /// More entries than the cap allows.
    TooManyEntries { limit: usize },
    /// The program exited non-zero.
    Failed { status: String, stderr: String },
    /// The output was not the JSON object `nix search` emits.
    Unparseable { detail: String },
    /// The snapshot or a temporary file could not be written.
    Storage {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Run(error) => write!(f, "{error}"),
            Self::Cancelled => write!(f, "the catalogue build was cancelled"),
            Self::TimedOut => write!(f, "the catalogue build ran out of time"),
            Self::TooLarge { bytes, limit } => write!(
                f,
                "`nix search` produced more than the {limit} byte limit (stopped after {bytes}); \
                 the snapshot was not kept"
            ),
            Self::TooManyEntries { limit } => write!(
                f,
                "`nix search` returned more than {limit} entries; the snapshot was not kept"
            ),
            Self::Failed { status, stderr } => {
                write!(f, "`nix search` failed ({status}): {stderr}")
            }
            Self::Unparseable { detail } => {
                write!(f, "`nix search` output could not be read: {detail}")
            }
            Self::Storage { path, source } => {
                write!(f, "cannot use {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for CatalogError {}

/// Bounds on a full catalogue build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_output_bytes: u64,
    pub max_entries: usize,
    pub timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_output_bytes: MAX_OUTPUT_BYTES,
            max_entries: MAX_ENTRIES,
            timeout: Duration::from_mins(30),
        }
    }
}

/// The command that produces a full catalogue.
#[must_use]
pub fn search_command(resolution: &Resolution, cwd: &Path, limits: Limits) -> CommandSpec {
    CommandSpec::new(Program::Nix, cwd)
        // The runner must not retain the whole catalogue either: its own buffer
        // would grow without bound while this module counted the stream.
        .with_max_capture(limits.max_output_bytes)
        .args([
            "search",
            "--json",
            "--extra-experimental-features",
            "nix-command flakes",
            &resolution.reference,
            "^",
        ])
        .with_timeout(limits.timeout)
}

/// Parse one `nix search --json` object into entries.
///
/// `nix search` returns an object keyed by attribute path, with a prefix that
/// names the flake output and the system (`legacyPackages.x86_64-linux.hello`).
/// The prefix is dropped so the attribute is usable as `pkgs.<attr>` in a Home
/// Manager module, which is the only thing this application writes.
pub fn parse_entries(input: &[u8], limits: Limits) -> Result<Vec<FullEntry>, CatalogError> {
    let raw: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(input).map_err(|error| CatalogError::Unparseable {
            detail: error.to_string(),
        })?;
    let mut entries = Vec::with_capacity(raw.len().min(limits.max_entries));
    for (key, value) in &raw {
        let Some(attr) = strip_attr_prefix(key) else {
            continue;
        };
        let object = value.as_object().ok_or_else(|| CatalogError::Unparseable {
            detail: format!("`{key}` is not an object"),
        })?;
        let text = |field: &str| {
            object
                .get(field)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let name = text("pname");
        entries.push(FullEntry {
            name: if name.is_empty() {
                attr.to_string()
            } else {
                name
            },
            attr: attr.to_string(),
            version: text("version"),
            description: text("description"),
        });
        if entries.len() > limits.max_entries {
            return Err(CatalogError::TooManyEntries {
                limit: limits.max_entries,
            });
        }
    }
    entries.sort_by(|left, right| left.attr.cmp(&right.attr));
    entries.dedup_by(|left, right| left.attr == right.attr);
    Ok(entries)
}

/// Drop `legacyPackages.<system>.` or `packages.<system>.` from an attribute.
fn strip_attr_prefix(key: &str) -> Option<&str> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() >= 3 && matches!(parts.first(), Some(&"legacyPackages" | &"packages")) {
        return key
            .split_once('.')
            .and_then(|(_, rest)| rest.split_once('.'))
            .map(|(_, attr)| attr)
            .filter(|attr| !attr.is_empty());
    }
    if parts.len() >= 2 {
        // No known prefix: keep the key minus the system segment when it looks
        // like one (`x86_64-linux`), otherwise keep it whole.
        if parts
            .first()
            .is_some_and(|system| system.contains('-') && !system.contains('.'))
        {
            return key
                .split_once('.')
                .map(|(_, attr)| attr)
                .filter(|attr| !attr.is_empty());
        }
    }
    (!key.is_empty()).then_some(key)
}

/// Build (or reuse) the full catalogue for `resolution`.
#[allow(clippy::too_many_arguments)]
pub fn build(
    fs: &dyn WriteFs,
    runner: &dyn Runner,
    resolution: &Resolution,
    state_dir: &Path,
    cwd: &Path,
    limits: Limits,
    cancel: &crate::proc::CancelToken,
    sink: &mut dyn FnMut(Stream, &str),
) -> Result<Snapshot, CatalogError> {
    if let Some(snapshot) = load_cached(fs, state_dir, resolution) {
        return Ok(snapshot);
    }

    let command = search_command(resolution, cwd, limits);
    let mut collected = Vec::new();
    let mut over_cap: Option<u64> = None;

    let mut sink_with_cap = |stream: Stream, line: &str| {
        sink(stream, line);
        if stream != Stream::Stdout || over_cap.is_some() {
            return;
        }
        let written = u64::try_from(collected.len())
            .unwrap_or(u64::MAX)
            .saturating_add(u64::try_from(line.len()).unwrap_or(u64::MAX))
            .saturating_add(1);
        if written > limits.max_output_bytes {
            // Record the breach. The runner is not interrupted from here, but
            // nothing is appended and the partial output is never stored.
            over_cap = Some(written);
            return;
        }
        collected.extend_from_slice(line.as_bytes());
        collected.push(b'\n');
    };

    let outcome = runner
        .run(&command, cancel, &mut sink_with_cap)
        .map_err(CatalogError::Run)?;

    if outcome.cancelled {
        return Err(CatalogError::Cancelled);
    }
    if outcome.timed_out {
        return Err(CatalogError::TimedOut);
    }
    // Either signal is enough: this module counts what it keeps, and the runner
    // reports that it had to drop bytes from its own retained copy.
    if let Some(bytes) = over_cap {
        return Err(CatalogError::TooLarge {
            bytes,
            limit: limits.max_output_bytes,
        });
    }
    if outcome.truncated {
        return Err(CatalogError::TooLarge {
            bytes: limits.max_output_bytes.saturating_add(1),
            limit: limits.max_output_bytes,
        });
    }
    if !outcome.status.success() {
        return Err(CatalogError::Failed {
            status: format!("{}", outcome.status),
            stderr: crate::donated::nix_log::strip_ansi(&outcome.stderr)
                .lines()
                .last()
                .unwrap_or_default()
                .to_string(),
        });
    }

    let entries = parse_entries(&collected, limits)?;
    let snapshot = Snapshot::new(resolution, entries)?;
    store(fs, state_dir, &snapshot)?;
    Ok(snapshot)
}

/// Where snapshots live.
#[must_use]
pub fn snapshot_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("catalog")
}

/// Where the snapshot for `resolution` lives.
#[must_use]
pub fn snapshot_path(state_dir: &Path, resolution: &Resolution) -> PathBuf {
    snapshot_dir(state_dir).join(format!(
        "{}.json",
        resolution.cache_key(SNAPSHOT_FORMAT_VERSION)
    ))
}

/// Read a cached snapshot, if one is usable right now.
#[must_use]
pub fn load_cached(
    fs: &dyn WriteFs,
    state_dir: &Path,
    resolution: &Resolution,
) -> Option<Snapshot> {
    let path = snapshot_path(state_dir, resolution);
    let bytes = fs.read_bytes(&path).ok()?;
    let snapshot: Snapshot = serde_json::from_slice(&bytes).ok()?;
    snapshot.is_usable_for(resolution).ok()?;
    Some(snapshot)
}

/// Write a snapshot: assemble, digest, then one atomic rename.
fn store(fs: &dyn WriteFs, state_dir: &Path, snapshot: &Snapshot) -> Result<(), CatalogError> {
    let dir = snapshot_dir(state_dir);
    fs.create_dir_all(&dir)
        .map_err(|source| CatalogError::Storage {
            path: dir.clone(),
            source,
        })?;
    let final_path = snapshot_dir(state_dir).join(format!("{}.json", snapshot.revision_hash));
    let temp = snapshots_temp_path(state_dir, &snapshot.revision_hash);
    let bytes = serde_json::to_vec(snapshot).map_err(|error| CatalogError::Unparseable {
        detail: error.to_string(),
    })?;
    fs.write_new(&temp, &bytes)
        .map_err(|source| CatalogError::Storage {
            path: temp.clone(),
            source,
        })?;
    fs.rename(&temp, &final_path)
        .map_err(|source| CatalogError::Storage {
            path: final_path.clone(),
            source,
        })?;
    Ok(())
}

fn snapshots_temp_path(state_dir: &Path, key: &str) -> PathBuf {
    snapshot_dir(state_dir).join(format!(".{key}.tmp"))
}

/// One result from the full catalogue.
#[derive(Debug, Clone)]
pub struct Hit<'a> {
    pub entry: &'a FullEntry,
    /// Ranking tier from the donated scale (0 is the best match).
    pub tier: u8,
}

/// One page of full-catalogue results.
#[derive(Debug, Clone)]
pub struct Page<'a> {
    pub hits: Vec<Hit<'a>>,
    /// 1-based page actually used (clamped).
    pub page: usize,
    pub per_page: usize,
    pub total: usize,
    pub pages: usize,
}

/// Search a snapshot with the same ranking and paging rules as the curated
/// catalogue.
///
/// The ordering key is `(tier, position, name length, name, attr)`: the donated
/// tiers decide relevance, and `attr` is the final tie-break so two pages cannot
/// disagree about where a hit belongs. An empty query browses; a real query drops
/// entries that do not mention it.
#[must_use]
pub fn search<'a>(snapshot: &'a Snapshot, query: &str, page: usize, per_page: usize) -> Page<'a> {
    let term = crate::donated::nix_search::rank::ranking_term(query);
    let per_page = per_page.clamp(1, crate::catalog::MAX_PER_PAGE);

    let mut ranked: Vec<(crate::donated::nix_search::rank::Relevance, &'a FullEntry)> = snapshot
        .entries
        .iter()
        .filter_map(|entry| {
            let relevance = crate::donated::nix_search::rank::relevance_fields(
                &entry.attr.to_lowercase(),
                &entry.name.to_lowercase(),
                &entry.description.to_lowercase(),
                &term,
            );
            (term.is_empty() || relevance.tier < 6).then_some((relevance, entry))
        })
        .collect();

    ranked.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.attr.cmp(&right.1.attr))
    });

    let total = ranked.len();
    let pages = total.div_ceil(per_page);
    let page = page.max(1).min(pages.max(1));
    let start = (page - 1) * per_page;
    let hits = ranked
        .iter()
        .skip(start)
        .take(per_page)
        .map(|(relevance, entry)| Hit {
            entry,
            tier: relevance.tier,
        })
        .collect();

    Page {
        hits,
        page,
        per_page,
        total,
        pages,
    }
}

/// Canonical digest over the entries, so a corrupted or edited snapshot is not
/// trusted.
fn digest_entries(entries: &[FullEntry]) -> Result<String, CatalogError> {
    let bytes = serde_json::to_vec(entries).map_err(|error| CatalogError::Unparseable {
        detail: error.to_string(),
    })?;
    Ok(Sha256::of_bytes(&bytes).to_hex())
}
