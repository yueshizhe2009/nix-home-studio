# Repository baseline (before the first commit)

This file describes what the first commit should contain, what it must not
contain, and which checks were run. It exists so the initial commit is a
reviewed decision rather than `git add -A`.

Nothing here has been committed: the repository still has no history.

## Scope

The baseline is the source of truth as of the end of **M6.4** (2026-11-02):

- core, app, adapter and CLI crates, with their tests and fixtures;
- the desktop application (`crates/nhs-gui`) including its front end, capability
  file and check scripts;
- the two M6.2 prototypes, kept as the evidence behind the toolkit decision;
- the audit, architecture, plan and GUI-decision documents;
- licence, notice and governance files.

**131 files, 1.5 MiB.** Generated schemas and build output are excluded, so the
tree is all source and documentation.

| Group | Files |
| --- | --- |
| `crates/nhs-core` (code, tests, fixtures, donated modules) | 62 |
| `crates/nhs-gui` (Rust, `dist/`, `capabilities/`, scripts, tests) | 27 |
| `crates/nhs-gui-spike-tauri` | 10 |
| `crates/nhs-gui-spike-iced` | 4 |
| `crates/nhs-app` | 4 |
| `crates/nhs-adapter` | 3 |
| `crates/nhs-cli` | 7 |
| `docs` | 5 |
| root files (`README`, `LICENSE`, `NOTICE`, `Cargo.*`, `.gitignore`, `AGENTS.md`) | 7 |
| `src/donated/README.md` (policy; no code) | 1 |

## Excluded on purpose

| Path | Why |
| --- | --- |
| `upstream/` | Separate read-only clones studied during the audit. They have their own git history and must not be merged into this one. |
| `target/`, `**/gen/schemas/` | Build output. Tauri generates the schemas; they are not source. |
| `result`, `result-*`, `.direnv/` | Nix and direnv artefacts. |
| `*.log`, `node_modules/` | Local junk; the GUI has no npm step. |

## Licence and attribution

- Our code: **Apache-2.0** (`LICENSE`).
- `NOTICE` records that NixBox-derived code is included and that
  `nix-home-manager-gui` is a **design reference only** — it carries no licence,
  and no code from it is in the tree.
- Donated modules sit beside their tests with a `PROVENANCE.md` naming the
  upstream repository, the exact commit (`7bab22d…`) and the original path:
  - `crates/nhs-core/src/donated/nix-log/`
  - `crates/nhs-core/src/donated/nix-search/`
- Runtime dependencies in use are permissively licensed (Tauri: Apache-2.0 OR
  MIT; iced in the prototype: MIT; the remaining crates are MIT/Apache-2.0). No
  GPL/AGPL code is included; the GPL candidate considered during the toolkit
  round was not adopted.
- Both prototypes are our own code and are kept deliberately, not as vendored
  third-party work.

## Checks run before proposing the baseline

| Check | Result |
| --- | --- |
| Secret scan over every candidate file (`api_key`, `secret`, `password`, `token`, private-key headers) | no secrets; the only matches are the `CancelToken` type and the AGENTS rule about secrets |
| Files larger than 200 KiB | none; the largest is a `Cargo.lock` at ~106 KiB |
| Non-text files | only `crates/nhs-gui/icons/icon.png` (32×32 PNG, 110 bytes) |
| Absolute personal paths | none in source or docs |
| Executable bits | none |
| Generated files in the list | none (`gen/`, `target/` absent) |
| Upstream repositories | clean (`git status` empty in both) |
| Real Home Manager configuration / state directory | neither exists on this host |

## Proposed first commit

The commands are for the maintainer to run; this session does not commit.

```sh
cd /home/ysz/orca/projects/nix-home-studio

git add -A
git status --short          # expect exactly the 131 files listed by:
git ls-files --others --exclude-standard

git commit -m "Nix Home Studio: core, app, adapter, CLI and the desktop GUI through M6.4"
```

The current branch is `master`. If `main` is preferred, rename before the first
commit:

```sh
git branch -m main
```

An optional tag marks the accepted milestone:

```sh
git tag -a m6.4 -m "M6.4 accepted: full build-only loop"
```

## What the first commit deliberately leaves out

- No CI configuration yet: the gates live in
  `crates/nhs-gui/scripts/check-all.sh` and can be wired into CI next.
- No packaging: no installer, no desktop entry, no bundled WebKitGTK runtime.
- No settings editor: module options beyond the curated catalogue are still
  out of scope, as is activation/rollback.
