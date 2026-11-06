# M7.4.1 — delivery fixes

Date: 2026-11-07. Scope: the six items accepted from the M7.4.0 audit. No
feature work, no rollback UI, no `nix-env`, no real Home Manager change, no Nix
GC.

## 1. Changes

| Item | Change |
| --- | --- |
| GUI `--help` / `--version` | `crates/nhs-gui/src/main.rs` parses exactly one option before GTK starts; `--help`/`-h` and `--version`/`-V` work without a display |
| GUI unknown arguments | unknown arguments and extra arguments are refused with exit code `2`; `--check --bogus` no longer prints JSON |
| Platform metadata | both `nix/cli.nix` and `nix/gui.nix` declare `platforms = [ "x86_64-linux" ]` |
| CLI version | `nhs --version` and `nhs -V` print `nhs 0.1.0`; extra arguments are a usage error |
| README rollback wording | now says core can describe the one-step rollback but nothing executes it and the history screen has no control |
| Version de-duplication | Nix derives both package versions from the relevant `Cargo.toml`; `tauri.conf.json` no longer carries a second copy; only the two Cargo manifests remain |

Added checks:

- `crates/nhs-gui/tests/args.rs` — help/version without display, unknown argument
  refusal, `--check --bogus` refusal.
- `crates/nhs-cli/tests/version.rs` — `--version`/`-V`, extra-argument refusal.
- `crates/nhs-gui/scripts/check-packaging.js` — root/GUI Cargo versions match,
  Tauri does not carry a stale version, Nix files do not hard-code a version and
  declare only `x86_64-linux`.
- `checks.<system>.metadata` in `flake.nix` — evaluates both package metadata
  values and asserts `["x86_64-linux"]` and equal versions.

## 2. Results

| Check | Result |
| --- | --- |
| `cargo test` GUI args | 4 passed |
| `cargo test` CLI version | 2 passed |
| `node crates/nhs-gui/scripts/check-packaging.js` | `version 0.1.0; platforms x86_64-linux` |
| `nix build .#checks.x86_64-linux.metadata` | pass |
| `nix build .#nhs-cli` | pass; `nhs --version` → `nhs 0.1.0` (`/nix/store/3sjj…-nhs-cli-0.1.0`) |
| `nix build .#nhs-gui` | pass; `/nix/store/wsbw…-nix-home-studio-0.1.0`; no missing libraries |
| Nix `nhs-gui --help` / `--version` without display | pass; exit 0 |
| Nix `nhs-gui --check --bogus` | exit 2, no JSON on stdout |
| Nix `nhs-gui --check` (real HOME) | `profile-only`, 3 generations, switch false |
| Nix `nhs-gui --flow-check` (scratch HOME) | 11 steps, 2 files, 1 snapshot, no state directory |
| `nix flake check` | pass (only the existing app-`meta` and formatter-alias warnings) |
| `sh crates/nhs-gui/scripts/check-all.sh` | **352 tests passed** |

## 3. Safety

The changes are argument parsing, metadata and documentation only:

- `Program` still allows only `Nix` and `HomeManager`;
- History still has no rollback control; `check-apply.js` still asserts zero
  buttons;
- `RollbackPlan` still has no caller;
- the real profile remains `home-manager-3-link`; no state directory exists.
