# Nix Home Studio 0.1.0 — technical preview

Status: **release-candidate notes; not published**. Application code commit:
`6f0e5c0`; the release notes and acceptance records are documentation on top of
that code. Remote baseline: `614f4ee`.

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

## Verification status for this candidate

- M7.4.1 local gate: **352 tests passed**; CLI and GUI Nix builds passed;
  `nix flake check` passed with the existing app-metadata and formatter-alias
  warnings.
- M7.4.2: fresh temporary HOME/XDG roots and Nix profile; CLI/GUI install and
  command checks passed; `--flow-check` completed 11 steps; packaged GUI
  startup smoke test passed. See `docs/m742-clean-install-acceptance.md`.
- Latest remote CI run at the time of writing is on `614f4ee`, not this local
  candidate. It passed the 346-test gate. The `6f0e5c0` candidate has not yet
  been pushed, so it has no remote CI result yet.

The existing non-blocking flake warnings (missing app `meta` and the deprecated
`nixfmt-rfc-style` alias) are known P3 items, not claims of a failed build.
