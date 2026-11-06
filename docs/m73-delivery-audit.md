# M7.3 delivery audit

Date: 2026-11-06. Read-only checks plus one documentation fix; no feature work,
no push, no tag move, no real Home Manager change, no Nix GC.

## 1. Delivery completeness

Commands were run from the repository, not from a prepared shell:

| Command | Result |
| --- | --- |
| `nix flake show --json` | `packages.{default,nhs-cli,nhs-gui}` and `apps.{default,nhs-cli}` on `x86_64-linux` and `aarch64-linux` |
| `nix build .#nhs-cli` | pass; `bin/nhs` |
| `nix build .#nhs-gui` | pass; `bin/nhs-gui`, `.desktop`, icon |
| `nix run .#nhs-cli -- discover --json` | exit 0; empty HOME → `unknown`, 0 generations, no state directory |
| `nix run .# -- --check` | exit 0; real HOME → `profile-only`, 3 generations, `switch.available=false` |
| `nix profile install .#nhs-cli` + `.#nhs-gui` | both install into a temporary profile; `bin/nhs` and `bin/nhs-gui` resolve to the built outputs |
| installed `nhs-gui --check` | exit 0; empty HOME → `unknown`, `switch.available=false` |

The desktop entry is `share/applications/nix-home-studio.desktop`
(`Exec=nhs-gui`, `Icon=nix-home-studio`); the icon is installed at
`share/icons/hicolor/32x32/apps/nix-home-studio.png`.

**Gap found and fixed:** README described `nix build`/`nix run` but not
installation, and did not state the verified/not-verified boundary. It now has
install commands and a short status table. The flake outputs and the README
names agree.

## 2. Safety boundary review

| Boundary | Evidence |
| --- | --- |
| Process allowlist | `Program::ALL = [Program::Nix, Program::HomeManager]` in `crates/nhs-core/src/proc.rs`; no `nix-env` in Rust, JS, Nix or shell sources |
| Process creation | `Command::new` appears only in `crates/nhs-core/src/proc.rs` and test harnesses; `tests/safety.rs` asserts the single spawn site |
| GUI writes | `crates/nhs-gui/tests/readonly.rs` scans GUI sources; only `flowcheck.rs` may write, and it must use the temporary directory and may not spawn |
| History | `history_read_only` reason; `dist/app.js` renders no rollback control; `check-apply.js` asserts **zero buttons** on the History render |
| Rollback code | `nhs_core::rollback` only builds a `RollbackPlan`; no adapter, app, GUI or CLI caller executes it |
| Activation | `--switch` remains a refusal; no `home-manager switch` path in the product |
| Real host | after all checks: `~/.local/state/nix/profiles/home-manager → home-manager-3-link`, `~/.config/home-manager` absent, app state directory absent |

The M7.2 finding still stands: rollback is not atomic and is not exposed in the
UI. M7.3 changed packaging only; it did not widen execution rights.

## 3. Release readiness

**Recommendation: the current candidate is suitable for a technical preview
push**, with the limitations below left visible. Neither the 32×32 icon nor the
absence of deb/AppImage should block Nix users.

Do not claim:

- `aarch64-linux` support — the flake evaluates it, but nothing was built;
- native Ubuntu desktop support — the Ubuntu 24.04 container and Xvfb are not a
  desktop session; the packaged GUI was only opened on CachyOS;
- an installer-friendly standalone binary — the GUI is a wrapped Nix package
  that expects its WebKitGTK/GTK closure in the Nix store;
- rollback support.

Before/at push:

1. Commit the README delivery section and this audit.
2. Keep `m6.4` at `2616f2a`; do not move it.
3. Let the existing GitHub gate run; it does not build the Nix outputs.
4. If a Nix build is added to CI later, build only `nhs-cli` on the runner
   first; the full GUI/WebKit build is too heavy for the current 4-core host and
   would be a poor default for routine CI.

M7.4 should be release preparation (install notes, limitations, optional binary
cache, perhaps a CLI-only CI build), not new features.
