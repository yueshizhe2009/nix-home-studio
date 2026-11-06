# Nix Home Studio

A Linux desktop application for people new to Nix and Home Manager: browse a
curated package catalogue, build a Home Manager module, review the exact diff,
validate it in a scratch copy, apply it with a backup, and read what was applied
afterwards.

**What it does not do:** it never activates a configuration (`home-manager
switch`), never touches NixOS system configuration, and does not roll back. The
history screen is read-only because the core has no rollback implementation — the
UI does not pretend otherwise.

## Current state

The desktop application is implemented through milestone M7.2: discovery,
catalogue search and selection, plan and per-file diff, review gate,
asynchronous validation in a scratch copy, build-only apply with snapshots, and
read-only history. It is built on Tauri 2 + WebKitGTK with the business rules in
Rust; the iced and Tauri prototypes under `crates/nhs-gui-spike-*` are kept as
the evidence from the toolkit comparison. M7.3 adds Nix flake packaging for the
CLI and the GUI as two separate derivations.

## Layout

| Path | What lives there |
| --- | --- |
| `crates/nhs-core` | Discovery, catalogue, module generation, edit engine, validation, process control |
| `crates/nhs-app` | Flow and safety gate; no filesystem, no process, no toolkit |
| `crates/nhs-adapter` | The only place a front end touches the world |
| `crates/nhs-cli` | Headless front end used by the test suite and by `nhs discover` |
| `crates/nhs-gui` | The desktop application (own Cargo workspace) |
| `crates/nhs-gui-spike-tauri`, `crates/nhs-gui-spike-iced` | M6.2 prototypes and their measurements |
| `docs/` | Audit, architecture, implementation plan, GUI decision |
| `upstream/` | Read-only clones studied for the audit; **not** part of this repository |

## Nix flake

The CLI and GUI are deliberately separate outputs, so a GTK/WebKitGTK problem
cannot block the command-line delivery:

```sh
nix build .#nhs-cli     # ./result/bin/nhs
nix build .#nhs-gui     # ./result/bin/nhs-gui, .desktop and icon
nix run .#nhs-cli -- discover --json
nix run .#               # the desktop application
```

The GUI output is a wrapped binary that carries the WebKitGTK, GTK and GSettings
environment. It is a Linux package for a Nix environment, not an AppImage or a
distribution `.deb`; a native desktop acceptance run is still required.

## Build and test

Rust workspaces keep their `target/` outside the tree:

```sh
CARGO_TARGET_DIR=/var/tmp/nhs-target cargo test --workspace
CARGO_TARGET_DIR=/var/tmp/nhs-gui-target cargo test --manifest-path crates/nhs-gui/Cargo.toml
```

The desktop application:

```sh
CARGO_TARGET_DIR=/var/tmp/nhs-gui-target cargo run --manifest-path crates/nhs-gui/Cargo.toml
```

Two headless entry points make the behaviour reviewable without a window:

```sh
nhs-gui --check        # the discovery state as JSON
nhs-gui --flow-check   # the whole flow against a throw-away HOME and fake toolchain
```

Everything in one place:

```sh
sh crates/nhs-gui/scripts/check-all.sh
```

## Safety boundaries

- Writes happen only through `nhs-core`'s `EditEngine::commit`; the GUI source is
  scanned to prove it never touches files or starts processes.
- Apply requires a built plan, a successful validation of that exact revision,
  and an acknowledged diff; the gate is re-checked when the confirmation is
  pressed, and the same revision cannot be written twice.
- Validation always runs in a scratch copy; a failed run keeps that copy and
  records the attempt.
- Prototype and end-to-end checks run against temporary HOME/state trees, never
  the user's configuration.

## Licence

Apache-2.0 (`LICENSE`). `NOTICE` records the NixBox-derived code and the rule
that `nix-home-manager-gui` is a design reference only — it carries no licence
and no code from it is included. Provenance for donated modules sits beside the
code in `crates/nhs-core/src/donated/<module>/PROVENANCE.md`.
