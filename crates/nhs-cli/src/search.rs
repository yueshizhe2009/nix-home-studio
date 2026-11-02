//! `nhs search` — the curated catalogue offline, the full one on request.
//!
//! Two tiers, one command, one JSON envelope. They are deliberately *not* merged:
//! the curated entries carry licence and platform metadata that the full catalogue
//! does not have, and pretending otherwise would make every filter mean two
//! different things. A filter that only applies to one tier is therefore refused
//! for the other, loudly, instead of being silently ignored.
//!
//! Everything user-visible except the final JSON goes to stderr, so `--json` can
//! be piped into another program without progress lines corrupting it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nhs_core::catalog::{self, Catalog, Filters, HostPlatform};
use nhs_core::fullcatalog::{self, FullEntry, Limits};
use nhs_core::proc::{CancelToken, RealRunner};
use nhs_core::revision::{self, SourceKind};
use nhs_core::{EnvView, FsView, RealFs};

use crate::app::{Exit, Failure};

/// Default page size.
pub const DEFAULT_PER_PAGE: usize = 20;

/// Parsed arguments for `nhs search`.
///
/// A flag struct is what a CLI is; the bools are the flags themselves.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default)]
pub struct SearchOptions {
    pub query: String,
    pub full: bool,
    pub json: bool,
    pub platform: Option<String>,
    pub include_unfree: bool,
    pub include_broken: bool,
    pub include_nixos_only: bool,
    pub license_allow: Vec<String>,
    pub license_deny: Vec<String>,
    pub page: usize,
    pub per_page: usize,
    pub flake_ref: Option<String>,
    pub timeout: Option<Duration>,
    pub home: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
}

impl SearchOptions {
    /// Parse the flags after `search`.
    ///
    /// Returns usage errors as strings; policy refusals are decided in [`run`],
    /// because "this tier has no licence data" is a refusal, not a typo.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self {
            page: 1,
            per_page: DEFAULT_PER_PAGE,
            ..Self::default()
        };
        let mut query: Option<String> = None;
        let mut index = 0;
        while let Some(arg) = args.get(index) {
            index += 1;
            match arg.as_str() {
                "--full" => options.full = true,
                "--json" => options.json = true,
                "--include-unfree" => options.include_unfree = true,
                "--include-broken" => options.include_broken = true,
                "--include-nixos-only" => options.include_nixos_only = true,
                "--platform" => {
                    options.platform = Some(next(args, &mut index, arg)?);
                }
                "--license-allow" => {
                    options.license_allow = split_list(&next(args, &mut index, arg)?);
                }
                "--license-deny" => {
                    options.license_deny = split_list(&next(args, &mut index, arg)?);
                }
                "--page" => {
                    options.page = parse_positive(&next(args, &mut index, arg)?, "--page")?;
                }
                "--per-page" => {
                    options.per_page = parse_positive(&next(args, &mut index, arg)?, "--per-page")?;
                }
                "--flake-ref" => {
                    options.flake_ref = Some(next(args, &mut index, arg)?);
                }
                "--timeout" => {
                    let seconds = parse_positive(&next(args, &mut index, arg)?, "--timeout")?;
                    options.timeout = Some(Duration::from_secs(
                        u64::try_from(seconds).map_err(|_| "--timeout is too large")?,
                    ));
                }
                "--home" => options.home = Some(PathBuf::from(next(args, &mut index, arg)?)),
                "--state-dir" => {
                    options.state_dir = Some(PathBuf::from(next(args, &mut index, arg)?));
                }
                other if other.starts_with("--") => {
                    return Err(format!("unexpected argument `{other}`"))
                }
                other => {
                    if query.is_some() {
                        return Err(format!("unexpected second query `{other}`"));
                    }
                    query = Some(other.to_string());
                }
            }
        }
        options.query =
            query.ok_or_else(|| "search needs a query (use \"\" to browse)".to_string())?;

        // `--flake-ref` and `--timeout` only mean something for the full tier.
        if !options.full {
            if options.flake_ref.is_some() {
                return Err("`--flake-ref` only applies to `--full`".to_string());
            }
            if options.timeout.is_some() {
                return Err("`--timeout` only applies to `--full`".to_string());
            }
        }
        Ok(options)
    }

    /// Filters that need catalogue metadata, which only the curated tier has.
    fn metadata_filters(&self) -> Vec<&'static str> {
        let mut used = Vec::new();
        if self.platform.is_some() {
            used.push("--platform");
        }
        if self.include_unfree {
            used.push("--include-unfree");
        }
        if self.include_broken {
            used.push("--include-broken");
        }
        if self.include_nixos_only {
            used.push("--include-nixos-only");
        }
        if !self.license_allow.is_empty() {
            used.push("--license-allow");
        }
        if !self.license_deny.is_empty() {
            used.push("--license-deny");
        }
        used
    }
}

fn next(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    let value = args
        .get(*index)
        .cloned()
        .ok_or_else(|| format!("`{flag}` needs a value"))?;
    *index += 1;
    Ok(value)
}

fn parse_positive(raw: &str, flag: &str) -> Result<usize, String> {
    let value: usize = raw
        .parse()
        .map_err(|_| format!("`{flag} {raw}` is not a positive number"))?;
    if value == 0 {
        return Err(format!("`{flag}` must be greater than zero"));
    }
    Ok(value)
}

fn split_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// Where the results came from, for the JSON envelope and the progress line.
struct Source {
    kind: String,
    reference: Option<String>,
    revision: Option<String>,
    pinned: Option<bool>,
}

/// Run the search.
pub fn run(options: &SearchOptions) -> Result<Exit, Failure> {
    // A metadata filter against a tier without metadata is a refusal, and it
    // happens before any lock is read or any process is started.
    if options.full {
        let unused = options.metadata_filters();
        if !unused.is_empty() {
            return Err(Failure::refused(format!(
                "{} cannot be used with `--full`: the full catalogue has no licence, platform or \
                 unfree metadata. Drop the filter, or search the curated catalogue without \
                 `--full`.",
                unused.join(", ")
            )));
        }
    }

    if options.full {
        run_full(options)
    } else {
        run_curated(options)
    }
}

fn run_curated(options: &SearchOptions) -> Result<Exit, Failure> {
    let catalog = Catalog::builtin()
        .map_err(|error| Failure::failed(format!("the built-in catalogue is broken: {error}")))?;
    let host = HostPlatform::from_os_arch(std::env::consts::OS, std::env::consts::ARCH);
    let filters = Filters {
        platform: options.platform.clone(),
        include_unfree: options.include_unfree,
        include_broken: options.include_broken,
        include_nixos_only: options.include_nixos_only,
        license_allow: options.license_allow.clone(),
        license_deny: options.license_deny.clone(),
    };
    let page = catalog::search(
        &catalog,
        &options.query,
        &filters,
        &host,
        options.page,
        options.per_page,
    );

    let source = Source {
        kind: "curated".to_string(),
        reference: None,
        revision: None,
        pinned: None,
    };
    let hits: Vec<serde_json::Value> = page
        .hits
        .iter()
        .map(|hit| {
            serde_json::json!({
                "attr": hit.entry.attr,
                "name": hit.entry.name,
                "version": serde_json::Value::Null,
                "description": hit.entry.description,
                "category": hit.entry.category,
                "license": hit.entry.license,
                // The frozen schema asks for the catalogue's tags, not a
                // runtime verdict; a UI can derive "unknown" from an empty list.
                "platform": hit.entry.platforms,
            })
        })
        .collect();

    emit(
        PageEnvelope {
            options,
            source: &source,
            page: page.page,
            per_page: page.per_page,
            total: page.total,
            pages: page.pages,
            hits,
        },
        || {
            let mut text = String::new();
            for hit in &page.hits {
                let license = hit.entry.license.as_deref().unwrap_or("unknown licence");
                let _ = writeln!(
                    text,
                    "{}\t{} — {}\n\t{}; {}",
                    hit.entry.attr,
                    hit.entry.name,
                    hit.entry.description,
                    hit.entry.category,
                    license
                );
            }
            text
        },
    )
}

#[allow(clippy::too_many_lines)]
fn run_full(options: &SearchOptions) -> Result<Exit, Failure> {
    let mut env = EnvView::from_process();
    if let Some(home) = &options.home {
        env.home.clone_from(home);
    }
    let state_dir = options
        .state_dir
        .clone()
        .unwrap_or_else(|| env.state_home().join("nix-home-studio"));
    let cwd = config_root(&env).unwrap_or_else(|| env.home.clone());
    let lock = read_lock(&cwd);

    let resolution = revision::resolve(options.flake_ref.as_deref(), lock.as_deref())
        .map_err(|error| Failure::failed(format!("cannot resolve nixpkgs: {error}")))?;

    let mut limits = Limits::default();
    if let Some(timeout) = options.timeout {
        limits.timeout = timeout;
    }

    let cached = fullcatalog::load_cached(&RealFs, &state_dir, &resolution);
    if let Some(snapshot) = &cached {
        eprintln!(
            "nhs: using cached catalogue {} ({} entries) from {}",
            snapshot.revision_hash, snapshot.entries_count, snapshot.reference
        );
    } else {
        eprintln!(
            "nhs: building the catalogue for {} — the first run can take several minutes",
            resolution.reference
        );
    }

    let started = Instant::now();
    let runner = RealRunner::new(&env);
    let mut sink = |stream: nhs_core::proc::Stream, line: &str| {
        // Nix's own progress goes to stderr; stdout carries the JSON.
        if stream == nhs_core::proc::Stream::Stderr {
            eprintln!("nix: {line}");
        }
    };
    let snapshot = fullcatalog::build(
        &RealFs,
        &runner,
        &resolution,
        &state_dir,
        &cwd,
        limits,
        &CancelToken::new(),
        &mut sink,
    )
    .map_err(|error| Failure::failed(format!("the catalogue could not be built: {error}")))?;

    if cached.is_none() {
        eprintln!(
            "nhs: catalogue ready: {} entries in {} ms (key {})",
            snapshot.entries_count,
            started.elapsed().as_millis(),
            snapshot.revision_hash
        );
    }
    if !snapshot.pinned {
        eprintln!(
            "nhs: note: {} is not a pinned revision, so a rebuild can return different packages",
            snapshot.reference
        );
    }

    let page = fullcatalog::search(&snapshot, &options.query, options.page, options.per_page);
    let source = Source {
        kind: match resolution.kind {
            SourceKind::Explicit => "explicit",
            SourceKind::LockFile => "flake-lock",
            SourceKind::UnpinnedDefault => "unpinned-default",
        }
        .to_string(),
        reference: Some(resolution.reference.clone()),
        revision: resolution.revision.clone(),
        pinned: Some(resolution.pinned),
    };
    let hits: Vec<serde_json::Value> = page
        .hits
        .iter()
        .map(|hit| {
            let entry: &FullEntry = hit.entry;
            serde_json::json!({
                "attr": entry.attr,
                "name": entry.name,
                "version": entry.version,
                "description": entry.description,
                "category": serde_json::Value::Null,
                "license": serde_json::Value::Null,
                "platform": serde_json::Value::Null,
            })
        })
        .collect();

    emit(
        PageEnvelope {
            options,
            source: &source,
            page: page.page,
            per_page: page.per_page,
            total: page.total,
            pages: page.pages,
            hits,
        },
        || {
            let mut text = String::new();
            for hit in &page.hits {
                let _ = writeln!(
                    text,
                    "{}\t{} {} — {}",
                    hit.entry.attr, hit.entry.name, hit.entry.version, hit.entry.description
                );
            }
            text
        },
    )
}

/// Everything the output needs, in one value so `emit` keeps two arguments.
struct PageEnvelope<'a> {
    options: &'a SearchOptions,
    source: &'a Source,
    page: usize,
    per_page: usize,
    total: usize,
    pages: usize,
    hits: Vec<serde_json::Value>,
}

/// Print the envelope (JSON) or the human lines, plus the footer.
fn emit(envelope: PageEnvelope<'_>, human: impl FnOnce() -> String) -> Result<Exit, Failure> {
    let PageEnvelope {
        options,
        source,
        page,
        per_page,
        total,
        pages,
        hits,
    } = envelope;
    if options.json {
        let payload = serde_json::json!({
            "schema": 1,
            "query": options.query,
            "source": {
                "kind": source.kind,
                "reference": source.reference,
                "revision": source.revision,
                "pinned": source.pinned,
            },
            "page": page,
            "per_page": per_page,
            "total": total,
            "pages": pages,
            "hits": hits,
        });
        let rendered = serde_json::to_string_pretty(&payload)
            .map_err(|error| Failure::failed(error.to_string()))?;
        println!("{rendered}");
    } else {
        print!("{}", human());
        println!("{total} result(s), page {page}/{pages}");
    }
    Ok(Exit::Ok)
}

/// The configuration root, when a layout names one.
fn config_root(env: &EnvView) -> Option<PathBuf> {
    let discovery = nhs_core::detect(&RealFs, env);
    match discovery.layout {
        nhs_core::Layout::FlakeHome { root, .. } => Some(root),
        nhs_core::Layout::ClassicHome { main_file } => main_file.parent().map(Path::to_path_buf),
        _ => None,
    }
}

/// The text of `flake.lock` under `root`, if there is one.
fn read_lock(root: &Path) -> Option<String> {
    let path = root.join("flake.lock");
    RealFs
        .exists(&path)
        .then(|| std::fs::read_to_string(&path).ok())
        .flatten()
}
