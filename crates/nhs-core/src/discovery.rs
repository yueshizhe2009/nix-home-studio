//! Home Manager layout discovery.
//!
//! Pure over [`FsView`] + [`EnvView`]: no process environment, no process
//! spawning, no writes. Every probe is recorded in [`Discovery::attempts`] with
//! the path tried and the outcome, so a caller can explain *why* a layout was
//! accepted or refused instead of only reporting "no configuration found".
//!
//! # Detection order
//!
//! 1. `$XDG_CONFIG_HOME/home-manager` (else `$HOME/.config/home-manager`):
//!    - `flake.nix` → `FlakeHome` when it defines `homeConfigurations.<user>`;
//!      `FlakeNixos` when it also defines `nixosConfigurations` **and** that
//!      configuration pulls in the `home-manager` module (D5 prefers the NixOS
//!      shape in that case, because the HM config is then owned by the system
//!      flake).
//!    - `home.nix` → `ClassicHome`, *unless* the same directory's `flake.nix`
//!      references `home.nix` (then the file is an input of the flake, not a
//!      classic entry point).
//! 2. `/etc/nixos/flake.nix` or `/etc/nixos/configuration.nix` → `FlakeNixos`
//!    (read-only detection; D2 forbids rebuilding anything).
//! 3. No configuration at all, but `$XDG_STATE_HOME/nix/profiles/home-manager`
//!    exists → `ProfileOnly`, with the `home-manager-<n>-link` siblings
//!    collected as [`GenerationLink`]s.
//! 4. Otherwise `Unknown`, whose reason lists every path tried.
//!
//! # "Never guess"
//!
//! All steps are evaluated before a decision. If two or more *configuration*
//! layouts are plausible (for example a user flake and a NixOS flake), the
//! result is `Unknown { reason }` naming every candidate. `ProfileOnly` is a
//! fallback, so it never competes with a configuration.
//!
//! # Flake heuristics
//!
//! M1 does not evaluate Nix. Facts are extracted textually:
//!
//! - `homeConfigurations` / `nixosConfigurations` are detected by keyword;
//! - the `home-manager` module reference is detected by the keyword
//!   `home-manager` in the flake text;
//! - the user attribute is accepted only when `$USER` literally appears in the
//!   file, otherwise discovery refuses rather than guessing;
//! - `host` comes from `networking.hostName = "…"`, else from the identifier
//!   after `nixosConfigurations.`, else the literal `unknown`.
//!
//! These heuristics are intentionally conservative. M2+ replaces them with
//! `rnix` once parsing is introduced.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::env::EnvView;
use crate::fs::FsView;

const NIXOS_DIR: &str = "/etc/nixos";
const FLAKE_NIX: &str = "flake.nix";
const HOME_NIX: &str = "home.nix";

/// Detected Home Manager deployment shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layout {
    /// Flake with `homeConfigurations.<attr>`.
    FlakeHome {
        root: PathBuf,
        main_file: PathBuf,
        attr: String,
    },
    /// Flake (or `/etc/nixos/configuration.nix`) where HM is a NixOS module.
    /// Detected for display only; never mutated (D2).
    FlakeNixos {
        root: PathBuf,
        main_file: PathBuf,
        host: String,
        user: String,
    },
    /// Classic `home.nix` without a flake.
    ClassicHome { main_file: PathBuf },
    /// No configuration found; HM exists only as a profile element plus
    /// generation links.
    ProfileOnly {
        profile_dir: PathBuf,
        generations: Vec<GenerationLink>,
    },
    /// Nothing usable, or more than one plausible shape.
    Unknown { reason: String },
}

impl Layout {
    /// Stable machine-readable variant name (kebab-case).
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::FlakeHome { .. } => "flake-home",
            Self::FlakeNixos { .. } => "flake-nixos",
            Self::ClassicHome { .. } => "classic-home",
            Self::ProfileOnly { .. } => "profile-only",
            Self::Unknown { .. } => "unknown",
        }
    }

    /// Rust variant name, used in human-readable reasons and tests.
    #[must_use]
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::FlakeHome { .. } => "FlakeHome",
            Self::FlakeNixos { .. } => "FlakeNixos",
            Self::ClassicHome { .. } => "ClassicHome",
            Self::ProfileOnly { .. } => "ProfileOnly",
            Self::Unknown { .. } => "Unknown",
        }
    }

    /// The configuration entry point, when the layout has one.
    #[must_use]
    pub fn main_file(&self) -> Option<&Path> {
        match self {
            Self::FlakeHome { main_file, .. }
            | Self::FlakeNixos { main_file, .. }
            | Self::ClassicHome { main_file } => Some(main_file),
            Self::ProfileOnly { .. } | Self::Unknown { .. } => None,
        }
    }

    /// Directory that contains the configuration, when known.
    #[must_use]
    pub fn root(&self) -> Option<&Path> {
        match self {
            Self::FlakeHome { root, .. } | Self::FlakeNixos { root, .. } => Some(root),
            Self::ClassicHome { main_file } => main_file.parent(),
            Self::ProfileOnly { profile_dir, .. } => Some(profile_dir),
            Self::Unknown { .. } => None,
        }
    }

    /// Human-readable explanation carried by `Unknown`.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Unknown { reason } => Some(reason),
            _ => None,
        }
    }

    /// Home Manager generations, when the layout carries any.
    #[must_use]
    pub fn generations(&self) -> &[GenerationLink] {
        match self {
            Self::ProfileOnly { generations, .. } => generations,
            _ => &[],
        }
    }

    /// Stable JSON projection. Keys are part of the CLI's `--json` contract.
    #[must_use]
    pub fn to_json(&self) -> Value {
        match self {
            Self::FlakeHome {
                root,
                main_file,
                attr,
            } => json!({
                "kind": self.kind(),
                "root": root,
                "main_file": main_file,
                "attr": attr,
            }),
            Self::FlakeNixos {
                root,
                main_file,
                host,
                user,
            } => json!({
                "kind": self.kind(),
                "root": root,
                "main_file": main_file,
                "host": host,
                "user": user,
            }),
            Self::ClassicHome { main_file } => json!({
                "kind": self.kind(),
                "main_file": main_file,
            }),
            Self::ProfileOnly {
                profile_dir,
                generations,
            } => json!({
                "kind": self.kind(),
                "profile_dir": profile_dir,
                "generations": generations.iter().map(GenerationLink::to_json).collect::<Vec<_>>(),
            }),
            Self::Unknown { reason } => json!({
                "kind": self.kind(),
                "reason": reason,
            }),
        }
    }
}

/// One Home Manager generation link found next to a profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationLink {
    /// Numeric id parsed from `home-manager-<n>-link`.
    pub id: Option<u32>,
    /// The symlink itself.
    pub link: PathBuf,
    /// Raw symlink target, when it could be read.
    pub target: Option<PathBuf>,
}

impl GenerationLink {
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "link": self.link,
            "target": self.target,
        })
    }
}

/// One probe performed during discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryAttempt {
    /// Short step identifier, e.g. `config_home_flake`.
    pub step: String,
    /// Path that was inspected, when the step had one.
    pub path: Option<PathBuf>,
    /// Whether this step produced a layout candidate.
    pub accepted: bool,
    /// What happened, in one sentence.
    pub outcome: String,
}

impl DiscoveryAttempt {
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "step": self.step,
            "path": self.path,
            "accepted": self.accepted,
            "outcome": self.outcome,
        })
    }
}

/// Result of [`detect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub layout: Layout,
    pub attempts: Vec<DiscoveryAttempt>,
}

impl Discovery {
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "layout": self.layout.to_json(),
            "main_file": self.layout.main_file(),
            "reason": self.layout.reason(),
            "attempts": self.attempts.iter().map(DiscoveryAttempt::to_json).collect::<Vec<_>>(),
        })
    }
}

/// Detect the Home Manager layout and record every probe.
///
/// Never mutates anything and never guesses between competing layouts.
#[must_use]
pub fn detect(fs: &dyn FsView, env: &EnvView) -> Discovery {
    let mut attempts = Vec::new();
    let mut candidates: Vec<Layout> = Vec::new();

    let config_dir = env.config_home().join("home-manager");
    let flake = config_dir.join(FLAKE_NIX);
    let home_nix = config_dir.join(HOME_NIX);

    let flake_text = probe_user_flake(fs, env, &flake, &config_dir, &mut attempts, &mut candidates);

    // Classic entry point. A `home.nix` imported by the flake is an input of the
    // flake, not a competing classic configuration.
    if fs.exists(&home_nix) {
        let imported_by_flake = flake_text
            .as_deref()
            .is_some_and(|text| text.contains(HOME_NIX));
        if imported_by_flake {
            attempts.push(DiscoveryAttempt {
                step: "config_home_classic".to_string(),
                path: Some(home_nix),
                accepted: false,
                outcome: "home.nix is referenced by flake.nix; treated as a flake input, not a classic layout"
                    .to_string(),
            });
        } else {
            attempts.push(DiscoveryAttempt {
                step: "config_home_classic".to_string(),
                path: Some(home_nix.clone()),
                accepted: true,
                outcome: "accepted ClassicHome: home.nix without a competing flake import"
                    .to_string(),
            });
            candidates.push(Layout::ClassicHome {
                main_file: home_nix,
            });
        }
    } else {
        attempts.push(DiscoveryAttempt {
            step: "config_home_classic".to_string(),
            path: Some(home_nix),
            accepted: false,
            outcome: "not present".to_string(),
        });
    }

    probe_nixos(fs, env, &mut attempts, &mut candidates);

    // Profile-only is the documented fallback: it only applies when no
    // configuration layout was found at all.
    if candidates.is_empty() {
        let profiles = env.state_home().join("nix").join("profiles");
        let profile_link = profiles.join("home-manager");
        if fs.exists(&profile_link) || fs.is_symlink(&profile_link) {
            let generations = generation_links(fs, &profiles);
            attempts.push(DiscoveryAttempt {
                step: "state_profile".to_string(),
                path: Some(profile_link),
                accepted: true,
                outcome: format!(
                    "accepted ProfileOnly: home-manager profile with {} generation link(s)",
                    generations.len()
                ),
            });
            candidates.push(Layout::ProfileOnly {
                profile_dir: profiles,
                generations,
            });
        } else {
            attempts.push(DiscoveryAttempt {
                step: "state_profile".to_string(),
                path: Some(profile_link),
                accepted: false,
                outcome: "no home-manager entry in the nix profiles directory".to_string(),
            });
        }
    } else {
        attempts.push(DiscoveryAttempt {
            step: "state_profile".to_string(),
            path: Some(env.state_home().join("nix").join("profiles")),
            accepted: false,
            outcome: "skipped: a configuration layout was found".to_string(),
        });
    }

    let layout = if candidates.len() > 1 {
        Layout::Unknown {
            reason: ambiguity_reason(&candidates),
        }
    } else if candidates.is_empty() {
        Layout::Unknown {
            reason: unknown_reason(&attempts),
        }
    } else {
        candidates.pop().unwrap_or_else(|| Layout::Unknown {
            reason: unknown_reason(&attempts),
        })
    };

    Discovery { layout, attempts }
}

fn probe_user_flake(
    fs: &dyn FsView,
    env: &EnvView,
    flake: &Path,
    root: &Path,
    attempts: &mut Vec<DiscoveryAttempt>,
    candidates: &mut Vec<Layout>,
) -> Option<String> {
    if !fs.exists(flake) {
        attempts.push(DiscoveryAttempt {
            step: "config_home_flake".to_string(),
            path: Some(flake.to_path_buf()),
            accepted: false,
            outcome: "not present".to_string(),
        });
        return None;
    }

    let text = match fs.read_to_string(flake) {
        Ok(text) => text,
        Err(error) => {
            attempts.push(DiscoveryAttempt {
                step: "config_home_flake".to_string(),
                path: Some(flake.to_path_buf()),
                accepted: false,
                outcome: format!("flake.nix exists but could not be read: {error}"),
            });
            return None;
        }
    };

    match classify_user_flake(&text, root, flake, env.user.as_deref()) {
        Ok(layout) => {
            attempts.push(DiscoveryAttempt {
                step: "config_home_flake".to_string(),
                path: Some(flake.to_path_buf()),
                accepted: true,
                outcome: format!("accepted {} from flake.nix", layout.display_name()),
            });
            candidates.push(layout);
        }
        Err(reason) => {
            attempts.push(DiscoveryAttempt {
                step: "config_home_flake".to_string(),
                path: Some(flake.to_path_buf()),
                accepted: false,
                outcome: reason,
            });
        }
    }

    Some(text)
}

fn probe_nixos(
    fs: &dyn FsView,
    env: &EnvView,
    attempts: &mut Vec<DiscoveryAttempt>,
    candidates: &mut Vec<Layout>,
) {
    let root = PathBuf::from(NIXOS_DIR);
    let flake = root.join(FLAKE_NIX);
    let configuration = root.join("configuration.nix");

    let candidate_path = if fs.exists(&flake) {
        flake
    } else if fs.exists(&configuration) {
        configuration
    } else {
        attempts.push(DiscoveryAttempt {
            step: "nixos".to_string(),
            path: Some(flake),
            accepted: false,
            outcome: "neither /etc/nixos/flake.nix nor /etc/nixos/configuration.nix is present"
                .to_string(),
        });
        return;
    };

    let text = match fs.read_to_string(&candidate_path) {
        Ok(text) => text,
        Err(error) => {
            attempts.push(DiscoveryAttempt {
                step: "nixos".to_string(),
                path: Some(candidate_path.clone()),
                accepted: false,
                outcome: format!(
                    "{} exists but could not be read: {error}",
                    candidate_path.display()
                ),
            });
            return;
        }
    };

    let path = candidate_path;
    match classify_nixos(&text, &root, &path, env.user.as_deref()) {
        Ok(layout) => {
            attempts.push(DiscoveryAttempt {
                step: "nixos".to_string(),
                path: Some(path),
                accepted: true,
                outcome: format!(
                    "accepted {} (read-only detection; never rebuilt)",
                    layout.display_name()
                ),
            });
            candidates.push(layout);
        }
        Err(reason) => {
            attempts.push(DiscoveryAttempt {
                step: "nixos".to_string(),
                path: Some(path),
                accepted: false,
                outcome: reason,
            });
        }
    }
}

/// Extract generation links from a nix profiles directory, sorted by id.
#[must_use]
pub fn generation_links(fs: &dyn FsView, profiles: &Path) -> Vec<GenerationLink> {
    let Ok(entries) = fs.read_dir(profiles) else {
        return Vec::new();
    };

    let mut links = Vec::new();
    for entry in entries {
        let Some(file_name) = entry.file_name() else {
            continue;
        };
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(id) = name
            .strip_prefix("home-manager-")
            .and_then(|rest| rest.strip_suffix("-link"))
            .and_then(|digits| digits.parse::<u32>().ok())
        else {
            continue;
        };
        links.push(GenerationLink {
            id: Some(id),
            target: fs.read_link(&entry).ok(),
            link: entry,
        });
    }
    links.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then_with(|| left.link.cmp(&right.link))
    });
    links
}

#[derive(Debug, Default)]
struct FlakeFacts {
    home_configurations: bool,
    nixos_configurations: bool,
    home_manager_module: bool,
    mentions_user: bool,
    host: Option<String>,
}

fn scan_flake(text: &str, user: Option<&str>) -> FlakeFacts {
    FlakeFacts {
        home_configurations: text.contains("homeConfigurations"),
        nixos_configurations: text.contains("nixosConfigurations"),
        home_manager_module: text.contains("home-manager"),
        mentions_user: user.is_some_and(|user| !user.is_empty() && text.contains(user)),
        host: quoted_after(text, "networking.hostName")
            .or_else(|| identifier_after(text, "nixosConfigurations.")),
    }
}

fn classify_user_flake(
    text: &str,
    root: &Path,
    main_file: &Path,
    user: Option<&str>,
) -> Result<Layout, String> {
    let Some(user) = user.filter(|user| !user.is_empty()) else {
        return Err(
            "flake.nix found but neither $USER nor $LOGNAME is set; cannot choose the homeConfigurations attribute"
                .to_string(),
        );
    };
    let facts = scan_flake(text, Some(user));

    if facts.nixos_configurations && facts.home_manager_module {
        return Ok(Layout::FlakeNixos {
            root: root.to_path_buf(),
            main_file: main_file.to_path_buf(),
            host: facts.host.unwrap_or_else(|| "unknown".to_string()),
            user: user.to_string(),
        });
    }

    if facts.home_configurations {
        if !facts.mentions_user {
            return Err(format!(
                "flake.nix defines homeConfigurations but the user {user} does not appear in it; refusing to guess the attribute"
            ));
        }
        return Ok(Layout::FlakeHome {
            root: root.to_path_buf(),
            main_file: main_file.to_path_buf(),
            attr: user.to_string(),
        });
    }

    if facts.nixos_configurations {
        return Err(
            "flake.nix defines nixosConfigurations but does not reference the home-manager module"
                .to_string(),
        );
    }

    Err("flake.nix defines neither homeConfigurations nor a home-manager NixOS module".to_string())
}

fn classify_nixos(
    text: &str,
    root: &Path,
    main_file: &Path,
    user: Option<&str>,
) -> Result<Layout, String> {
    let facts = scan_flake(text, user);
    if !facts.nixos_configurations && !facts.home_manager_module {
        return Err(
            "the NixOS configuration does not define nixosConfigurations or reference home-manager"
                .to_string(),
        );
    }
    Ok(Layout::FlakeNixos {
        root: root.to_path_buf(),
        main_file: main_file.to_path_buf(),
        host: facts.host.unwrap_or_else(|| "unknown".to_string()),
        user: user.unwrap_or("unknown").to_string(),
    })
}

/// Text between the first pair of double quotes after `marker`.
fn quoted_after(text: &str, marker: &str) -> Option<String> {
    let start = text.find(marker)?;
    let rest = text.get(start.saturating_add(marker.len())..)?;
    let after_first_quote = rest.split_once('"')?.1;
    let value = after_first_quote.split_once('"')?.0.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Identifier immediately after `marker` (letters, digits, `_`, `-`).
fn identifier_after(text: &str, marker: &str) -> Option<String> {
    let start = text.find(marker)?;
    let rest = text.get(start.saturating_add(marker.len())..)?;
    let identifier: String = rest
        .trim_start()
        .chars()
        .take_while(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-'))
        .collect();
    if identifier.is_empty() {
        None
    } else {
        Some(identifier)
    }
}

fn ambiguity_reason(candidates: &[Layout]) -> String {
    let described: Vec<String> = candidates
        .iter()
        .map(|layout| {
            let path = layout.main_file().or_else(|| layout.root()).map_or_else(
                || "<unknown>".to_string(),
                |path| path.display().to_string(),
            );
            format!("{} at {path}", layout.display_name())
        })
        .collect();
    format!(
        "two or more Home Manager layouts are plausible ({}); refusing to guess",
        described.join(" and ")
    )
}

fn unknown_reason(attempts: &[DiscoveryAttempt]) -> String {
    let described: Vec<String> = attempts
        .iter()
        .map(|attempt| {
            let path = attempt
                .path
                .as_ref()
                .map_or_else(|| "-".to_string(), |path| path.display().to_string());
            let outcome = &attempt.outcome;
            format!("{path}: {outcome}")
        })
        .collect();
    format!(
        "no Home Manager configuration found; tried: {}",
        described.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::{classify_user_flake, generation_links, Layout};
    use crate::fs::MemFs;
    use std::path::{Path, PathBuf};

    const HOME_FLAKE: &str = r#"
{
  outputs = { home-manager, ... }: {
    homeConfigurations."tester" = home-manager.lib.homeManagerConfiguration { };
  };
}
"#;

    const NIXOS_FLAKE: &str = r#"
{
  outputs = { nixpkgs, home-manager, ... }: {
    nixosConfigurations.testerbox = nixpkgs.lib.nixosSystem {
      modules = [
        home-manager.nixosModules.home-manager
        { networking.hostName = "testerbox"; home-manager.users.tester = { }; }
      ];
    };
  };
}
"#;

    #[test]
    fn flake_with_both_shapes_prefers_flake_nixos() {
        let combined = format!("{HOME_FLAKE}\n{NIXOS_FLAKE}");
        let layout = classify_user_flake(
            &combined,
            Path::new("/home/tester/.config/home-manager"),
            Path::new("/home/tester/.config/home-manager/flake.nix"),
            Some("tester"),
        );
        assert!(matches!(layout, Ok(Layout::FlakeNixos { .. })));
    }

    #[test]
    fn flake_without_the_user_is_refused() {
        let layout = classify_user_flake(
            HOME_FLAKE,
            Path::new("/root"),
            Path::new("/root/flake.nix"),
            Some("someone-else"),
        );
        assert!(layout.is_err());
    }

    #[test]
    fn generation_links_are_sorted_and_ignore_unrelated_entries() {
        let fs = MemFs::new()
            .with_file("/nix/store/gen1/home-manager", "")
            .with_symlink(
                "/home/tester/.local/state/nix/profiles/home-manager-2-link",
                "/nix/store/gen1",
            )
            .with_file(
                "/home/tester/.local/state/nix/profiles/home-manager",
                "not a link",
            );
        let links = generation_links(&fs, Path::new("/home/tester/.local/state/nix/profiles"));
        assert_eq!(links.len(), 1);
        assert_eq!(links.first().and_then(|link| link.id), Some(2));
        let target = links.first().and_then(|link| link.target.clone());
        assert_eq!(target, Some(PathBuf::from("/nix/store/gen1")));
    }
}
