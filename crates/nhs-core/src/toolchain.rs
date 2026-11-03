//! Toolchain resolution.
//!
//! Resolves how `home-manager` can be invoked using exactly the five strategies
//! from architecture §3.1, in this order:
//!
//! 1. `home-manager` on `PATH` → [`HmInvocation::Binary`];
//! 2. inside the resolved Home Manager generation
//!    (`<generation>/bin/home-manager`, else `home-path/bin/home-manager`)
//!    → [`HmInvocation::FromGeneration`] — the audit host's only working route;
//! 3. a nix profile element (`~/.nix-profile`, `/nix/var/nix/profiles/default`,
//!    the state profile, `/run/current-system/sw`) → [`HmInvocation::ProfileElement`];
//! 4. `nix run nixpkgs#home-manager` → [`HmInvocation::NixRun`] (needs network);
//! 5. [`HmInvocation::Absent`].
//!
//! Every strategy gets an entry in [`Toolchain::attempts`], including the ones
//! that were skipped or never reached, so "not found" always names all five.
//!
//! `nixos-rebuild` is detected for *display only* (D2). This module stores its
//! path and never builds or spawns a command line; the crate has no process
//! spawning at all in M1.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::discovery::generation_links;
use crate::env::EnvView;
use crate::fs::FsView;

/// Flake reference used by the `nix run` fallback.
pub const NIX_RUN_FLAKE_REF: &str = "nixpkgs#home-manager";

/// Strategy names, in normative resolution order. `docs/architecture.md` §3.1
/// must list them in exactly this order.
pub const STRATEGY_ORDER: [&str; 5] = [
    STRATEGY_PATH_BINARY,
    STRATEGY_PROFILE_ELEMENT,
    STRATEGY_GENERATION_OR_STORE,
    STRATEGY_NIX_RUN,
    STRATEGY_ABSENT,
];

pub const STRATEGY_PATH_BINARY: &str = "path_binary";
pub const STRATEGY_PROFILE_ELEMENT: &str = "profile_element";
pub const STRATEGY_GENERATION_OR_STORE: &str = "generation_or_store";
pub const STRATEGY_NIX_RUN: &str = "nix_run";
pub const STRATEGY_ABSENT: &str = "absent";
pub const STRATEGY_NIXOS_REBUILD: &str = "nixos_rebuild";

/// How `home-manager` will be invoked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HmInvocation {
    /// A `home-manager` executable on `PATH`.
    Binary(PathBuf),
    /// Found inside a resolved Home Manager generation.
    FromGeneration {
        generation: PathBuf,
        binary: PathBuf,
    },
    /// Found by scanning the store for `*-home-manager-*/bin/home-manager`.
    ///
    /// This is the audit host's real case: a generation carries only
    /// `activate`/`home-path`, never the `home-manager` command itself.
    /// `candidates` lists every match, best first, so the choice is auditable.
    FromStore {
        binary: PathBuf,
        version: String,
        candidates: Vec<PathBuf>,
    },
    /// Installed as a `nix profile` element.
    ProfileElement {
        profile_dir: PathBuf,
        binary: PathBuf,
    },
    /// Not installed; run it from nixpkgs (slower, needs network).
    NixRun { flake_ref: String },
    /// Genuinely absent.
    Absent,
}

impl HmInvocation {
    /// The resolved executable, when the strategy produced one.
    ///
    /// `NixRun` and `Absent` carry no binary: the caller must either fall back to
    /// `PATH` or report that the tool is missing.
    #[must_use]
    pub fn binary(&self) -> Option<&Path> {
        match self {
            Self::Binary(binary)
            | Self::FromGeneration { binary, .. }
            | Self::FromStore { binary, .. }
            | Self::ProfileElement { binary, .. } => Some(binary),
            Self::NixRun { .. } | Self::Absent => None,
        }
    }

    /// Stable strategy name used in JSON and in `attempts`.
    #[must_use]
    pub fn strategy(&self) -> &'static str {
        match self {
            Self::Binary(_) => "path_binary",
            Self::FromGeneration { .. } => "generation",
            Self::FromStore { .. } => "store_binary",
            Self::ProfileElement { .. } => "profile_element",
            Self::NixRun { .. } => "nix_run",
            Self::Absent => "absent",
        }
    }

    /// Stable JSON projection.
    #[must_use]
    pub fn to_json(&self) -> Value {
        match self {
            Self::Binary(binary) => json!({
                "kind": "binary",
                "binary": binary,
            }),
            Self::FromGeneration { generation, binary } => json!({
                "kind": "from-generation",
                "generation": generation,
                "binary": binary,
            }),
            Self::FromStore {
                binary,
                version,
                candidates,
            } => json!({
                "kind": "from-store",
                "binary": binary,
                "version": version,
                "candidates": candidates,
            }),
            Self::ProfileElement {
                profile_dir,
                binary,
            } => json!({
                "kind": "profile-element",
                "profile_dir": profile_dir,
                "binary": binary,
            }),
            Self::NixRun { flake_ref } => json!({
                "kind": "nix-run",
                "flake_ref": flake_ref,
            }),
            Self::Absent => json!({ "kind": "absent" }),
        }
    }
}

/// Resolved external tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolchain {
    /// Path to `nix`; the literal `nix` when none was found (which implies
    /// `home_manager` is `Absent`, because `NixRun` requires a nix binary).
    pub nix: PathBuf,
    pub home_manager: HmInvocation,
    /// Detected for display only. Never executed anywhere in the crate (D2).
    pub nixos_rebuild: Option<PathBuf>,
    /// `(strategy, outcome)` for every strategy tried, in resolution order,
    /// plus `nix` and `nixos_rebuild` probes.
    pub attempts: Vec<(String, String)>,
}

impl Toolchain {
    /// Outcome recorded for `strategy`, if any.
    #[must_use]
    pub fn attempt(&self, strategy: &str) -> Option<&str> {
        self.attempts
            .iter()
            .find(|(name, _)| name == strategy)
            .map(|(_, outcome)| outcome.as_str())
    }

    /// Stable JSON projection.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let attempts: Vec<Value> = self
            .attempts
            .iter()
            .map(|(strategy, outcome)| json!({ "strategy": strategy, "outcome": outcome }))
            .collect();
        json!({
            "nix": self.nix,
            "home_manager": self.home_manager.to_json(),
            "nixos_rebuild": self.nixos_rebuild,
            "attempts": attempts,
        })
    }
}

/// Resolve the toolchain. Records every strategy, succeeds or not.
#[must_use]
pub fn resolve_toolchain(fs: &dyn FsView, env: &EnvView) -> Toolchain {
    let mut attempts: Vec<(String, String)> = Vec::new();

    let (nix, nix_outcome) = resolve_nix(fs, env);
    attempts.push(("nix".to_string(), nix_outcome));
    let nix_present = nix.is_some();
    let nix = nix.unwrap_or_else(|| PathBuf::from("nix"));

    let mut home_manager = HmInvocation::Absent;
    let mut winner: Option<&'static str> = None;

    // The order below is normative and mirrored in `docs/architecture.md` §3.1.
    // `toolchain_attempts_follow_the_documented_order` locks it.
    //
    // 1. PATH binary — explicit and user-controlled.
    let (found, outcome) = find_path_binary(fs, env);
    attempts.push((STRATEGY_PATH_BINARY.to_string(), outcome));
    if let Some(invocation) = found {
        home_manager = invocation;
        winner = Some(STRATEGY_PATH_BINARY);
    }

    // 2. `nix profile` element — an explicit installation, not a heuristic.
    if let Some(previous) = winner {
        attempts.push((STRATEGY_PROFILE_ELEMENT.to_string(), skipped(previous)));
    } else {
        let (found, outcome) = find_profile_element(fs, env);
        attempts.push((STRATEGY_PROFILE_ELEMENT.to_string(), outcome));
        if let Some(invocation) = found {
            home_manager = invocation;
            winner = Some(STRATEGY_PROFILE_ELEMENT);
        }
    }

    // 3. A resolved Home Manager generation, then the store for the command.
    if let Some(previous) = winner {
        attempts.push((STRATEGY_GENERATION_OR_STORE.to_string(), skipped(previous)));
    } else {
        let (found, outcome) = find_in_generation(fs, env);
        attempts.push((STRATEGY_GENERATION_OR_STORE.to_string(), outcome));
        if let Some(invocation) = found {
            home_manager = invocation;
            winner = Some(STRATEGY_GENERATION_OR_STORE);
        }
    }

    // 4. nix run fallback.
    if let Some(previous) = winner {
        attempts.push((STRATEGY_NIX_RUN.to_string(), skipped(previous)));
    } else if nix_present {
        home_manager = HmInvocation::NixRun {
            flake_ref: NIX_RUN_FLAKE_REF.to_string(),
        };
        winner = Some(STRATEGY_NIX_RUN);
        attempts.push((
            STRATEGY_NIX_RUN.to_string(),
            format!("selected: run {NIX_RUN_FLAKE_REF} through nix (needs network)"),
        ));
    } else {
        attempts.push((
            STRATEGY_NIX_RUN.to_string(),
            "not usable: no nix binary found, cannot run nixpkgs#home-manager".to_string(),
        ));
    }

    // 5. Absent.
    let absent_outcome = winner.map_or_else(
        || {
            "selected: no home-manager on PATH, in a profile element, or in the store, and no nix fallback"
                .to_string()
        },
        |strategy| format!("not reached: {strategy} resolved home-manager"),
    );
    attempts.push((STRATEGY_ABSENT.to_string(), absent_outcome));
    if winner.is_none() {
        home_manager = HmInvocation::Absent;
    }

    let (nixos_rebuild, rebuild_outcome) = resolve_nixos_rebuild(fs, env);
    attempts.push((STRATEGY_NIXOS_REBUILD.to_string(), rebuild_outcome));

    Toolchain {
        nix,
        home_manager,
        nixos_rebuild,
        attempts,
    }
}

fn skipped(previous: &str) -> String {
    format!("skipped: home-manager already resolved by {previous}")
}

fn resolve_nix(fs: &dyn FsView, env: &EnvView) -> (Option<PathBuf>, String) {
    let mut tried = Vec::new();
    let fixed = [
        PathBuf::from("/usr/bin/nix"),
        PathBuf::from("/run/current-system/sw/bin/nix"),
        env.home.join(".nix-profile").join("bin").join("nix"),
        PathBuf::from("/nix/var/nix/profiles/default/bin/nix"),
    ];

    for candidate in &fixed {
        if is_file(fs, candidate) {
            return (
                Some(candidate.clone()),
                format!("found {}", candidate.display()),
            );
        }
        tried.push(candidate.display().to_string());
    }

    for entry in &env.path_entries {
        let candidate = entry.join("nix");
        if is_file(fs, &candidate) {
            return (
                Some(candidate.clone()),
                format!("found on PATH at {}", candidate.display()),
            );
        }
        tried.push(candidate.display().to_string());
    }

    (None, format!("not found; tried {}", tried.join(", ")))
}

fn find_path_binary(fs: &dyn FsView, env: &EnvView) -> (Option<HmInvocation>, String) {
    let mut tried = Vec::new();
    for entry in &env.path_entries {
        let candidate = entry.join("home-manager");
        if is_file(fs, &candidate) {
            return (
                Some(HmInvocation::Binary(candidate.clone())),
                format!("found on PATH at {}", candidate.display()),
            );
        }
        tried.push(entry.display().to_string());
    }
    let unique: Vec<&String> = {
        let mut seen: Vec<&String> = Vec::new();
        for entry in &tried {
            if !seen.contains(&entry) {
                seen.push(entry);
            }
        }
        seen
    };
    (
        None,
        format!(
            "not found in any of {} distinct PATH entries (searched {} entries)",
            unique.len(),
            tried.len()
        ),
    )
}

fn find_in_generation(fs: &dyn FsView, env: &EnvView) -> (Option<HmInvocation>, String) {
    let profiles = env.state_home().join("nix").join("profiles");
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    let current = profiles.join("home-manager");
    if fs.is_symlink(&current) || fs.exists(&current) {
        candidates.push(current);
    }

    let mut generations = generation_links(fs, &profiles);
    generations.sort_by_key(|left| std::cmp::Reverse(left.id));
    for generation in &generations {
        candidates.push(generation.link.clone());
    }

    if let Some(user) = &env.user {
        let legacy = PathBuf::from("/nix/var/nix/profiles/per-user")
            .join(user)
            .join("home-manager");
        if fs.is_symlink(&legacy) || fs.exists(&legacy) {
            candidates.push(legacy);
        }
    }

    if candidates.is_empty() {
        // A distribution may install Home Manager without linking a profile; the
        // command is still in the store, and that is what validation must run.
        if let Some((invocation, detail)) = from_store(fs, env) {
            let binary = invocation
                .binary()
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            return (
                Some(invocation),
                format!(
                    "no HM generation profile; found home-manager in the store at {binary}{detail}"
                ),
            );
        }
        return (
            None,
            format!(
                "no HM generation profile found under {} (looked for home-manager and home-manager-<n>-link)",
                profiles.display()
            ),
        );
    }

    for link in candidates {
        let generation = match fs.canonicalize(&link) {
            Ok(path) => path,
            Err(error) => {
                notes.push(format!("{}: cannot resolve ({error})", link.display()));
                continue;
            }
        };

        let direct = generation.join("bin").join("home-manager");
        if is_file(fs, &direct) {
            return (
                Some(HmInvocation::FromGeneration {
                    generation,
                    binary: direct.clone(),
                }),
                format!("found inside generation at {}", direct.display()),
            );
        }

        let via_home_path = generation
            .join("home-path")
            .join("bin")
            .join("home-manager");
        if is_file(fs, &via_home_path) {
            return (
                Some(HmInvocation::FromGeneration {
                    generation,
                    binary: via_home_path.clone(),
                }),
                format!("found via home-path at {}", via_home_path.display()),
            );
        }

        // A Home Manager generation does not ship the `home-manager` command: its
        // `bin/` holds only `home-manager-generation`, and `home-path` is the user's
        // package environment. The command lives in its own derivation in the store.
        if let Some((invocation, detail)) = from_store(fs, env) {
            let binary = invocation
                .binary()
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            return (
                Some(invocation),
                format!("generation has no home-manager binary; found it in the store at {binary}{detail}"),
            );
        }

        notes.push(format!(
            "{}: no bin/home-manager or home-path/bin/home-manager, and no \
             *-home-manager-*/bin/home-manager in {}",
            generation.display(),
            env.store_root.display()
        ));
    }

    (
        None,
        format!(
            "HM generation profiles resolved but no home-manager binary was found: {}",
            notes.join("; ")
        ),
    )
}

fn find_profile_element(fs: &dyn FsView, env: &EnvView) -> (Option<HmInvocation>, String) {
    let mut roots = vec![
        env.home.join(".nix-profile"),
        PathBuf::from("/nix/var/nix/profiles/default"),
        env.state_home()
            .join("nix")
            .join("profiles")
            .join("profile"),
        PathBuf::from("/run/current-system/sw"),
    ];

    // A PATH entry that is `<profile>/bin` with a `manifest.json` next to it is a
    // nix profile; treat its binary as a profile element rather than a plain
    // PATH hit.
    for entry in &env.path_entries {
        let is_bin_dir = entry
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "bin");
        let Some(profile_dir) = entry.parent() else {
            continue;
        };
        if !is_bin_dir || !fs.exists(&profile_dir.join("manifest.json")) {
            continue;
        }
        if !roots.iter().any(|root| root == profile_dir) {
            roots.push(profile_dir.to_path_buf());
        }
    }

    let mut tried = Vec::new();
    for root in roots {
        let binary = root.join("bin").join("home-manager");
        if is_file(fs, &binary) {
            return (
                Some(HmInvocation::ProfileElement {
                    profile_dir: root,
                    binary: binary.clone(),
                }),
                format!("found profile element at {}", binary.display()),
            );
        }
        tried.push(root.display().to_string());
    }

    (
        None,
        format!("not found in profile roots [{}]", tried.join(", ")),
    )
}

fn resolve_nixos_rebuild(fs: &dyn FsView, env: &EnvView) -> (Option<PathBuf>, String) {
    let mut tried = Vec::new();
    let fixed = [
        PathBuf::from("/run/current-system/sw/bin/nixos-rebuild"),
        PathBuf::from("/usr/bin/nixos-rebuild"),
    ];
    for candidate in &fixed {
        if is_file(fs, candidate) {
            return (
                Some(candidate.clone()),
                format!(
                    "detected {} (display only; never executed)",
                    candidate.display()
                ),
            );
        }
        tried.push(candidate.display().to_string());
    }
    for entry in &env.path_entries {
        let candidate = entry.join("nixos-rebuild");
        if is_file(fs, &candidate) {
            return (
                Some(candidate.clone()),
                format!(
                    "detected on PATH at {} (display only; never executed)",
                    candidate.display()
                ),
            );
        }
        tried.push(candidate.display().to_string());
    }
    (None, format!("not found; tried {}", tried.join(", ")))
}

fn is_file(fs: &dyn FsView, path: &std::path::Path) -> bool {
    fs.exists(path) && !fs.is_dir(path)
}

/// Resolve `home-manager` by scanning the store, with the audit trail the
/// resolution report expects.
fn from_store(fs: &dyn FsView, env: &EnvView) -> Option<(HmInvocation, String)> {
    let candidates = find_home_managers_in_store(fs, env);
    let best = candidates.first()?;
    let alternatives = candidates.len().saturating_sub(1);
    let detail = if alternatives == 0 {
        String::new()
    } else {
        format!(
            "; chose version {} over {} other candidate(s): {}",
            best.version,
            alternatives,
            candidates
                .iter()
                .skip(1)
                .map(|candidate| format!("{} ({})", candidate.binary.display(), candidate.version))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Some((
        HmInvocation::FromStore {
            binary: best.binary.clone(),
            version: best.version.clone(),
            candidates: candidates
                .iter()
                .map(|candidate| candidate.binary.clone())
                .collect(),
        },
        detail,
    ))
}

/// Scan the store for every `home-manager` command, best candidate first.
///
/// A Home Manager generation never contains the command, so this is the only
/// local route on distributions where `home-manager` is not on `PATH`.
///
/// Ordering is deterministic and independent of hash prefixes:
///
/// 1. highest parsed version wins (`0-unstable-2026-09-01` beats
///    `0-unstable-2026-08-06`, `26.11` beats `25.05`);
/// 2. equal versions are broken by store path, ascending.
///
/// Picking by hash order alone (the previous behaviour) would select an
/// arbitrary version whenever several are installed.
fn find_home_managers_in_store(fs: &dyn FsView, env: &EnvView) -> Vec<StoreCandidate> {
    let Ok(entries) = fs.read_dir(&env.store_root) else {
        return Vec::new();
    };
    let mut found: Vec<StoreCandidate> = Vec::new();
    for entry in entries {
        let Some(name) = entry.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Store paths are `<hash>-home-manager-<version>`. Match on the infix so
        // that `home-manager-generation`, `home-manager-path`, `home-manager-files`,
        // `home-manager-source` and `.drv` files are all excluded.
        let is_derivation = std::path::Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("drv"));
        if !name.contains("-home-manager-") || is_derivation {
            continue;
        }
        if [
            "-home-manager-generation",
            "-home-manager-path",
            "-home-manager-files",
            "-home-manager-source",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
        {
            continue;
        }
        let binary = entry.join("bin").join("home-manager");
        if !is_file(fs, &binary) {
            continue;
        }
        let version = name
            .split_once("-home-manager-")
            .map(|(_, version)| version.to_string())
            .unwrap_or_default();
        found.push(StoreCandidate { binary, version });
    }
    found.sort_by(|left, right| {
        compare_versions(&right.version, &left.version).then_with(|| left.binary.cmp(&right.binary))
    });
    found
}

/// One `home-manager` command found in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreCandidate {
    pub binary: PathBuf,
    /// Version text as it appears in the store name, e.g. `0-unstable-2026-08-06`.
    pub version: String,
}

/// Compare two version strings component-wise.
///
/// Numeric components compare numerically, everything else compares
/// lexicographically, and a longer version wins when the shared prefix is equal.
/// This is enough for `26.11`, `0-unstable-2026-08-06` and `git-abcdef` and does
/// not try to be a full Nix version comparator.
fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let split = |value: &str| -> Vec<String> {
        value
            .split(|character: char| !character.is_ascii_alphanumeric())
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect()
    };

    let left_parts = split(left);
    let right_parts = split(right);
    for (left_part, right_part) in left_parts.iter().zip(right_parts.iter()) {
        let ordering = match (left_part.parse::<u64>(), right_part.parse::<u64>()) {
            (Ok(left_number), Ok(right_number)) => left_number.cmp(&right_number),
            _ => left_part.cmp(right_part),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left_parts.len().cmp(&right_parts.len())
}

#[cfg(test)]
mod tests {
    use super::{resolve_toolchain, HmInvocation};
    use crate::env::EnvView;
    use crate::fs::MemFs;
    use std::path::PathBuf;

    fn env(home: &str, path_entries: &[&str]) -> EnvView {
        EnvView {
            home: PathBuf::from(home),
            user: Some("tester".to_string()),
            path_entries: path_entries.iter().copied().map(PathBuf::from).collect(),
            xdg_config_home: None,
            xdg_state_home: None,
            store_root: PathBuf::from("/nix/store"),
        }
    }

    #[test]
    fn skips_later_strategies_after_a_path_binary() {
        let fs = MemFs::new().with_file("/opt/bin/home-manager", "");
        let toolchain = resolve_toolchain(&fs, &env("/home/tester", &["/opt/bin"]));
        assert!(matches!(toolchain.home_manager, HmInvocation::Binary(_)));
        assert!(toolchain
            .attempt("generation_or_store")
            .is_some_and(|outcome| outcome.starts_with("skipped")));
    }
}
