# M6.5 — real-environment acceptance

Date: 2026-11-03. Host: CachyOS, niri on Wayland, Rust 1.98, Home Manager
`0-unstable-2026-08-06` resolved from the Nix store, 3 generations, no
`~/.config/home-manager`.

Evidence is grouped by **how it was produced**, because fixtures and the real
machine prove different things:

- **A — real host, read-only.**
- **B1 — real toolchain, throw-away HOME.**
- **Fixture** — automated tests (the existing suites).

## A. Real host (read-only)

| Check | Result | How |
| --- | --- | --- |
| Discovery | `profile-only`, `editable: false`, 3 generations | `nhs-gui --check` |
| Toolchain | `from-store`, `…/bin/home-manager` (nothing on `PATH`), `nix` at `/usr/bin/nix` | `nhs discover --json` |
| Discovery latency | 78–89 ms warm, 593 ms first call | three `--check` runs |
| Window start-up | 741–920 ms (three launches, Wayland native) | poll the compositor until the window appears |
| Memory | 230–248 MiB PSS | `/proc/<pid>/smaps_rollup` after 3 s idle |
| Discover screen | read-only notice, status `This configuration is read-only here.` | AT-SPI |
| Navigation | only the implemented screens enabled; `Confirm` disabled | AT-SPI states |
| History | generations 1–3 with real store targets; `Nothing has been applied from here yet.`; no rollback control | AT-SPI |
| Package search | typing `firefox` returned the curated hit | AT-SPI (keyboard worked during this run) |

Zero-write check, scoped to the paths the application may touch:

| Path | Before | After |
| --- | --- | --- |
| `~/.config/home-manager` | absent | absent |
| `~/.local/state/nix-home-studio` | absent | absent |
| `~/.config` aggregate | — | **hash changed**, but every recently modified file belonged to other running applications (Orca's own logs/state, `yazi`), not to `nhs-gui` |

The aggregate hash is therefore not usable on a live desktop; the meaningful
check is the per-path table above plus the fixture hashing test.

**One write does happen:** the window creates its own WebKit data directory,
`~/.local/share/dev.local.nhs.studio` (108 KiB, 8 files) — the persisted language
choice lives there. That is application data, not the Nix configuration or the
product state directory, and it is recorded here so the boundary is explicit.

Error path: with `PATH=/nonexistent`, `--check` still reports `profile-only` and
no error. The toolchain is only needed for validation; with the fix below it is
resolved from the store rather than from `PATH`.

## B1. Real toolchain, throw-away HOME

Preparation, and what it taught us:

1. `home-manager build -f home.nix` without `NIX_PATH` fails: this host has no
   `<nixpkgs>` channel. Not a product issue, but it means a real validation
   needs a nixpkgs source; the store had one, which had to match the installed
   Home Manager (`lib/services/lib.nix` exists).
2. The minimal config must set `home.username` and `home.homeDirectory`;
   standalone Home Manager refuses without them.
3. With those, a direct `home-manager build` of an empty config succeeded in
   **24 s**, exit 0, generation produced, disk unchanged.

Driven application flow (temporary HOME and state, real toolchain, real build):

| Step | Result |
| --- | --- |
| Discover | `classic-home`, editable |
| Catalogue | empty query lists the curated page; **jq** selected (already in the store) |
| Plan | `home.nix (replace, +1 −1)` plus the module creation |
| Review | acknowledged; validation enabled |
| **Validate** | **success after ~27 s** (`Command: home-manager build -f …/root/home.nix · 27158 ms`, `The build succeeds in the scratch copy.`) |
| Confirm | file list shown; apply enabled |
| **Apply** | module and import written, **one snapshot, `applied`, 2 files** |
| History | snapshot listed; still read-only; no rollback |
| Re-confirm | refused — status `This exact plan has already been applied.` |
| Cleanup | the temporary HOME/state tree is removed; disk unchanged |

### Defect found and fixed

The application could discover a store-only Home Manager but **could not
validate**: `Session::validate` re-resolved `home-manager` through `PATH`, which
on this host has no such binary. The user-facing result was
`validation could not run: home-manager was not found on PATH`.

The fixture suites never caught it because every test put a fake tool on `PATH`.

Fix, in the core and the adapter rather than the GUI:

- `HmInvocation::binary()` exposes the resolved executable.
- The store scan is no longer limited to the “a generation exists” branch: a
  distribution that installs Home Manager without linking a profile still has
  the command in the store.
- `Session::validate` prepends the resolved binary's directory to the runner's
  `PATH`, so the allowlist check still applies and the same binary that was
  discovered is the one executed.

Regression test (written first, failed, then passed):
`validation_uses_the_store_toolchain_when_path_is_empty`.

### Method limitation

Keyboard injection did not work in this session: the X window had input focus,
but the web view never received a focusable element, so `Tab`/typing had no
effect. B1 was therefore driven with AT-SPI actions (click, toggle) and an empty
search; A's search used real typing earlier in the same session. This is a
harness limitation, not an application defect: the accessibility tree accepted
every action, and the same keyboard path is covered by the M6.4 acceptance run.

## Fixture evidence (unchanged)

`crates/nhs-gui/scripts/check-all.sh` still passes: both test suites, clippy,
formatting, the front-end checks, the write-boundary scan, `--check` and
`--flow-check`. This is what makes the behaviour reproducible off this host.

## What remains unverified

- A real **user** configuration: creating `~/.config/home-manager` was not
  authorised, so M6.5 ran against a temporary HOME.
- Network-dependent builds: the test build used packages already in the store.
- Real Wayland HiDPI and screen-reader speech: unchanged from M6.2.
