# Upstream audit

Status: complete (phase 0). Audit only — no application code was written and no upstream file was modified.

- NixBox: `upstream/nixbox` @ `7bab22d` (v0.4.5, Apache-2.0, Rust edition 2024, 7-crate workspace, ~39k lines).
- nix-home-manager-gui: `upstream/nix-home-manager-gui` @ `25c2b81` (0.1.0, **no license**, Tauri 2 + React 19, ~5.3k lines).

Conventions used below:

- **[V]** verified by reading the cited source or by running a command; the evidence is given.
- **[I]** inference or design judgement, not verified.
- Paths are repository-relative to the respective clone.

---

## 0. Executive summary

| Question | Answer |
| --- | --- |
| Can NixBox code be reused? | **Partly.** Apache-2.0 → reusable with attribution + NOTICE obligations. Its headless crates (`nixbox-nix`, `nixbox-core`) are genuinely UI-independent and are the best donor. But its *safety* layer is the weakest part: no backup, no pre-write diff of user config, no mtime/hash guard, no atomic write for user files. |
| Can nix-home-manager-gui code be reused? | **No, not now.** No `LICENSE` file, no `license` field in `package.json` or `src-tauri/Cargo.toml` → default all-rights-reserved. Design reference only. This must be resolved with the author before any line is copied. |
| Does either generate Home Manager modules? | Only NixBox. NixBox writes `nixbox-home-packages.nix` in the config root and imports it into the main file with a **text scanner**. nix-home-manager-gui generates nothing and writes only the user's `home.nix`. |
| Does either preserve comments/format when editing **existing** files? | NixBox: **no** (regenerates its own generated files wholesale; scans user files with a hand-rolled lexer). nix-home-manager-gui: **yes**, via `rnix` AST splicing + re-parse — its single strongest reusable *idea*. |
| Does either support HM generations + rollback? | nix-home-manager-gui **yes** (lists `home-manager generations`, activates `<store>/activate`); NixBox **no** (only its own `nix-env` profile). |
| Does either back up and diff before writing? | nix-home-manager-gui has a one-level `.nix.bak` + `similar` unified diff + external-change guard. NixBox has **none** of these for user files. |
| Recommended base | **Rewrite the safety/write path; donate from NixBox (`rnix`-based verification, scan, catalog, nix-log, rebuild runner) and borrow the *ideas* of nix-home-manager-gui's AST splicing, staging workspace, and generation activation.** |

The single most important finding: **the two projects have nearly disjoint strengths and neither has a correct "safe edit → validate → apply → rollback" loop.** Combining them naively would combine NixBox's unsafe writes with a project whose license forbids copying.

---

## 1. License and dependency notes

### 1.1 NixBox

| Item | Finding | Evidence |
| --- | --- | --- |
| License | Apache-2.0 | `LICENSE:2`; `Cargo.toml:22` (`license = "Apache-2.0"`) [V] |
| Obligations | Keep the license, attribute the author, reproduce NOTICE if one exists; state significant modification in changed files (§4b). No NOTICE file in the repo. | Apache-2.0 §4 [V] |
| Trademark | The name "NixBox" is not granted by the license (§6). Our product must not be called NixBox. | [V] |
| Crate shape | 7 crates; policy lives in `nixbox-nix` + `nixbox-core`; all three front-ends own only event/render code | `docs/ARCHITECTURE.md:27-54` [V] |
| Nix parser | `rnix = "0.14"` (rowan transitively) — but **only used in `manifest/migration.rs`** | `crates/nixbox-nix/Cargo.toml:22`; `grep rnix::` → only `migration.rs` [V] |
| Diff crate | **none** | no `similar`/`diffy`/`imara-diff` in any manifest [V] |
| VCS crate | **none** — subprocess `git` / `jj` | `crates/nixbox-core/src/vcs.rs:811-849` [V] |
| Networking | `reqwest` (rustls) for the flake browser + optional telemetry | `Cargo.toml:53` [V] |
| Telemetry | **Present.** Opt-in, but ships an endpoint `https://nixb.duckdns.org:58443/v1/telemetry` | `crates/nixbox-config/src/telemetry.rs`; `docs/CONFIGURATION.md:35` [V] — **must be stripped if any core code is donated** |
| Lint policy | Very strict: `clippy::pedantic`+`nursery` deny, `unwrap_used`, `panic`, `as_conversions`, `indexing_slicing` deny | `Cargo.toml:24-40` [V] — donated code already meets this bar, a plus |

### 1.2 nix-home-manager-gui

| Item | Finding | Evidence |
| --- | --- | --- |
| `LICENSE` file | **absent** | `git ls-files` filtered for license files → empty [V] |
| `package.json` license | not set (`"private": true`) | `package.json:2-4` [V] |
| `Cargo.toml` license | not set | `src-tauri/Cargo.toml:2-6` [V] |
| README license section | none | `README.md:1-27` [V] |
| **Legal effect** | Without a license, default copyright applies: **no permission to copy, modify, or redistribute**. Using it as a *design reference* (reading and re-implementing) is fine; copying files is not. | [I], standard interpretation |
| Frontend | React 19, Vite 8, no test runner | `package.json:12-25` [V] |
| Backend | Tauri 2 (2.12.5), `rnix 0.14`, `similar 3.2`, `rowan 0.16`, `regex`, `dirs 7`, `tokio` | `src-tauri/Cargo.toml:21-31` [V] |
| Tauri security | `security.csp: null` (CSP disabled), capabilities only `core:default` + `opener:default` | `src-tauri/tauri.conf.json:22-24`; `src-tauri/capabilities/default.json:6-9` [V] |

**Action item (blocking for reuse):** ask the author to add a license. Until then the GUI project is reference-only. If we want its `nix_edit.rs` ideas, we re-implement them from the pattern, not from the text.

### 1.3 Dependency risk (both)

- Both shell out to `nix`/`home-manager`; neither links `libnixstore` — good for portability, no C++ ABI coupling [V].
- nix-home-manager-gui's `runner.rs` sets `Stdio::null()` on stdin (`runner.rs:80`), so any prompt (sudo password, "are you sure") hangs or fails. It never uses `sudo` at all [V].
- NixBox's GUI needs the full gpui graphics stack (`Cargo.toml:6-13` excludes it from default members); its CI installs a long list of X11/Wayland dev packages [I from `.github/workflows/publish.yml` job `gui`].

---

## 2. Verified feature matrix

Legend: **Y** implemented · **P** partial/with caveats · **N** absent. Every cell cites source or test.

### 2.1 Package search / catalog

| Capability | NixBox | nix-home-manager-gui |
| --- | --- | --- |
| Search mechanism | `nix --quiet search --json <flake> ^` builds a full catalog, then in-process ranking | `nix search --json` per query, three fallback sources |
| Evidence | `crates/nixbox-nix/src/search.rs:210-252` (command), `:96-130` (local scan), `:70` (`load_or_build`) | `src-tauri/src/nix.rs:287-305`, sources `:259-271` [V] |
| nixpkgs revision pinned to the user's config | Y — follows `flake.lock` `root.inputs.nixpkgs` | P — `--inputs-from <flakeDir>`, falls back to registry/nixpkgs if the flake has no direct input |
| Evidence | `search.rs:254-300`, `catalog_source_from_lock` | `nix.rs:281-283` [V] |
| Cache | Y — `$XDG_CACHE_HOME/nixbox/package-catalog.json`, atomic rename, invalidated by re-reading `flake.lock` | N — re-runs `nix search` every query |
| Evidence | `search.rs:27-38,302-313`; `crates/nixbox-core/src/search.rs:14-16` | — [V] |
| Ranking | tiered relevance (exact→prefix→token→substring→description), shortest-name tiebreak, 200 cap | none (delegates to `nix search`) |
| Evidence | `search.rs:386-498`; tests `ranks_name_matches_before_description_matches`, `exact_match_displaces_early_weak_matches` | `nix.rs:320-341` [V] |
| Filters (license / platform / broken / unfree) | **N — no filters at all** | P — post-hoc platform grouping only |
| Evidence | `search.rs:61-65` (`PackageCatalog` has only `source` + `documents`) | `src/platforms.ts:1-53` (frontend) [V] |
| Pagination | N (hard 200 cap; CLI `--limit` default 30, `commands/search.rs:20-21`) | N |
| Tests | 13 unit + 2 `#[ignore]` live; `matching_disk_catalog_avoids_nix_evaluation` proves the cache path works without nix | 2 unit (`parses_search_results`, `uses_the_users_own_nixpkgs_first`) |

**Judgement.** NixBox's catalog is the more valuable asset: it decouples search latency from nix evaluation and is testable without nix. But it has a hard failure mode — `catalog_source_from_lock` bails unless `inputs.nixpkgs` is a **direct, non-`follows`** input (`search.rs:290-294`). That is very common in real configs. And rebuilding the catalog means one full `nix search` (~256 MiB JSON, minutes cold) [V, documented in `docs/SEARCH.md:13-49`].

### 2.2 Install / remove, and what lands on disk

| Capability | NixBox | nix-home-manager-gui |
| --- | --- | --- |
| "Install" writes declarative config (not `nix-env -i`) | Y | Y |
| Files written | `nixbox-home-packages.nix`, `nixbox-system-packages.nix`, `nixbox-*-settings.nix`, `nixbox-home-integration.nix`, plus **in-place edits to `home.nix` / `configuration.nix` / `flake.nix`** | **only** the single user file from Settings (default `~/.config/home-manager/home.nix`) |
| Evidence | `crates/nixbox-nix/src/manifest.rs:215-234,470-500`; `crates/nixbox-config/src/lib.rs:200-258`; `wiring.rs:18-39` | `src-tauri/src/workspace.rs:57-71`; `src-tauri/src/settings.rs:66-68`; `nix_edit.rs:201-271` [V] |
| Auto-import into user config | Y — `ensure_imported` inserts the managed file into `imports` | N — no `imports` handling anywhere |
| Evidence | `manifest.rs:621-653`; `ImportStatus` `:607-617`; tests `inserts_into_multiline_imports`, `creates_imports_list_when_missing` | `grep -rn imports src/ src-tauri/src/` → none [V] |
| Generation strategy | Two marker-delimited blocks (`# nixbox:packages:start/end`) in a **fully regenerated** file; hand edits inside are lost by design | No generation — edits the user's file in place |
| Evidence | `manifest.rs:390-416` (parse), `:470-500` (render); `docs/MANAGED_FILES.md:5,40` | `nix_edit.rs:1-6` [V] |
| Independence from nix evaluation | The install path itself needs no `nix eval` | Same |
| Alternative "profile mode" | Y — `nixbox profile setup` switches the module to a `buildEnv` + `nix-env --set` on activation, so installs need **no config switch** | N |
| Evidence | `crates/nixbox-core/src/manifest.rs` `PROFILE_MODULE` `:506-535`; `crates/nixbox-core/src/profile.rs:40-74,176-261`; test `only_setup_and_teardown_change_a_targets_mode` | — [V] |

**Judgement.** NixBox's "generated module + marker block + auto-import" model is the right *shape* for a beginner tool and is worth keeping. Its *implementation* is unsafe for user files (see §2.4). nix-home-manager-gui's "edit one file in place" model is safer per-write but does not scale to a curated beginner catalog and has no import awareness.

### 2.3 Module generation and Nix editing

| Capability | NixBox | nix-home-manager-gui |
| --- | --- | --- |
| Real AST parsing | P — `rnix` used **only** in `manifest/migration.rs` (`signature()`, `replace_imports()`) | **Y** — `rnix` throughout `nix_edit.rs` |
| Evidence | `migration.rs:7,179-249,307-388`; `rnix::Root::parse` + token-stream comparison | `nix_edit.rs:1-6,20-26,386-428` [V] |
| Preserves comments / formatting on edit | N for managed files (full re-render, hard-coded tabs `manifest.rs:470-484`); N for user files (text splices) | **Y** — locate node with rnix, splice into byte range, re-parse to validate |
| Comment preservation test | — | `does_not_split_trailing_comments` (`nix_edit.rs:737`) |
| Refuses ambiguous/computed targets | P — migration refuses computed imports (`${`) and string imports; scanner skips complex expressions | Y — returns `Opaque`/`Partial`, refuses to clobber; `refuses_to_clobber_partial_or_opaque_definitions`, `refuses_computed_package_lists` |
| Evidence | `migration.rs:320-338`; `scan.rs:345-399` (`clean_token` rejects `(){}[]"'=$\`) | `nix_edit.rs:386-428,478-502,805,928` [V] |
| Nix keyword/attr quoting | Y — `attr_name()` `wiring.rs:428-443` | Y — `quotes_keys_that_need_it` (`nix_edit.rs:773`) |
| Multi-file awareness | P — config root + main file + generated files; scan is one file at a time | N — one file only; options set elsewhere are read-only |
| Validation after edit | P — migration re-parses with `rnix`; ordinary writes **are not re-parsed** | Y — every edit re-parses |

**Judgement.** This is the clearest split. **Take the `rnix` splice-and-revalidate pattern** (whichever source is licensed), and use NixBox's `signature()` idea — compare the non-trivia token stream so formatting may change but executable content must be accounted for — as the guard for rewriting generated files. Reject the ad-hoc text scanners (`wiring.rs`, `manifest.rs:ensure_imported`, `scan.rs:strip_comment`), which have documented blind spots (`docs/MANAGED_FILES.md:204,245`).

### 2.4 Diff, backup, conflict detection, recovery

| Capability | NixBox | nix-home-manager-gui |
| --- | --- | --- |
| Pre-write diff of the config | **N** — install writes then rebuilds; no preview | **Y** — `similar` unified diff, 3 lines context, rendered in `ApplyPage` |
| Evidence | `session.rs:265-307` (apply → activate, no review step) | `workspace.rs:73-97`; `src/pages/ApplyPage.tsx:41-50` [V] |
| Backup | **N** — `docs/MANAGED_FILES.md:7`: "NixBox does not create backups" | P — single `home.nix.bak`, overwritten each save, no history/restore |
| Evidence | — | `workspace.rs:64-66` [V] |
| Atomic write | P — only `migration.rs:390-421` (`atomic_write`: temp + `sync_all` + rename) and journal writes | **N** — plain `std::fs::write` (`workspace.rs:67`) |
| Conflict detection vs. concurrent user edit | **N** for config writes — unconditional `fs::write` at ~10 sites; no mtime/hash check | Y — full-text comparison before save; refuses with "changed outside the app" |
| Evidence | `grep for mtime, sha256, conflict` in write paths → none | `workspace.rs:43-45,58-63`; test `refuses_to_overwrite_external_changes` (`:131`) [V] |
| Lock against another instance | P — journal `.lock` via `create_new`, held for the whole apply; external editors/VCS do not participate | P — in-process `RunLock` only (`runner.rs:29-41`) |
| Stale-review rejection (a real generation/epoch check) | Y — `vcs.rs:351-356` "configuration changed since review; review again" | N |
| Recovery after crash | P — `state.json` re-runs the *activation* only, never re-applies; journal is written **after** the files | N — no recovery, `.bak` can be overwritten |
| Evidence | `state.rs:32-40`; `session.rs:473-500`; `docs/VERSION_CONTROL.md:45` | — [V] |

**Judgement — highest-risk area.** Neither project has the README's required loop: *stage → diff → backup → content-hash guard → atomic write → validate → explicit apply*. We must build it ourselves. The pieces exist as patterns in both upstreams (`workspace.rs` staging + guard, `migration.rs` `atomic_write` + reverse rollback, `vcs.rs` stale-review rejection) but they are spread out, inconsistent, and in one case in an unlicensed repo.

### 2.5 Home Manager / NixOS generations and rollback

| Capability | NixBox | nix-home-manager-gui |
| --- | --- | --- |
| List HM generations | **N** | **Y** — `home-manager generations` + regex parse |
| Evidence | only `nix-env --profile ~/.local/state/nix/profiles/nixbox --list-generations` (`profile.rs:228-237`) | `src-tauri/src/nix.rs:440-446`; test `parses_generations` (`:481`) [V] |
| Rollback | **N** for HM/NixOS. Only its own `nixbox` profile: `nix-env --profile … --rollback` or `--switch-generation N` (`profile.rs:240-250`) | **Y**, unusual method — runs `<generation>/activate` from `/nix/store` |
| HM generation discovery method | — | `home-manager generations` CLI only |
| Evidence | `profile.rs:240-250`; test `rollback_targets_the_profile_as_its_owner` (`:470`) | `nix.rs:468-474` (path must be under `/nix/store`, no `..`); server-side re-resolution `commands.rs:309-350`; test `only_activates_store_paths` (`:609`) [V] |
| NixOS generations / `nixos-rebuild --rollback` | **N** | **N** |
| Bootable generations (`nix-env -G`) | **N** | **N** |
| UI surface | none (planned only, `docs/NIXOS_SETTINGS_PLAN.md:183,194`) | `GenerationsPage.tsx:11-75` with confirm step |

**Judgement.** nix-home-manager-gui is the only project with a working generations UI, and its security thinking is good (re-resolve the id server-side, refuse paths outside `/nix/store`). But running `activate` directly **bypasses `home-manager switch --rollback`**, so the HM state bookkeeping (the `home-manager` profile pointer) is not updated by the same code path that HM itself uses. **Recommendation:** list generations by parsing the profile symlinks and `home-manager generations` output, but roll back with `home-manager switch --rollback` / `--switch-generation N` so the profile pointer and activation come from one supported command. Treat `activate` as a fallback only, and only with the same `/nix/store` guard.

### 2.6 Build / apply / error handling

| Capability | NixBox | nix-home-manager-gui |
| --- | --- | --- |
| Separate build (no activation) from switch | Y — `home-manager build` vs `switch` | Y — `home-manager build` and `switch` |
| Evidence | `crates/nixbox-cmd/src/apply.rs`; `build.rs:48-112` | `nix.rs:423-438`; `builds_home_manager_args` (`:582`) [V] |
| Structured progress | **Y** — appends `--log-format internal-json -v` and parses activities (`nix_log.rs:13,131-177`) | N — raw lines as Tauri events |
| Cancellation | Y — new process group, `SIGTERM` then `SIGKILL` after 3 s, one terminal event | N |
| Evidence | `build.rs:117-226`; test `cancellation_stops_rebuild_and_emits_cancelled` (`:228+`) | `runner.rs:66-130` [V] |
| sudo for NixOS | Y — `Escalation::{Terminal, Askpass}`, inserts `-A` + `SUDO_ASKPASS` | **N** — never uses sudo, stdin is null |
| Evidence | `rebuild.rs:55-106`; test `askpass_escalation_makes_sudo_ask_through_the_helper` | `runner.rs:80` [V] |
| Error surfacing | exit code only (`"<prog> exited with status N"`), nix's own text relayed verbatim; no classifier | string error with stderr; `run-done{success,code}` |
| Output caps | 256 MiB (catalog) / 50 MiB (live search) / 64 KiB stderr / 1000 log lines | unbounded frontend buffer |
| Test coverage of the real command surface | **none in `cargo test`**; only `just vm-test` (NixOS VM, `vm/test.nix`, `vm/test-script.py`) | **none** — 36 unit tests, zero integration |

**Judgement.** NixBox's build runner is genuinely good engineering (process groups, structured log parsing, bounded I/O, deterministic cancellation tests) and should be the donor for the process layer. Neither has an error *classifier*, which a beginner-facing tool needs.

### 2.7 Tests and CI (verified inventory)

| | NixBox | nix-home-manager-gui |
| --- | --- | --- |
| Unit tests | ~170 across crates (VCS module alone has 21 real-`git`/`jj` tests) | 36 |
| Integration tests | 3 thin binary tests (`crates/*/tests/`) | 0 |
| Real nix in `cargo test` | 0 (2 `#[ignore]` live search, 3 `#[ignore]` network flake tests) | 0 |
| End-to-end | NixOS VM test `just vm-test` (`vm/test.nix`, `vm/test-script.py`, `vm/upgrade-test.py`) — offline, asserts install/remove/flake-wiring/profile-mode | none |
| CI | `.github/workflows/publish.yml` — fmt, clippy `-D warnings`, `cargo nextest`, binary smoke tests, GUI build, `nix build .#checks.x86_64-linux.build-jobs`, crates.io publish on `v*` | `.github/workflows/build.yml` — `pnpm build`, `cargo test --locked`, `cargo clippy -D warnings`, deb/rpm build on Ubuntu 22.04, dmg on macOS |
| Test isolation caveat | Uses real env vars (`NIXBOX_CONFIG_DIR`, `XDG_CONFIG_HOME`) mutated under a `Mutex` and relies on **nextest process isolation**; plain `cargo test` races — **confirmed**: `profile::tests::only_profile_mode_package_changes_skip_the_switch` fails only in parallel (§6.1) | n/a |
| Unrunnable fixture | `real_config_detection` hardcodes `/home/rajveer/...` and is `#[ignore]`d (`scan.rs:832`) | n/a |

### 2.8 Packaging and Linux runtime dependencies

| | NixBox | nix-home-manager-gui |
| --- | --- | --- |
| Distribution | flake packages (`nixbox`, `nixbox-tui`, `nixbox-cli`), desktop entry + icon, crates.io launcher | Tauri bundle `targets: "all"`; CI builds deb/rpm (Ubuntu 22.04) and dmg |
| AppImage | via flake/nix packaging rather than Tauri | not configured |
| `.desktop` / icons | yes (`assets/`) | default Tauri icon set only; no app-specific desktop entry |
| Runtime deps | gpui graphics stack; CI installs X11/Wayland dev libs | `libwebkit2gtk-4.1`, `libxdo`, `libssl`, `libayatana-appindicator3`, `librsvg2` → **WebKitGTK 4.1 runtime** |
| Nix packaging of the app itself | Y (`flake.nix`) | N (single `.nix` file is a test fixture) |
| Self-contained offline build | not verified | not verified |

**Judgement.** For a Nix-adjacent tool, a **Nix-packaged** deliverable is the least surprising and avoids the WebKitGTK runtime dependency entirely. Tauri's advantage is a mature web frontend; its cost is a large C runtime that users of a Nix tool may not have installed.

---

## 3. Reuse classification

### 3.1 Reuse as a dependency (NixBox crates, Apache-2.0)

Viable after a fork/vendor pass that removes telemetry and inverts path resolution:

| Module | Why it's reusable | Required changes |
| --- | --- | --- |
| `nixbox-nix/src/nix_log.rs` (360 lines) | Pure `serde_json`; parses nix `internal-json` activity stream | none besides attribution |
| `nixbox-nix/src/build.rs` runner half (`:117-226`) | process-group spawn, bounded readers, deterministic cancel | caller supplies `BuildEvent` sink instead of owning it |
| `nixbox-nix/src/search.rs` catalog half (`:27-38,254-300,302-326,386-498`) | ranking + snapshot are pure; testable without nix | make `flake.lock` traversal accept `follows` indirection; make cache path a parameter; add filters |
| `nixbox-nix/src/manifest/migration.rs` `signature()` (`:156-249`) | Non-trivia token comparison — the correct guard for "rewrite a generated file, but only if its program is unchanged" | lift out of the migration-specific function |
| `nixbox-nix/src/manifest/migration.rs` `atomic_write` (`:390-421`) + reverse rollback (`:114-141`) | Temp + `sync_all` + rename + restore-on-error | make generic |
| `nixbox-nix/src/wiring.rs` `attr_name` (`:428-443`) + `input_name_for` (`:412-425`) | correct Nix identifier/keyword quoting | keep the functions, drop the rest of the text scanner |
| `nixbox-core/src/vcs.rs` journal/boundary/redaction model | Only design in the set for "what did the app change" | drop the push/create-remote features |

### 3.2 Reusable as design reference only

- **nix-home-manager-gui — everything.** Unlicensed. Ideas to re-implement: AST splice + re-parse (`nix_edit.rs`), staged working copy with full-text compare-before-write (`workspace.rs`), server-side re-resolution of generation ids (`commands.rs:309-350`), render-space detection of dotted vs nested attrpaths (`locate()` `nix_edit.rs:386-428`).
- **NixBox text scanners** (`manifest.rs:ensure_imported`, `wiring.rs` input editing, `scan.rs` external-package scan): the *goal* (find/replace in an arbitrary user file) is right; the implementation is a hand-rolled lexer with documented blind spots. Reference for the test matrix only.
- **NixBox GUI `delta.rs`** (within-line diff emphasis, port of delta's alignment): nice-to-have for a diff view; gpui-bound, port or skip.

### 3.3 Must be written from scratch

1. **The safe write pipeline**: stage → hash → diff → backup → atomic write → re-parse → apply. Neither upstream has it end to end.
2. **HM/NixOS generation model**: one abstraction over classic HM, flake HM, HM-inside-NixOS, and bare `nix profile`. See §4.2 — this is a live portability blocker.
3. **Configuration discovery** (is this config classic, flake, HM-in-NixOS, or standalone profile?).
4. **Error classification** for beginner-facing messages.
5. **Package catalog filters** (license, platform, broken/unfree) and pagination.
6. **Curated beginner catalog / module templates** — neither has one.

---

## 4. Unresolved questions and environment findings

### 4.1 Questions to settle before implementation

| # | Question | Why it blocks | Proposed default |
| --- | --- | --- | --- |
| Q1 | License of nix-home-manager-gui | Cannot copy `nix_edit.rs`/`workspace.rs` | Ask author; meanwhile re-implement the pattern |
| Q2 | Target user's configuration shapes | Every feature depends on it | Support 3 shapes (see §4.2) with explicit detection |
| Q3 | Does the MVP support `nixos-rebuild` (sudo) at all? | Sudo in a GUI needs `pkexec`/askpass; large scope | **No.** User-profile `home-manager switch` only in v1; NixOS shown read-only |
| Q4 | Backend for generations when HM was installed as a `nix profile` element | `home-manager generations` may be unavailable | Detect via profile links + `home-manager` presence; degrade gracefully |
| Q5 | Should we write into the user's file at all, or only into a sidecar module? | Safety vs. usefulness | Write only a sidecar module + one import line; the import insertion is the only touch of a user file |
| Q6 | Ship as Nix flake package, Tauri bundle, or both? | Runtime deps differ wildly | Nix package first (`nix build .#default`), AppImage second |
| Q7 | Catalog size policy | Full `nix search` of nixpkgs is minutes cold and 256 MiB | Curated subset (~300 packages) for v1 + "search all nixpkgs" as an explicit opt-in |

### 4.2 Verified environment findings on this machine (portability blockers)

Observed on the audit host [V]:

| Observation | Command evidence | Consequence |
| --- | --- | --- |
| Nix 2.35.4, flakes experimental features available | `nix --version`; `nix eval --expr '1+1'` → `2` | Base tooling OK |
| **No `/etc/nixos`** | `ls /etc/nixos` → no such file | NixBox's `configuration.nix` / `nixos-rebuild` path is unusable here |
| **No `~/.config/home-manager`, no `flake.lock`** | `ls ~/.config/home-manager` → no such file | NixBox's default config root (`~/.config/nixos`, `nixbox-config/src/lib.rs:316-322`) is wrong here |
| A Home Manager **generation profile exists**: `~/.local/state/nix/profiles/home-manager`, `home-manager-1..3-link` | `ls ~/.local/state/nix/profiles/` | HM was applied and has 3 generations |
| `home-manager` is installed as a **flakes profile element**, not a command on PATH | `nix profile list --json` → `home-manager-path`; `which home-manager` → not found | Anything invoking a bare `home-manager` binary fails. HM must be invoked via its profile path or `nix run` |
| No channels configured | `nix-channel --list` → empty | Channel-rollback features (nix-home-manager-gui `channels.rs`) are irrelevant here |
| Distro is CachyOS (Arch-based), **not NixOS** | `/etc/os-release` | "Nix on a foreign distro" is the primary supported scenario, not an edge case |
| No `cargo` default toolchain | `cargo --version` fails; `rustup toolchain list` → `stable` | Build commands must use `cargo +stable` |

**This is the most consequential finding after licensing.** Both upstreams assume a flake-based, NixOS-like layout; the machine we must support has neither `/etc/nixos` nor a HM flake, and does not even expose a `home-manager` executable. The architecture must therefore treat **configuration discovery and executable resolution as first-class, tested subsystems**, not as hardcoded constants.

---

## 5. Risk ranking

| Rank | Risk | Severity | Likelihood | Mitigation |
| --- | --- | --- | --- | --- |
| R1 | Copying unlicensed code from nix-home-manager-gui | High (legal) | High if rushed | License gate in repo: dependency allowlist; re-implement patterns |
| R2 | Destroying a user's config on a bad write (no atomic write, no backup, no hash guard in either upstream) | High | Medium | Own write pipeline; every write behind stage+diff+backup+atomic+re-parse; never write without the user seeing the diff |
| R3 | Assuming a flake layout / `home-manager` on PATH → tool does nothing useful on the audit host | High | High (observed) | Discovery module with fixtures for the 4 layouts in §4.2 |
| R4 | Accidental `home-manager switch` / `nixos-rebuild switch` | High | Low if gated | Separate build/apply verbs; `switch` requires an explicit confirm dialog + typed confirmation; no auto-apply after edit |
| R5 | Catalog build cost (minutes, 256 MiB JSON) on first run | Medium | High | Curated catalog first; full search opt-in with progress and cancel |
| R6 | `rnix`-based edits still corrupt exotic configs (`let`-wrapped bodies, computed attrs) | Medium | Medium | Refuse + report instead of guessing; token-stream guard before rewrite; fixture corpus of real-world shapes |
| R7 | Telemetry / network code sneak in via donated crates | Medium (privacy) | Medium | Fork with telemetry module deleted; dependency audit; no network features in v1 |
| R8 | Rollback semantics mismatch (run `activate` vs `switch --rollback`) | Medium | Medium | Use HM's own rollback command; `activate` only as guarded fallback |
| R9 | NixBox scan/edit text scanners copied as-is | Medium | Medium | Do not copy; only the goal and the test matrix |
| R10 | No integration test surface for nix itself → regressions shipped | Medium | High | Fixture-based fake `nix` binary in PATH + one optional VM/sandbox test |
| R11 | WebKitGTK dependency unavailable on Arch/CachyOS users | Low | Medium | Prefer Nix-packaged build; document runtime deps |
| R12 | Upstream drift (NixBox v0.4.5 moving fast) | Low | High | Vendor the donated modules at a pinned commit, record the commit in the NOTICE file |
| R13 | Process-global env-var test isolation (NixBox's pattern) hides ordering bugs and fails on real machines | Medium | High (demonstrated) | Inject config roots/paths explicitly; no `env::set_var` in tests; do not depend on nextest process forking for correctness |

---

## 6. What was inspected and what was run

### Inspected [V]

- Both repositories in full at the directory level (`git ls-files`, manifests, CI, tests, `docs/`), plus targeted reads of: `nixbox-nix/{search,manifest,manifest/migration,wiring,scan,build,nix_log,settings}.rs`, `nixbox-core/{engine,session,vcs,rebuild,profile,op,state}.rs`, `nixbox-config/src/lib.rs`, `docs/{ARCHITECTURE,MANAGED_FILES,VERSION_CONTROL,SEARCH,FLAKE_BROWSER}.md`; and `nix-home-manager-gui` `src-tauri/src/{nix,workspace,nix_edit,commands,runner,channels}.rs`, `src-tauri/{Cargo.toml,tauri.conf.json,capabilities/default.json}`, `src/pages/*.tsx`, `.github/workflows/build.yml`.
- License, telemetry, and packaging claims were checked directly, not taken from READMEs.

### Tests actually run

Upstream test suites were executed against this machine to confirm the audit's claims (compiled into `/tmp/opencode/*` target dirs; sources untouched):

| Suite | Command | Result |
| --- | --- | --- |
| nix-home-manager-gui | `cargo +stable test --locked` in `src-tauri` | see §6.1 |
| NixBox | `cargo +stable test --workspace --exclude nixbox --locked` | see §6.1 |

### 6.1 Test evidence log

Executed 2026-10-25 on the audit host (CachyOS/Arch, Nix 2.35.4, `rustc` stable via `cargo +stable`). Target dirs were redirected to `/var/tmp/nhs-audit/*` so the clones were never written to (`git status --porcelain` in both clones is empty after all runs).

| Suite | Command | Result |
| --- | --- | --- |
| nix-home-manager-gui | `cargo +stable test --locked` (in `src-tauri`) | **36 passed, 0 failed** — exactly the 36 unit tests claimed in §2.7. No integration tests exist, as stated. Builds cleanly with the WebKitGTK 4.1 dev libs present. |
| NixBox `nixbox-nix` | `cargo +stable test -p nixbox-nix --lib --locked` | **84 passed, 0 failed, 6 ignored** (the 2 live-search + 3 network flake + 1 hardcoded-`/home/rajveer` tests) |
| NixBox `nixbox-config` | `cargo +stable test -p nixbox-config --lib --locked` | **21 passed, 0 failed** |
| NixBox `nixbox-core` | `cargo +stable test -p nixbox-core --lib --locked` | **99 passed, 6 failed** (parallel) / **100 passed, 5 failed** (`--test-threads=1`) — see findings below |
| NixBox workspace | `cargo +stable test --workspace --exclude nixbox --locked` | Aborts at `nixbox-cli`'s binary test (`aliases_show_help_without_opening_an_interface`, `ENOENT` spawning its own binary via `CARGO_BIN_EXE_*`), so the workspace run is a **harness limitation on this host**, not a product result. The library suites above are the meaningful signal. |

**Environment-driven failures (not product defects) [V].** The 4–5 persistent `nixbox-core` failures all come from the host not looking like the author's machine. The audit's own §4.2 findings predict them:

1. `rebuild::tests::{askpass_escalation…, escalation_leaves_commands_without_sudo_alone, only_a_structured_log_adds_flags}` — assert on `home-manager` argv/sudo wrapping, but no `home-manager` binary exists here (§4.2), so `home_manager_switch_cmd` takes its `nix run nixpkgs#home-manager` fallback and the arguments differ (`left: "nix"` vs `right: "sudo"`).
2. `session::tests::a_failed_setup_does_not_turn_a_package_batch_into_a_system_switch` — same missing-binary cause (`left: Some(Configuration)` vs `right: Some(Packages)`).
3. `vcs::tests::jj_snapshots_and_sibling_workspaces_retain_history_but_commits_reset_it` — `jj` is not installed; its `integration_ready()` guard covers `git`+identity but not `jj`, so instead of `SKIP` it fails with `os error 2`. **This is a small real test-harness defect**, and it weakens my earlier claim that every jj test self-skips.

**One genuine test-isolation defect [V].** `profile::tests::only_profile_mode_package_changes_skip_the_switch` fails **only** under parallel execution (present with default threads, absent with `--test-threads=1`). Cause: `TempConfigDir` (`crates/nixbox-core/src/lib.rs:97-123`) sets the process-global `NIXBOX_CONFIG_DIR` under `ENV_LOCK` and releases the lock with the guard on `Drop`, but tests in `rebuild`/`session` read `builds_vm()`/`Config` concurrently; the mutation and the read are not mutually exclusive for the whole test body. Upstream's CI uses **`cargo nextest`** (`.github/workflows/publish.yml:50`, `justfile:24`), which forks a process per test and hides exactly this class of bug. Consequence for us: **do not copy NixBox's env-var-based test isolation**; inject config paths explicitly (as our architecture §3.1 requires) and use `tempfile::TempDir` with no global mutation.

**Net effect on the audit:** no claim in §2 needs retraction; one needs tightening — the jj test does not reliably self-skip, and the suite is green only in a NixOS/HM-like environment with `jj` installed. This reinforces R3 (environment assumptions) and R10 (no integration tests) and adds a concrete argument for explicit dependency injection over process-global environment state.


### Things deliberately NOT done

Per the project's safety constraints: no `home-manager switch`, no `nixos-rebuild`, no package install/removal, no writes to the user's configuration, no edits to upstream files (only `CARGO_TARGET_DIR` redirected outside the repos), no inspection of secrets or unrelated personal files.

### Not verified [unverified]

- Behaviour against a live Nix + Home Manager configuration (no activation was performed, by design).
- Whether the two upstreams still build on a clean machine inside their own Nix dev shells (only `cargo` builds were attempted).
- The upstream GitHub pages' current state (only the local clones at the pinned commits were examined) — in particular whether a license has since been added to nix-home-manager-gui.
- NixBox's AppImage/desktop integration actually launching on a non-NixOS distro.
- HM behaviour when only a `nix profile`-installed HM exists (the audit host case): whether `home-manager generations` works via the profile path — to be tested in Milestone 1 with a stub, never against the real config.
