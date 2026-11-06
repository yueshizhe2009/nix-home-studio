# M7.3 — packaging audit and second-environment plan

Date: 2026-11-06. Scope approved by the user: Nix flake packaging for both the
CLI and the GUI, verified separately; podman Ubuntu 24.04 as the second
environment; network allowed only inside that environment. No real Home Manager
configuration is read or written, no Nix GC is run, no generation is deleted and
no rollback UI is added.

## 1. What existed before M7.3

| Area | State |
| --- | --- |
| Flake / lock | none |
| Rust workspaces | root workspace (`nhs-core`, `nhs-app`, `nhs-adapter`, `nhs-cli`); `nhs-gui` is a separate workspace with its own lock |
| Tauri packaging | `bundle.active=false`; no deb/AppImage; `frontendDist=dist` is embedded at compile time |
| Desktop entry | none |
| Icon | `crates/nhs-gui/icons/icon.png`, 32×32, 110 B — a placeholder |
| Runtime linker deps | `libwebkit2gtk-4.1`, `libgtk-3`, `libsoup-3`, `libjavascriptcoregtk-4.1`, glib/gio/gobject, dbus, cairo/pango, wayland/xkbcommon (about 144 `ldd` entries) |

The GUI is not linked into the root workspace. A packaging derivation for it
must use `crates/nhs-gui/Cargo.lock` and build below `crates/nhs-gui`; using the
root lock produces `Cargo.lock is not the same in cargo-vendor-dir`, which was
observed and fixed with `cargoRoot`.

## 2. Flake layout

- `flake.nix`: one `nixpkgs` input, `packages.nhs-cli`, `packages.nhs-gui`,
  `packages.default = nhs-gui`, and matching `apps`.
- `nix/cli.nix`: `buildRustPackage` with the root lock, `cargoBuildFlags =
  [ "-p" "nhs-cli" ]`, no GTK/WebKit inputs. The installed binary is `nhs`.
- `nix/gui.nix`: `buildRustPackage` with the GUI lock, `cargoRoot` and
  `buildAndTestSubdir` both set to `crates/nhs-gui`; `pkg-config`,
  `wrapGAppsHook3`, `copyDesktopItems`; `gtk3`, `webkitgtk_4_1`, `libsoup_3`,
  `glib`, `gsettings-desktop-schemas`, `adwaita-icon-theme`; a `.desktop` entry
  and the 32×32 icon.
- The source set excludes `.git`, `upstream`, `target`, `result*`,
  `node_modules`, `docs`, `README.md`, the agent notes, the two toolkit spikes
  and `*.log`, so the read-only upstream clones and documentation never enter
  the build store.

The two derivations are independent: a WebKitGTK packaging problem cannot stop
`nhs-cli`, and the CLI does not pull GTK/WebKit into its closure.

## 3. Verification matrix

Status categories, kept separate in the final report:

- **Build verified** — `nix build` produced the output.
- **Runtime verified** — the installed binary runs and its dynamic libraries
  resolve.
- **Headless GUI verified** — `--check`, `--flow-check`, and an Xvfb window
  process in the container.
- **Native desktop verified** — the packaged GUI was opened on CachyOS; no native Ubuntu desktop session was available, so the Ubuntu row is not claimed.

| Layer | Host | Ubuntu 24.04 container |
| --- | --- | --- |
| CLI build | pass | pass |
| CLI run / discover | pass (empty HOME and real HOME) | pass (`unknown`; `classic-home`; `profile-only` with 1 generation) |
| GUI build | pass (18 min) | pass (clean store) |
| GUI dynamic libs | pass (163 entries, none missing) | pass (163 entries, none missing) |
| GUI `--check` / `--flow-check` | pass | pass |
| GUI window (Xvfb) | not required | pass (`Nix Home Studio` window present) |
| Native desktop | CachyOS package launch pass; Ubuntu not run | not run |

### Acceptance categories

| Category | Status | Evidence |
| --- | --- | --- |
| **Build verified** | yes | host CLI/GUI and container CLI/GUI `nix build` exit 0 |
| **Runtime verified** | yes | CLI discovery; GUI `--check`; `--flow-check`; profile install; `ldd` |
| **Headless GUI verified** | yes | Xvfb found the `Nix Home Studio` window; no native session claimed |
| **Native desktop verified** | **partial** | the packaged GUI opened on the CachyOS development desktop and History was read-only; no Ubuntu desktop session was available, so the container result is not a native Ubuntu acceptance |

## 4. Host results

| Check | Result |
| --- | --- |
| `nix build .#nhs-cli` | pass; output `/nix/store/…-nhs-cli-0.1.0/bin/nhs` |
| CLI runtime | pass; `ldd` resolves only the Nix glibc/libgcc closure |
| CLI discovery, empty HOME | pass; `unknown`, 0 generations, no state directory |
| CLI discovery, real HOME | pass; `profile-only`, 3 generations, generation 3 at `/nix/store/2ybak…` |
| `nix build .#nhs-gui` | pass after the final source-filter rebuild; output `/nix/store/15bp…-nix-home-studio-0.1.0` |
| GUI dynamic libraries | pass; 163 `ldd` entries, no `not found`; WebKitGTK 2.54.1, GTK3 3.24.52, libsoup 3.8.0, JSC 4.1 resolve from the store |
| GUI `--check`, real HOME | pass; `profile-only`, 3 generations, `switch.available=false`; real profile still `home-manager-3-link` |
| GUI `--flow-check`, scratch HOME | pass; 11 steps, validation `true`, apply `applied`, 2 files written, 1 snapshot, scratch tree removed |
| Desktop entry / icon | installed under `share/applications` and `share/icons/hicolor/32x32` |
| Packaged GUI on the CachyOS desktop | pass; History shows 3 generations with generation 3 `current`, the read-only banner and no rollback control; screenshot `/var/tmp/nhs-m73/real-history-final.png` |
| `nix flake check --no-build` | pass for `x86_64-linux`; aarch64 omitted by the check |

The first GUI attempt failed at the vendor-lock check because the derivation used
the root lock; `cargoRoot = "crates/nhs-gui"` fixed it. No source code changed.

## 5. Environment notes

- Nixpkgs available on the host: 26.11 source, `webkitgtk_4_1` 2.54.1,
  `rustc/cargo` 1.99.0, `wrapGAppsHook3`, `libsoup_3`.
- `nix flake lock` selected `github:NixOS/nixpkgs/02c087f3fe673c9e317f206a627cb6b910336dbe`
  (2026-10-20). GitHub's API rate limit was hit during locking; Nix used its
  cached metadata.
- Network is used only for the container builds and the initial flake lock.
- The GUI's 32×32 icon is installed as-is; a production icon is a design task,
  not a packaging change.

## 6. Second environment

Ubuntu 24.04 is chosen over 26.04 as the mature LTS baseline. The container gets
a fresh single-user Nix installation and an empty HOME; nothing from the host
`HOME` is mounted. The container verifies clean builds, dynamic-library
resolution, CLI discovery, GUI process start under Xvfb, and the temporary-HOME
flow. It does **not** claim a native Ubuntu desktop session.

### Container results

| Check | Result |
| --- | --- |
| Ubuntu 24.04 + single-user Nix 2.35.4 | pass after the installer workaround below |
| `nix build .#nhs-cli` in the container | pass; `/nix/store/643jj…-nhs-cli-0.1.0` |
| CLI run / `discover --json`, empty HOME | pass; `unknown`, 0 generations, no state directory |
| `nix build .#nhs-gui` in the container | pass; `/nix/store/v1n9…-nix-home-studio-0.1.0` |
| GUI dynamic libraries | pass; 163 entries, no `not found` |
| Both outputs installed into a profile | pass; `bin/nhs` and `bin/nhs-gui` |
| GUI `--check`, empty HOME | pass; `unknown`, 0 generations, `switch.available=false` |
| GUI `--flow-check`, scratch HOME | pass; 11 steps, validation `true`, apply `applied`, 2 files, 1 snapshot |
| GUI window under Xvfb | pass; `Nix Home Studio` found in `xwininfo -root -tree` |
| Classic layout fixture | pass; `classic-home`, read-only |
| Profile-only generation fixture | pass; `profile-only`, 1 generation |

The first container attempt exposed two installer requirements: `/nix` must
exist before running the single-user installer as root (otherwise it tries
`sudo`), and Nix's default `build-users-group = nixbld` must have at least one
explicit member. The working setup creates the `nixbld` group, a `nixbld1`
user, adds it to the group, and writes `/etc/nix/nix.conf` with
`experimental-features = nix-command flakes`, `sandbox = false` and the group.
No host configuration is involved.
