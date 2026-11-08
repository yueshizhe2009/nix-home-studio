# Nix Home Studio 0.1.0 — technical preview

Status: **release-candidate notes; not published**. Application code baseline:
`6f0e5c0`; the WP1/WP4 close-out is a follow-up commit on top of it. Remote
baseline: `f977b5a`; the close-out has not been pushed.

## Highlights

- Separate Nix packages for the `nhs` CLI and `nhs-gui` desktop application.
- CLI discovery, planning, validation, build-only apply, and curated package
  search.
- Tauri desktop workflow for package selection, module generation, per-file
  diff review, scratch-copy validation, guarded file writes, and read-only
  generation/snapshot history.
- `nhs` and `nhs-gui` expose `--help` and `--version`; the GUI handles them
  before GTK startup. Unknown GUI arguments are rejected with exit code 2.
- Package metadata declares only `x86_64-linux`; Cargo/Tauri/Nix version drift
  is checked during packaging validation.

## Install

Requires a working Nix installation on Linux. From a checkout of this source:

```sh
nix build .#nhs-cli
nix build .#nhs-gui
nix profile add .#nhs-cli        # Nix 2.35+
nix profile add .#nhs-gui
# Older Nix: use `nix profile install` instead.
```

The CLI and GUI are separate derivations. The GUI is a wrapped Nix package
whose GTK/WebKitGTK runtime is supplied through the Nix store; it is not a
standalone binary bundle.

## Safety boundaries

- The application does not run `home-manager switch` and does not activate a
  configuration.
- NixOS system configuration is detection-only; no `sudo` or
  `nixos-rebuild` path is provided.
- History is read-only. There is no rollback control or rollback execution
  path; `RollbackPlan` only describes the supported previous-generation
  command.
- `nix-env` is not in the process allowlist.
- Apply writes only through the guarded edit engine after validation and
  review; validation runs against a scratch copy.

## Support matrix

| Environment | 0.1.0 status | Evidence / limitation |
| --- | --- | --- |
| `x86_64-linux`, CLI | Supported for technical preview | Nix build, profile installation and discovery verified on the development host and in the M7.3 Ubuntu 24.04 container; version/help verified in the M7.4.2 temporary profile on CachyOS. |
| `x86_64-linux`, GUI | Supported for technical preview | Nix build and runtime dependencies verified; native desktop use verified on CachyOS. M7.4.2 also launched the installed GUI in the active Wayland session. |
| Ubuntu 24.04 native desktop session | Not yet verified | Container build, Xvfb window and headless checks are not equivalent to a native Ubuntu desktop session. |
| `aarch64-linux` | Not supported | Flake evaluation only; no build or runtime verification. Package metadata intentionally excludes it. |
| Other architectures / operating systems | Not supported | Not built or tested. |
| deb / AppImage / Flatpak | Not provided | Install through Nix profiles. |

## Known limitations

- The installed icon is still a 32×32 placeholder.
- There is no standalone `.deb`, AppImage or Flatpak package.
- Native Ubuntu desktop behaviour is unverified; the verified native desktop is
  CachyOS.
- Screen-reader speech and the full 125%/150%/200% scaling matrix have not been
  completely verified (see `docs/m66-accessibility-report.md`).
- The current GitHub Actions workflow runs Rust/frontend/safety gates but does
  not build the Nix flake outputs. Nix build evidence is local/container
  evidence, not CI evidence.
- The isolated M7 end-to-end activation run is test-only evidence, not a
  product capability. The harness deliberately disables user service
  activation and does not validate systemd/D-Bus service activation.

## Verification status for this candidate

- M7.4.1 local gate: **352 tests passed**; CLI and GUI Nix builds passed;
  `nix flake check` passed with the existing app-metadata and formatter-alias
  warnings.
- WP1/WP4 close-out: **355 tests passed**; the Nix CLI was rebuilt and the
  clean-HOME hint was checked. No activation was run.
- WP2 isolation hardening (post-review): `scripts/test-e2e-isolation.sh`
  **69 tests passed** (negative environment, symlink/path escape, missing
  directories, timeout, SIGTERM, cleanup, real-environment snapshots);
  `shellcheck` 0.11.0, `sh -n`, `bash -n`, `git diff --check` and the full
  `check-all.sh` gate passed. CLI and GUI Nix outputs were rebuilt. No
  `--execute`, switch, rollback, D-Bus/systemd access, GC, tag move or push
  in the preflight phase; the separately authorized 2026-11-08 sandbox
  activation is recorded above.
- M7.4.2: fresh temporary HOME/XDG roots and Nix profile; CLI/GUI install and
  command checks passed; `--flow-check` completed 11 steps; packaged GUI
  startup smoke test passed. See `docs/m742-clean-install-acceptance.md`.
- M7 E2E: **run once in an isolated sandbox on 2026-11-08** after explicit
  authorization. `scripts/e2e-home-manager.sh --execute` completed build →
  switch → install `hello` → switch → rollback in `/var/tmp/nhs-e2e.d45fGg`;
  the final profile was `home-manager-1-link`, `hello` was absent after
  rollback, the real profile/config/app state and HM data were unchanged, and
  no systemd/D-Bus command appeared in the logs. The product itself still never
  activates; Gate 8 now has test-only evidence. See
  `docs/m7-wp2-isolation-hardening.md`.
- Remote CI: `f977b5a` is the current remote baseline; GitHub Actions run
  `42367587794` passed in 2m1s with the 352-test gate. Local `8903314`
  (WP1/WP4 plus the initial WP2 preflight) and the WP2 hardening candidate had
  no remote CI result when this document was prepared; the push result is
  reported separately.

The existing non-blocking flake warnings (missing app `meta` and the deprecated
`nixfmt-rfc-style` alias) are known P3 items, not claims of a failed build.
