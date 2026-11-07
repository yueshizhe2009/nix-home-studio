//! `nhs` — command-line front end for nix-home-studio.
//!
//! Subcommands:
//!
//! ```text
//! nhs discover                 what was detected, read-only
//! nhs plan                     render + inject, show the diff, write nothing
//! nhs validate                 plan, then build it in a scratch copy
//! nhs apply [--build-only]     validate, then write (never activates)
//! nhs apply --switch [--yes]   refused in this version (exit 3)
//! ```
//!
//! Exit codes are part of the interface: `0` done, `1` failed, `2` nothing to do,
//! `3` refused by policy. `--switch` is a policy refusal even with `--yes`; the
//! message prints the exact command it would have run, and no process is started.
//!
//! Path flags (`--home`, `--config-home`) exist so a caller can inspect a tree
//! that is not the current user's. Their precedence is exact and deliberately
//! narrow:
//!
//! | Setting | Comes from | `--home` affects it | `--config-home` affects it |
//! | --- | --- | --- | --- |
//! | `$HOME` | `HOME`, else empty | yes | no |
//! | config dir | `XDG_CONFIG_HOME`, else `$HOME/.config` | **only when `XDG_CONFIG_HOME` is unset** | yes |
//! | state dir | `XDG_STATE_HOME`, else `$HOME/.local/state` | **only when `XDG_STATE_HOME` is unset** | no |
//! | `PATH` | process `PATH` | no | no |
//! | store root | `NHS_STORE_ROOT`, else `/nix/store` | no | no |
//!
//! So `--home` is *not* a sandbox: an exported `XDG_CONFIG_HOME` still wins for
//! config discovery, and the store is still the real one. Use `--config-home`,
//! `XDG_STATE_HOME` and `NHS_STORE_ROOT` together for a fully isolated view.
//! `--home` never writes anything.
//!
//! `--json` prints one stable object:
//!
//! ```json
//! {
//!   "schema": 1,
//!   "layout": { "kind": "profile-only", "...": "..." },
//!   "main_file": null,
//!   "generations": [ { "id": 3, "link": "...", "target": "..." } ],
//!   "discovery_attempts": [ { "step": "...", "path": "...", "accepted": false, "outcome": "..." } ],
//!   "toolchain": {
//!     "nix": "/usr/bin/nix",
//!     "home_manager": { "kind": "from-generation", "generation": "...", "binary": "..." },
//!     "nixos_rebuild": null,
//!     "attempts": [ { "strategy": "path_binary", "outcome": "..." } ]
//!   }
//! }
//! ```
//!
//! Exit code is 0 whenever discovery completed, including `Unknown` and
//! `ProfileOnly`; 1 on a usage or internal error.

#![allow(clippy::doc_markdown, clippy::option_if_let_else)]

mod app;
mod search;

use std::path::PathBuf;

use nhs_core::{detect, resolve_toolchain, EnvView, HmInvocation, Layout, RealFs};

const USAGE: &str = "\
usage:
  nhs discover [--json] [--home <path>] [--config-home <path>]
  nhs plan     [--json] [--home <path>] [--module <path>] [--state-dir <path>]
               [--packages a,b] [--set <path>=<json>]...
  nhs validate [same flags as plan]
  nhs apply    [--build-only] [same flags as plan]
  nhs apply    --switch [--yes]        # refused in this version
  nhs --version
  nhs search   <query> [--full] [--json] [--platform <tag>] [--page N] [--per-page N]
               [--include-unfree] [--include-broken] [--include-nixos-only]
               [--license-allow a,b] [--license-deny a,b]
               [--flake-ref <ref>] [--timeout <secs>] [--home <path>] [--state-dir <path>]

exit codes: 0 done, 1 failed, 2 nothing to do, 3 refused by policy";

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(exit) => std::process::ExitCode::from(exit.code()),
        Err(failure) => {
            eprintln!("nhs: {}", failure.message);
            std::process::ExitCode::from(failure.exit.code())
        }
    }
}

fn run(args: &[String]) -> Result<app::Exit, app::Failure> {
    let Some(command) = args.first() else {
        return Err(app::Failure::usage(format!("missing subcommand\n{USAGE}")));
    };
    let rest: &[String] = args.get(1..).unwrap_or(&[]);

    if matches!(command.as_str(), "--version" | "-V") {
        if !rest.is_empty() {
            return Err(app::Failure::usage(format!(
                "`{command}` takes no arguments\n{USAGE}"
            )));
        }
        println!("nhs {}", env!("CARGO_PKG_VERSION"));
        return Ok(app::Exit::Ok);
    }
    if matches!(command.as_str(), "-h" | "--help" | "help") {
        println!("{USAGE}");
        return Ok(app::Exit::Ok);
    }
    if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return Ok(app::Exit::Ok);
    }

    match command.as_str() {
        "discover" => run_discover(rest)
            .map(|()| app::Exit::Ok)
            .map_err(app::Failure::usage),
        "plan" => app::Options::parse(rest)
            .map_err(app::Failure::usage)
            .and_then(|options| app::run_plan(&options)),
        "validate" => app::Options::parse(rest)
            .map_err(app::Failure::usage)
            .and_then(|options| app::run_validate(&options)),
        "apply" => app::Options::parse(rest)
            .map_err(app::Failure::usage)
            .and_then(|options| app::run_apply(&options)),
        "search" => search::SearchOptions::parse(rest)
            .map_err(app::Failure::usage)
            .and_then(|options| search::run(&options)),
        other => Err(app::Failure::usage(format!(
            "unknown subcommand `{other}`\n{USAGE}"
        ))),
    }
}

#[derive(Debug, Default)]
struct DiscoverArgs {
    json: bool,
    home: Option<PathBuf>,
    config_home: Option<PathBuf>,
}

impl DiscoverArgs {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--json" => parsed.json = true,
                "--home" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| "--home requires a path".to_string())?;
                    parsed.home = Some(PathBuf::from(value));
                }
                "--config-home" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| "--config-home requires a path".to_string())?;
                    parsed.config_home = Some(PathBuf::from(value));
                }
                other => return Err(format!("unexpected argument {other}")),
            }
        }
        Ok(parsed)
    }
}

fn run_discover(args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return Ok(());
    }

    let options = DiscoverArgs::parse(args)?;

    let mut env = EnvView::from_process();
    if let Some(home) = options.home {
        env.home = home;
    }
    if let Some(config_home) = options.config_home {
        env.xdg_config_home = Some(config_home);
    }

    let discovery = detect(&RealFs, &env);
    let toolchain = resolve_toolchain(&RealFs, &env);

    if options.json {
        print_json(&discovery, &toolchain)
    } else {
        print_human(&discovery, &toolchain);
        Ok(())
    }
}

fn print_json(
    discovery: &nhs_core::Discovery,
    toolchain: &nhs_core::Toolchain,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "schema": 1,
        "layout": discovery.layout.to_json(),
        "main_file": discovery.layout.main_file(),
        "generations": discovery
            .layout
            .generations()
            .iter()
            .map(nhs_core::GenerationLink::to_json)
            .collect::<Vec<_>>(),
        "discovery_attempts": discovery
            .attempts
            .iter()
            .map(nhs_core::DiscoveryAttempt::to_json)
            .collect::<Vec<_>>(),
        "toolchain": toolchain.to_json(),
    });
    let rendered = serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())?;
    println!("{rendered}");
    Ok(())
}

fn print_human(discovery: &nhs_core::Discovery, toolchain: &nhs_core::Toolchain) {
    println!("layout: {}", discovery.layout.kind());

    match &discovery.layout {
        Layout::FlakeHome {
            root,
            main_file,
            attr,
        } => {
            println!("  root: {}", root.display());
            println!("  main file: {}", main_file.display());
            println!("  attribute: homeConfigurations.{attr}");
        }
        Layout::FlakeNixos {
            root,
            main_file,
            host,
            user,
        } => {
            println!("  root: {}", root.display());
            println!("  main file: {}", main_file.display());
            println!("  host: {host}");
            println!("  user: {user}");
            println!("  note: read-only detection; nixos-rebuild is never run");
        }
        Layout::ClassicHome { main_file } => {
            println!("  main file: {}", main_file.display());
        }
        Layout::ProfileOnly { profile_dir, .. } => {
            println!("  profile dir: {}", profile_dir.display());
        }
        Layout::Unknown { reason } => {
            println!("  reason: {reason}");
            println!("  hint: {}", nhs_core::flow::NO_CONFIG_HINT);
        }
    }

    match discovery.layout.main_file() {
        Some(main_file) => println!("main file: {}", main_file.display()),
        None => println!("main file: (none for this layout)"),
    }

    let generations = discovery.layout.generations();
    if generations.is_empty() {
        println!("generations: (none listed)");
    } else {
        println!("generations:");
        for generation in generations {
            let id = generation
                .id
                .map_or_else(|| "?".to_string(), |id| id.to_string());
            let target = generation.target.as_ref().map_or_else(
                || "(unresolved)".to_string(),
                |path| path.display().to_string(),
            );
            println!("  {id}: {} -> {target}", generation.link.display());
        }
    }

    println!("toolchain:");
    println!("  nix: {}", toolchain.nix.display());
    match &toolchain.home_manager {
        HmInvocation::Binary(binary) => println!("  home-manager: binary {}", binary.display()),
        HmInvocation::FromGeneration { generation, binary } => {
            println!("  home-manager: from generation {}", generation.display());
            println!("    binary: {}", binary.display());
        }
        HmInvocation::FromStore {
            binary,
            version,
            candidates,
        } => {
            println!("  home-manager: from store {}", binary.display());
            println!("    version: {version}");
            if candidates.len() > 1 {
                println!(
                    "    {} other candidate(s) rejected in favour of the highest version",
                    candidates.len().saturating_sub(1)
                );
            }
        }
        HmInvocation::ProfileElement {
            profile_dir,
            binary,
        } => {
            println!("  home-manager: profile element {}", profile_dir.display());
            println!("    binary: {}", binary.display());
        }
        HmInvocation::NixRun { flake_ref } => {
            println!("  home-manager: nix run {flake_ref} (needs network)");
        }
        HmInvocation::Absent => println!("  home-manager: absent"),
    }
    match &toolchain.nixos_rebuild {
        Some(path) => println!(
            "  nixos-rebuild: {} (display only; never executed)",
            path.display()
        ),
        None => println!("  nixos-rebuild: not found"),
    }

    print_attempts(discovery, toolchain);
}

fn print_attempts(discovery: &nhs_core::Discovery, toolchain: &nhs_core::Toolchain) {
    println!("toolchain attempts:");
    for (strategy, outcome) in &toolchain.attempts {
        println!("  {strategy}: {outcome}");
    }

    println!("discovery attempts:");
    for attempt in &discovery.attempts {
        let verdict = if attempt.accepted {
            "accepted"
        } else {
            "refused"
        };
        let path = attempt
            .path
            .as_ref()
            .map_or_else(|| "-".to_string(), |path| path.display().to_string());
        let step = &attempt.step;
        let outcome = &attempt.outcome;
        println!("  {step} [{verdict}] {path}: {outcome}");
    }
}
