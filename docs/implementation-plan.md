# Implementation plan

Status: proposal, pending user approval. Nothing here has been started.

Inputs: `docs/upstream-audit.md`, `docs/architecture.md`.

## Approved decisions (2026-10-25)

Binding rules from the user's review. See `docs/architecture.md` §10 for the architectural consequences.

- **D1 Licence red line.** No code from `nix-home-manager-gui` (no licence). Donate only from NixBox (Apache-2.0) with `LICENSE`, `NOTICE`, and per-module `PROVENANCE.md` (repo + commit `7bab22d` + original path). Unclear licensing is excluded outright.
- **D2 No `nixos-rebuild`, no sudo in v1.** Home Manager on a normal Linux distro only. NixOS is detected and displayed read-only.
- **D3 Sidecar module + one import line.** Never rewrite a user file; refuse on ambiguity or external modification; always diff + backup + hash-check first.
- **D4 Curated catalog first.** Full nixpkgs search is explicit opt-in and must not block core work.
- **D5 Primary acceptance env.** This CachyOS host's `ProfileOnly` state first, plus temporary `HOME` and fake Nix/HM commands; a HM flake fixture follows; classic + NixOS-integration modes enter the compatibility plan. No test may touch real user config or run a real switch.

**Milestone gate:** M1 only. Do not start M2, and do not touch a real Home Manager configuration, until the user reviews M1.

## Ground rules

- Every milestone ends with **runnable acceptance tests**, not with "looks right". A milestone is done when the listed commands pass and the evidence is recorded in this file.
- Milestones M1–M4 ship **CLI only**. The GUI is chosen and built at M5, after the core is proven (architecture §2.1).
- Until M6, **no test may touch a real user configuration**. All fixture roots live under `target/test-fixtures/` or a `tempfile::TempDir`.
- No milestone may run `home-manager switch`, `nixos-rebuild`, or `sudo` in CI. M7 is the only milestone allowed to activate anything, and only inside a disposable VM or a scratch `HOME`.
- Donated code is vendored at pinned commits with a `NOTICE` entry (audit §1.1, R12). No file may be added to `src/donated/` without its licence header preserved.
- **No `std::env::set_var` in tests** (audit R13): config roots and paths are injected explicitly.

## Safety gates

| Gate | Enforced at | Rule |
| --- | --- | --- |
| G1 | repository | Dependency allowlist; `deny.toml` (or `cargo-deny`) fails on any crate not explicitly listed. Blocks unlicensed code (R1). |
| G2 | code review | `core` may not `println!`, read `stdin`, or depend on a GUI crate. Enforced by a CI grep + a dependency-direction test. |
| G3 | code | The only function that writes a file is `EditEngine::commit(Plan)`. Enforced by a test that greps the source tree for `fs::write`/`File::create` outside that module. |
| G4 | runtime | `switch`/`rollback` require a confirmation value carrying the literal command string; a `--yes`-style bypass is not implemented in the GUI path. |
| G5 | runtime | Any file write requires an `expected_sha256` (except creation) and a complete backup; missing either → abort. |
| G6 | runtime | The applier re-parses every written file with `rnix` and refuses to leave a file that does not parse. |
| G7 | CI | Dirty-tree check: tests that would activate anything are behind an explicit feature/ignored test and are listed in the test evidence. |

---

## M1 — Skeleton, discovery, and a fake-Nix harness  ← **implemented, awaiting review**

### Status: done (2026-10-25). Do not start M2 before the user reviews this.

#### Delivered

| Area | Files |
| --- | --- |
| Governance (D1) | `LICENSE` (Apache-2.0), `NOTICE`, `src/donated/README.md` (provenance rules; contains no code) |
| Workspace | `Cargo.toml` (`[workspace.lints.clippy]`; deny on `unwrap_used`/`expect_used`/`panic`/`indexing_slicing`/`as_conversions`), `crates/nhs-core`, `crates/nhs-cli` |
| Injectable views | `crates/nhs-core/src/fs.rs` (`FsView`, `RealFs`, `MemFs`), `crates/nhs-core/src/env.rs` (`EnvView` — the **only** `std::env` reader) |
| Layout discovery | `crates/nhs-core/src/discovery.rs` (`Layout`, `GenerationLink`, per-probe `attempts`) |
| Toolchain | `crates/nhs-core/src/toolchain.rs` (`HmInvocation`, `Toolchain.attempts`, display-only `nixos_rebuild`) |
| CLI | `crates/nhs-cli/src/main.rs` — `nhs discover [--json] [--home] [--config-home]` |
| Tests | `crates/nhs-core/tests/{discovery,toolchain,safety,harness}.rs`, `tests/support/mod.rs`, `crates/nhs-cli/tests/cli.rs` |

#### Verified results

| Check | Result |
| --- | --- |
| `cargo test --workspace` | **28 passed, 0 failed** (parallel default; no `set_var`) |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo fmt --all --check` | clean |
| `nhs discover` on this real host | `profile-only`, 3 generations, `home-manager` resolved **from the store**, `nixos-rebuild` display-only |
| Upstream clones | `git status --porcelain` empty in both |
| User config | no `/etc/nixos` or `~/.config/home-manager` created; `home-manager` → `home-manager-3-link` unchanged |
| Executions | zero `nix` / `home-manager` / `nixos-rebuild` / `sudo` runs; `nhs-core` contains no process spawning at all |

#### Revision round 1 (2026-10-25, after review) — status: awaiting re-review

The user withheld formal approval and asked for one small revision, not a redesign. All three items are done; no design changed.

| # | Requested fix | What changed |
| --- | --- | --- |
| 1 | Multi-candidate store selection | `find_home_manager_in_store` → `find_home_managers_in_store`, returning every match sorted **highest version first**, ties broken by ascending store path. `HmInvocation::FromStore` now carries `version` and `candidates`. Previously `sort()` + `pop()` chose the lexicographically largest **hash**, i.e. an arbitrary version. |
| 2 | Unify search order, implementation vs doc | Implementation reordered to match `architecture.md` §3.1: PATH → profile element → generation/store → `nix run` → absent. Strategy names moved into `STRATEGY_ORDER` and a test (`toolchain_attempts_follow_the_documented_order`) fails on drift. |
| 2b | Distinguish real vs alternative layout tests | Tests are now named `real_layout_*` (generation without the command, command in the store — the only shape verified live) vs `alternative_layout_*` (generation carries `bin/home-manager` — fixture-only evidence). Documented in the test module header and in `architecture.md` §3.1. |
| 3 | Fake toolchain actually runs | `harness.rs` now executes the generated scripts: argv recorded verbatim, stdout/stderr captured, exit code preserved, repeated calls appended in order, and the resolver finds a fake `home-manager` on a fake `PATH` via `RealFs`. |
| 4 | `--home` semantics | Documented as a precedence table in the `nhs-cli` module docs, and locked by three CLI tests (`home_flag_moves_derived_config_and_state_dirs`, `xdg_config_home_outranks_the_home_flag`, `config_home_flag_outranks_xdg_and_home`). `--home` is explicitly **not** a sandbox. |

**Verification after the revision**

| Check | Result |
| --- | --- |
| `cargo +stable test --workspace` | **40 passed, 0 failed** |
| `cargo +stable clippy --workspace --all-targets -- -D warnings` | 0 errors |
| `cargo +stable fmt --all --check` | clean |
| `cargo +stable run -p nhs-cli -- discover --json` on this host | `profile-only`, 3 generations, `home-manager` **from store**, 2 candidates, version `0-unstable-2026-08-06` |

**Multi-candidate test results** (acceptance evidence):

| Test | Asserts |
| --- | --- |
| `real_layout_picks_the_highest_version_when_several_stores_exist` | Store has `zzzzzzzz`+2026-08-06, `aaaaaaaa`+2026-09-01, `mmmmmmmm`+2026-08-15. Must pick 2026-09-01. A hash-ordered pick would have chosen `zzzzzzzz` (the oldest), so this test fails on the old implementation. |
| `real_layout_breaks_equal_version_ties_deterministically` | Same version under `aaaaaaaa`/`bbbbbbbb`, listed in both orders → identical winner (ascending path). |
| `real_layout_compares_calendar_and_release_versions` | `25.05`, `26.11`, `0-unstable-2026-08-06` → picks `26.11`. |
| `real_layout_ignores_store_decoys` | `-generation`/`-path`/`-files`/`-source` and `.drv` are never selected. |
| `real_layout_picks_the_highest_version…` also asserts | `candidates.len() == 3` and the winner is first in the list. |

#### Two corrections found during implementation

1. **A Home Manager generation contains no `home-manager` binary.** The original design resolved `<generation>/bin/home-manager`, which does not exist on the target host: the generation's `bin/` holds only `home-manager-generation → ../activate`, and `home-path` is the *package* environment (`hello`, `jq`, `rg`, `man`). The command lives in its own `*-home-manager-<version>` store derivation. Resolution now scans the store; the strategy is named `generation_or_store`. Without this, the tool chose `NixRun` — demanding a network fetch for a binary already on disk.
2. **The first fixtures were unrealistically generous**, planting `bin/home-manager` inside a generation so the wrong design would pass. Fixtures now reproduce the real layout, and a regression test guards it.

#### Deviations from the brief

- `EnvView` gained `store_root`, overridable via `NHS_STORE_ROOT`, so the CLI acceptance test scans a fixture store rather than the host's real `/nix/store`. Default is `/nix/store`.
- Root `Cargo.toml` uses `[workspace.lints.clippy]`; a virtual workspace cannot carry a top-level `[lints]`.

#### Known gaps

- `ProfileOnly` is the only shape verified against a *real* environment. `FlakeHome`, `ClassicHome`, `FlakeNixos` pass against fixtures only (no such config exists on this host).
- Flake detection is a **text heuristic** over `flake.nix`, not an AST parse; M3 replaces it with `rnix`.
- Nothing executes Home Manager in M1, by design.
- The store scan is an O(entries) directory listing; if `/nix/store` is large this should become `nix-store -q --references` in M7.

### Original M1 brief (for comparison)


**Scope.** Workspace with `core` + `cli` crates; the `discovery` subsystem from architecture §3.1; a fake `nix`/`home-manager` binary injectable via `PATH` so every later milestone is testable without Nix. **No file writes, no Nix invocation against real state.**

**Deliverables**
- `cargo` workspace (`nhs-core`, `nhs-cli`), lint config matching NixBox's strictness (`deny` on `unwrap_used`, `panic`, `indexing_slicing`, `as_conversions`).
- Repo governance files required by **D1**: `LICENSE` (Apache-2.0 for our code), `NOTICE`, and the `src/donated/` skeleton with `PROVENANCE.md` template — empty of donated code until M2.
- `discovery` **pure** over an injectable filesystem/env trait (`FsView`, `EnvView`) — no `std::env` reads inside the logic under test, no `set_var` anywhere (R13).
- Layout detection for the four shapes: `FlakeHome`, `FlakeNixos`, `ClassicHome`, `ProfileOnly` (+ `Unknown{reason}`).
- Toolchain resolution with the **five-strategy** `HmInvocation` from architecture §3.1, including **`FromStore`** — the only strategy that works on this host (§10.1). Note the corrected fact: a HM generation contains **no** `home-manager` binary; it is a separate `*-home-manager-*` store derivation.
- `nhs discover --json`: prints detected layout, main file, generation, toolchain strategy, and **every strategy tried with its outcome**.
- Fake toolchain: shell scripts that append their argv to a log and emit canned output; selectable per test.
- Fixtures: fake `HOME` trees for all four layouts, incl. a replica of this host's `home-manager` + `home-manager-{1,2,3}-link` generation structure.

**Acceptance tests**
- `discovery_finds_flake_home_configuration`, `discovery_finds_classic_configuration`, `discovery_finds_home_manager_inside_nixos`, `discovery_reports_profile_only_layout`.
- `discovery_never_guesses_when_two_layouts_exist` → `Unknown{reason}` naming both.
- `toolchain_prefers_path_binary_when_present`.
- `toolchain_resolves_home_manager_from_the_store_when_not_on_path` — **the D5 primary case**: a generation whose `bin/` holds only `home-manager-generation` must resolve via the store scan (also asserts the `home-manager-generation`/`-path`/`-files`/`-source`/`.drv` decoys are rejected).
- `toolchain_prefers_the_command_inside_the_generation_when_present` — a generation that *does* carry the command wins over the store scan.
- `toolchain_resolves_a_profile_element_installation`.
- `toolchain_falls_back_to_nix_run_only_after_all_others_fail`, and `toolchain_reports_absent_with_every_strategy_named`.
- `nixos_rebuild_is_detected_but_never_executable` — asserts no code path builds an argv for it (D2).
- `discover --json` on the `ProfileOnly` fixture exits 0 and names why no config file was found.
- Gate G2 test: `core` has no GUI dependency, no printing, no `sudo` string.
- Gate G3 test: no `fs::write`/`File::create` outside the (not-yet-existing) edit module — in M1 this is asserted as "zero write sites" .
- Purity test: `discovery` module contains no `std::env::var`/`set_var` outside the `EnvView` adapter.

**Evidence:** `cargo test --workspace` + `nhs discover --json` on the real host, captured verbatim (read-only inspection; per D5 this must be re-run rather than trusted from the audit).

**Explicitly not in M1:** any file write, any nix invocation against real state, any donated code, any catalog, any GUI.

---

## M2 — Safe edit engine (the core deliverable)  ← **implemented, awaiting re-review**

> **Gate: M1 was approved before this started. M3 must not start until the user reviews M2.**

### Status: revision 5 done (2026-10-25). 107 workspace tests pass.

**Delivered**

| Area | Files |
| --- | --- |
| Write abstraction | `crates/nhs-core/src/fs.rs` — `WriteFs` trait (create/write/write_new/rename/remove/sync_dir/copy_mode), `RealFs` impl, `MemFs` is now interior-mutable so it can serve as a writable fixture |
| Engine | `crates/nhs-core/src/edit/mod.rs` — `Plan`, `FileChange`, `ChangeKind`, `Applied`, `EditError`, `StateLayout`, `EditEngine::{stage, stage_create, plan, commit, revert}` |
| Hash guard | `edit/hash.rs` (`Sha256`) |
| Diff | `edit/diff.rs` (unified diff + word-level insertion detection via `similar`) |
| Backups | `edit/backup.rs` — content-addressed under the state dir, `restore_all_guarded` refuses to touch content that changed after our write (guarded restore *and* guarded delete; there is no unconditional mode) |
| Lock | `edit/lock.rs` — `create_new`, token-checked release, stale lock reported with holder |
| Snapshots | `edit/snapshot.rs` — `format_version`, `Pending` before writes, `create_new` for the initial record |
| Re-parse | `edit/validate.rs` — `rnix` on every written `.nix` |
| Fixtures | `tests/support/flaky.rs` (generic one-shot fault injection over `MemFs` **and** `RealFs`) |

**Deviations from the brief (all deliberate)**

- `preview` is not a separate method: `stage` returns a `FileChange` that already carries the unified diff and line counts, and `plan` groups them. One representation, no second code path.
- `ChangeKind` is `Create | Replace`. A one-line import insertion is a `Replace`; `FileChange::is_pure_insertion()` derives the "only adds lines" claim from the diff instead of trusting a caller-supplied variant.
- Added `EditError::{ForbiddenTarget, DuplicatePath, InconsistentPlan, NoopNotOnDisk, StaleRevert, InvalidSnapshot, DanglingSymlink, AlreadyExists}` — each is a refusal with a specific reason, per D3.
- `Applied.warning` exists so a commit whose bytes landed but whose snapshot update failed is not reported as an error.

**Acceptance tests — all passing**

| Required test | Actual name / notes |
| --- | --- |
| `plan_produces_diff_without_touching_disk` | same name; also scans the directory for temp files instead of asserting on a hard-coded temp name |
| `commit_refuses_when_the_file_changed_after_the_plan` | same name |
| `commit_writes_atomically_and_leaves_no_temp_files` | same name |
| `a_failure_on_the_second_file_restores_the_first` | same name, plus `a_failure_does_not_touch_files_the_plan_never_reached` which checks the **inode** of the third file on `RealFs` |
| `backups_survive_a_second_apply` | same name |
| `revert_restores_exact_bytes_and_detects_a_corrupted_backup` | split into `revert_restores_exact_bytes`, `revert_refuses_a_corrupted_backup`, `revert_refuses_when_the_file_was_edited_afterwards` |
| `a_second_instance_cannot_commit_while_a_lock_is_held` | same name, plus `revert_takes_the_edit_lock` |
| `commit_reparses_every_written_file_and_refuses_invalid_nix` | same name |
| Gate G3 | `raw_write_syscalls_are_confined_to_the_filesystem_module` (needles, `std::fs` aliases/globs, `Command::new`, `unsafe`) + `the_engine_is_the_only_path_to_a_write` |

Additional: `a_conflict_aborts_every_file_in_the_plan`, `a_failed_commit_removes_files_it_created`, `snapshot_records_the_attempt_with_a_format_version`, `a_state_directory_inside_the_config_tree_is_refused`, `a_plan_that_changes_nothing_writes_nothing`, `staging_a_missing_file_plans_a_create`, `editing_a_symlinked_file_writes_through_the_link`, `a_dangling_symlink_is_refused`, `a_symlink_may_point_outside_the_tree_and_still_be_revertible`, `a_target_outside_the_configuration_tree_is_refused`, `stage_create_refuses_an_existing_file`, `a_plan_with_a_duplicate_target_is_refused`, `a_plan_mutated_after_review_is_refused`, `a_failed_snapshot_update_reports_success_with_a_warning`, `revert_recovers_a_pending_snapshot_after_a_crash`, `revert_rejects_an_id_that_escapes_the_state_directory`, and `tests/edit_real_fs.rs` (8 tests on the real filesystem: mode preservation, symlink-through, non-UTF-8 refusal, created-file revert).

**Two independent adversarial reviews were run.** The first found: `revert` accepting an unvalidated snapshot id (arbitrary file delete), a failed commit restoring files it never wrote, a TOCTOU gap between the hash check and the write, `Err` returned after bytes landed, metadata loss on replace, duplicate targets, no re-derivation of the reviewed plan, no-op entries skipping the hash check, unprotected revert against later edits, and a corrupt-digest fallback. The second round found the residual cases fixed here: `Create` was check-then-rename (now `O_EXCL`), a write that landed but failed read-back was not restored (now recorded before verification), rollback could clobber a newer edit (now hash-guarded per file), and `commit` did not enforce that a plan stays inside the config tree.

### Revision round 2 (2026-10-25, user review) — two post-effect failure holes

The user withheld acceptance again for two cases where an operation *had already taken effect* before it reported an error. Both were real.

| # | Defect | Fix |
| --- | --- | --- |
| 1 | `atomic_replace` returned the `sync_dir` error after a successful rename, so `commit` never recorded the file as written and the rollback skipped a file that had in fact changed. This also contradicted the documented rule that a flush failure is not a write failure. | The rename is now the commit point; the directory flush is best effort inside `atomic_replace`. Durability loss is not reported as a failed write. |
| 2 | A `Create` called `write_new` directly. `write_all`/`sync_all` can fail *after* the file exists, leaving a partial file that `commit` had not recorded, so rollback orphaned it. | `land_one` returns a `LandingError` carrying what the failed write left behind. A failed create is deleted immediately; if that cleanup also fails, the path is still added to the rollback set. *(The delete rule used here was `RestoreGuard::Unconditional`; revision 3 replaced it — see below.)* |

Rollback bookkeeping changed from a path list to **indices of touched files**, with a per-entry guard. *(Revision 2 used `Expect(hash(new_text))` for a replaced file and `Unconditional` for a created one; revision 3 removed `Unconditional`.)*

**New fault-injection capability:** `Fault::{SyncDir, SyncDirInto, WriteNewAfter, WriteNewPartial}`, and `Flaky::with_faults` so one test can arm a compound failure.

**New regression tests — each verified to fail against the pre-fix code (mutation-checked, not assumed):**

| Test | Pre-fix result |
| --- | --- |
| `a_directory_sync_failure_after_the_rename_is_not_a_write_failure` | FAILED (commit reported an error) |
| `a_file_whose_flush_failed_is_still_restored_after_a_later_failure` | FAILED (first file not restored) |
| `a_create_that_fails_mid_write_leaves_no_partial_file` | FAILED (partial file left) |
| `a_partial_create_is_cleaned_up_when_an_earlier_file_was_written` | FAILED |
| `a_create_that_fails_on_the_final_sync_is_still_cleaned_up` | FAILED |
| `a_rollback_does_not_overwrite_a_newer_external_edit` | FAILED (external edit clobbered) |

Mutation check method: revert each fix in place, run the test, confirm red, restore. Both mutations were confirmed to turn exactly the intended tests red.

**Verification after revision 2:** 99 tests pass · clippy 0 errors · `fmt --check` clean · `git diff --check` exit 0 · `nhs discover --json` on this host unchanged (`profile-only`, 3 generations, `from-store`, 2 candidates) · upstream clones and the real Home Manager profile untouched.

### Revision round 3 (2026-10-25, user review) — unconditional delete on a failed `create`

Revision 2 cleaned up a failed `create` by adding the path to the rollback set and deleting it *unconditionally*, on the grounds that the file provably did not exist before the plan. That reasoning is wrong: a path that did not exist before the plan can belong to somebody else by the time the rollback runs. If the immediate cleanup fails and another process claims the path in that gap, the rollback deletes their file. It also contradicted the rule the rest of the engine follows — a rollback may only touch content it can prove it wrote.

**Fix.** `RestoreGuard::Unconditional` no longer exists; there is no "restore/delete regardless" option at all.

- `land_one` now reports a `Leftover`: `Nothing` (cleanup succeeded, or the path is gone), `Ours(Sha256)` (the surviving bytes are a prefix of the planned text, which is exactly what an interrupted sequential write leaves), or `Unattributed` (exists but unreadable, longer, or different).
- A rollback delete happens only under `Expect(Some(those exact bytes))` — the same content guard a replaced file already had. A path that changed since is left alone.
- `Unattributed` is never touched: the path is reported through the new `EditError::Write { residue: Option<PathBuf> }`, and the message tells the user to inspect it before retrying. Leaving a residue is the safe failure mode; deleting an unknown file is not.

**New fault-injection capability:** `Fault::{RemoveInto, RemoveIntoTakenOver, ReadBytesInto}` — the second one fails our cleanup *and* replaces the file with foreign bytes, which is exactly the race being modelled.

**New regression tests (mutation-checked):**

| Test | Under the revision-2 rule |
| --- | --- |
| `a_leftover_taken_over_by_another_writer_is_never_deleted` | FAILED (foreign file deleted; no residue reported) |
| `an_unreadable_leftover_is_reported_instead_of_deleted` | FAILED (file deleted, no residue reported) |
| `a_taken_over_leftover_does_not_block_restoring_earlier_files` | FAILED (foreign file deleted in a multi-file plan) |
| `a_leftover_that_is_still_our_own_partial_write_is_removed` | ok — unchanged behaviour, kept as the control |

Mutation method: restore both old rules (`permits` always true, every leftover treated as ours) → 3 tests red, the control test still green → restore the fix.

**Verification after revision 3:** 103 tests pass · clippy 0 errors · `fmt --check` clean · `git diff --check` exit 0 · `nhs discover --json` on this host unchanged (`profile-only`, 3 generations, `from-store`, 2 candidates) · upstream clones and the real Home Manager profile untouched.

**Residual risk, stated rather than hidden:** a path whose content is byte-identical to our own partial write cannot be told apart from our write by content alone. That is the same standard `Replace` rollback already uses, and it is the strongest proof available without holding an open descriptor across the rollback. Everywhere content differs, the engine prefers a reported residue over an unprovable delete.

### Revision round 4 (2026-10-25, user review) — a failed read was treated as absence

`restore_all_guarded` computed the current state as `exists ? read().ok().map(hash) : None`, so an **unreadable** target produced `None` — the same value as "the file is absent". With an `Expect(None)` guard that passed the guard, and the rollback then deleted a file whose bytes it had never seen. The guard was asking a question it could not answer and treating silence as consent.

**Fix.** The observed state is now three-valued: `Observed::{Absent, Present(Sha256), Unreadable}`. `Observed::Unreadable` satisfies no guard — `permits` has exactly two accepting arms (present-with-expected-bytes, and absent-when-absence-is-expected) and refuses everything else. A refusal is reported per file, other entries still restore, and the message says explicitly that the file exists but could not be read.

The delete branch also acts on the observed state instead of re-probing with `exists`, so the window the guard just closed cannot be reopened between the check and the delete.

**Second defect, same review.** `commit` set `snapshot.error` from the pre-rollback error text and only afterwards attached the real `restore_ok` to the returned error, so a snapshot could claim "every already-written file was restored" when the restore had failed. The final error is now built first and the snapshot records exactly that error's text; a `Failed` snapshot and the returned error cannot disagree.

**New regression tests (mutation-checked):**

| Test | Pre-fix result |
| --- | --- |
| `an_unreadable_target_never_satisfies_an_absence_guard` | FAILED (guard passed, file deleted) |
| `a_file_created_after_the_guard_was_taken_survives_a_failed_read` | FAILED (foreign file deleted) |
| `the_snapshot_error_is_exactly_the_error_that_is_returned` | FAILED (snapshot claimed the restore succeeded) |

Mutation method: collapse `Unreadable` back into `Absent` and re-probe with `exists` before deleting → 2 red; move the snapshot write back before the result is known → 1 red. Both re-run against the final code after the clippy pass.

**Verification after revision 4:** 106 tests pass · clippy 0 errors · `fmt --check` clean · `git diff --check` exit 0 · `nhs discover --json` on this host unchanged (`profile-only`, 3 generations, `from-store`, 2 candidates) · upstream clones and the real Home Manager profile untouched.

**Evidence:** `cargo +stable test --workspace` → 106 passed, 0 failed; `cargo +stable clippy --workspace --all-targets -- -D warnings` → 0 errors; `cargo +stable fmt --all --check` → clean.

### Revision round 5 (2026-10-25, user review) — the check-to-action window

The review accepted revision 4's three fixes and then named the remaining boundary honestly: `observe()` hashes the target and `remove_file()` runs afterwards, so a writer that replaces the path between those two syscalls still loses its file. Revision 4 proved specific failure timings are handled; it did not, and cannot, prove every interleaving is safe.

**Decision: do not redesign the create/cleanup mechanism in v1; state the boundary precisely and lock the narrowest window the API allows.**

Why not a new mechanism:

- The property asked for is atomic compare-and-act on a path. POSIX has no such operation (`renameat2(RENAME_NOREPLACE)` covers creation only, is Linux-specific and is not exposed by `std`); a quarantine-by-rename scheme reintroduces the identical race on the restore step, and descriptor/`(dev,ino)` verification still leaves a gap before `unlink`. Every variant narrows the window; none closes it.
- The same window has existed since M2's first line of code in the *write* path (last hash check → `rename`), where earlier review rounds already settled for "check immediately before the act". Treating delete differently only helps if delete is made *recoverable*, which is a design change with its own failure modes (quarantine naming, crash residue inside the config tree, restore-step races).
- Introducing a new create/cleanup mechanism now, with four review rounds of hardening behind the current one, trades a microsecond theoretical window for a fresh class of practical bugs.

What was added instead:

| Change | Where |
| --- | --- |
| Normative rule 13 + §7.1.1 stating the exact windows (write and delete), the four guarantees that *are* provided, the threat model (a human editor, seconds-to-minutes: detected; an adversary racing syscalls: out of scope), and the v2 options | `docs/architecture.md` |
| Explicit v1 non-goal with the same reasoning, so it is a decision rather than an omission | `docs/implementation-plan.md` § Non-goals |
| An operation log in the fault harness (`Flaky::ops()`), recording every forwarded call in order | `crates/nhs-core/tests/support/flaky.rs` |
| `a_guarded_delete_immediately_follows_its_verifying_read` — asserts the delete is the *next* call after the verifying read, with exactly one probe before it | `crates/nhs-core/tests/edit.rs` |

The new test does not claim atomicity. It locks the only thing under our control: that the window stays one syscall wide, so a future change cannot silently widen it by adding a probe, a lookup or a log line between the check and the delete. Mutation-checked by re-adding an `exists` probe before the delete → FAILED.

**Correction to earlier verification wording:** `git diff --check` was vacuous in these reports, because every project file is still untracked (`git diff` sees no content). The whitespace check used from this round on is `grep -rn "[[:blank:]]$" crates docs --include=*.rs --include=*.md` → 0 matches.

**Verification after revision 5:** 107 tests pass · clippy 0 errors · `fmt --check` clean · whitespace scan 0 · `nhs discover --json` unchanged (`profile-only`, 3 generations, `from-store`, 2 candidates) · upstream clones and the real Home Manager profile untouched.

**Evidence:** `cargo +stable test --workspace` → 107 passed, 0 failed; `cargo +stable clippy --workspace --all-targets -- -D warnings` → 0 errors; `cargo +stable fmt --all --check` → clean.

**Known limitations (documented, not hidden)**

- `sync_dir` is best effort: a failed flush is not reported, because reporting it after the rename would claim the write did not happen.
- A stale lock left by a crash requires manual removal; it is reported with its holder, never auto-deleted.
- Revert restores to the canonical target recorded at stage time. If the user re-points the symlink afterwards, revert writes to the old target and the stale check will normally refuse.
- Ownership and extended attributes are not preserved on replace (permission bits are). Replacing a root-owned file is out of scope for v1 anyway (D2 forbids sudo).
- A failed `create` that cannot be cleaned up leaves a residue the engine will not delete unless the bytes are provably its own. Such a path is reported in `EditError::Write { residue }` and must be inspected by the user; that is deliberate, and the alternative (deleting an unconfirmed path) is worse.
- A target that exists but cannot be read satisfies no guard, so an unreadable file blocks its own restore and is reported rather than overwritten. That is the intended trade: a blocked revert is recoverable by hand, a deleted external file is not.
- Rollback ownership is proven by content, not by file identity: a path whose bytes are byte-identical to our own partial write is indistinguishable from it. Stronger proof would need an open descriptor carried from the failed write to the rollback.

---

## M3 — Module generation and non-destructive import insertion  ← **accepted (2026-10-27)**

**Scope.** Generated sidecar module with marker blocks, `rnix`-based rendering, token-stream guard, single import-line insertion into the user's file, refusal on opaque constructs.

**Deliverables**
- `ModuleSpec → Nix text → rnix parse (must be clean)`.
- Marker-block rewrite guarded by the token-stream comparison (donated `signature()` idea, architecture §4.2).
- `ImportInjector`: locate the real `imports` attribute via `rnix`, handle list/none/`let`-wrapped forms, insert one line, re-parse.
- NixOS-only options are **flagged and refused** for a Home Manager target (see the deviation note below about where the flag lives).
- Fixture corpus of real-world shapes: single-line `imports`, multi-line, no `imports`, `let … in`, `with pkgs;`, string imports, computed imports.

**Acceptance tests**
- `generated_module_parses_and_declares_the_selected_packages`.
- `regenerating_preserves_content_outside_the_markers` — write a comment and a setting outside the markers, regenerate, assert byte-preserved.
- `refuses_to_rewrite_a_managed_file_whose_program_was_hand_modified` — edit inside the markers, assert refusal and untouched file.
- `inserting_the_import_changes_exactly_one_line` — diff line count == 1 for every corpus fixture.
- `import_insertion_is_idempotent`.
- `refuses_opaque_or_computed_constructs_and_says_why` — corpus of 6 negative cases, each producing a specific message.
- `indentation_is_inherited_from_the_existing_file` (no hard-coded tabs).
- `system_only_options_are_flagged_and_refused_in_home_layouts`.

**Approved deviation (2026-10-27) — where the `# system-only` flag lives.** The original deliverable said the flag is carried *into the module* as a comment and surfaced in the diff. That conflicts with the guard hardened in M3.2: a comment inside the markers is refused, because a rewrite would silently drop it. Writing our own comment there would force the guard to accept generator-owned comments and weaken exactly the check that caught the `inherit` bugs. Since D2 forbids writing NixOS modules in v1, the comment would also be dead code. The flag therefore lives in `Plan.notes` / `check_target()` (returned to the caller and rendered in the diff view), not in the file. `system_only_options_are_flagged_and_refused_in_home_layouts` asserts both halves: the options are listed, and rendering/rewriting refuses them for a home target.

**Approved deviation (2026-10-26).** The rewrite guard is a *structural shape check* on the marker block, not the token-stream `signature()` comparison sketched in architecture §4.2. For the grammar this application emits, the shape check is strictly stricter (a signature normalises trivia and can admit token-identical content of a different shape) and it reports a location instead of a hash mismatch. Recorded in architecture §4.2.

**Progress**

| Stage | Status | Evidence |
| --- | --- | --- |
| M3.1 `render()` | done (2026-10-26) | `cargo +stable test -p nhs-core --test module` → 12 passed. Golden byte-for-byte module; byte-order sorting + de-duplication; `Style::detect` for 2/4-space, tab, CRLF and empty input; `${`/quote/backslash escaping that cannot interpolate; 13 invalid-package and 8 invalid-setting refusals, each naming the input and the reason; duplicate setting refused. `ModuleError::RenderedTextDoesNotParse` re-parses every render before it is returned. |
| M3.2 `rewrite()` | done, revised (2026-10-26) | `cargo +stable test -p nhs-core --test module` → **31 passed** (19 new since M3.1: 16 in the first round, 3 in the round-2 fix; workspace total 138). Marker location is line-based **and** cross-checked against real `#` comment tokens, so marker text inside a string is refused. Blocks are spliced out by byte range: prefix, the end-marker line, the gap between blocks and the suffix are copied verbatim — `regenerating_preserves_content_outside_the_markers` asserts the prefix/suffix and the surrounding comments; a final-newline test proves neither an existing nor a missing trailing newline is invented. Shape guard refuses a computed element, an extra binding, a non-scalar value and any interior comment, each with the 1-based line. Duplicate, missing, out-of-order and non-comment markers are separate errors. CRLF, tab-indented and one-space files keep their own conventions; an up-to-date file is a byte-identical no-op. **Review round 2 (same day):** the first version validated only the entries it collected as bindings, so `inherit (pkgs) hello;` inside either block was accepted and then silently dropped. Fixed by whitelisting *all* direct entries of the block's attribute set: anything that is not a plain `NODE_ATTRPATH_VALUE` is refused with its line (`refuses_inherit_in_the_managed_block`, `refuses_inherit_in_the_settings_block`, `refuses_a_multiline_inherit`). Mutation: turn the refusal back into `continue` → exactly those three tests red. Five mutations in total (drop the element check, regenerate instead of splice, trust the line scan, skip the comment refusal, skip unknown entries) each turn exactly the intended test red. |
| M3.3 `inject_sidecar()` | done, revised (2026-10-26) | `cargo +stable test -p nhs-core --test import` → **20 passed**; workspace total **158**. 14 fixture files under `tests/fixtures/imports/` cover single-line, empty, multi-line, no-`imports`, `let … in`, `with …;` shapes plus 7 negative ones. The inserted line is the *only* change: the test reconstructs that by removing/undoing one line and requiring the rest to match exactly, and five shapes are additionally asserted byte-for-byte. Idempotent, including an equivalent spelling (`./modules/./x.nix`); existing text is returned untouched, never reformatted. Refusals: `imports` not a list (computed / `++` / a bare path), a computed or string element, duplicate `imports`, `imports` inside a nested attribute set, a top level that is not a module, a file that does not parse, and an injectable path that is not a plain path. Each carries its 1-based line. Four mutations (ignore "already imported", accept computed elements, drop the nested check, force multi-line insertion) each turn exactly the intended tests red. **Review round 2 (same day):** `inherit imports;` is not an `AttrpathValue`, so a scan that only looked at bindings concluded "no imports here" and added a second binding — a module that no longer evaluates. Both scans now also inspect the *names* of `inherit` entries (`inherit imports;`, `inherit (pkgs) imports;`, multi-name lists, nested `config = { inherit (pkgs) imports; }`), refusing with the line: `refuses_imports_brought_in_by_inherit`, `refuses_an_inherit_with_a_source`, `refuses_imports_in_a_multi_name_inherit`, `refuses_an_inherited_imports_in_a_nested_attribute_set`. The name list is read from direct children only, so `inherit (lib.imports) hello;` is *not* treated as a declaration and stays injectable (`an_identifier_named_imports_inside_the_source_is_not_a_declaration`) — mutation-checked in both directions (drop the check → 3 red; naive descendant scan → the false-positive guard red). |
| M3.4 system-only + close-out | done (2026-10-27) | `Target::{Home, Nixos}` with `Target::for_layout` (only `FlakeNixos` maps to `Nixos`), `system_only_paths()`, `check_target()`; `render` and `rewrite` take the target and refuse a NixOS-only option for a home target *before* touching the file or rendering anything. Tests: `system_only_options_are_flagged_and_refused_in_home_layouts`, `a_nixos_target_reports_instead_of_refusing`, `layouts_map_to_the_module_system_they_would_be_written_for`, `a_home_only_spec_needs_no_notes`. Mutations: accept in Home → refusal test red; disable the flag scan → both system-only tests red. Module tests 35; workspace total 162. |

**Evidence:** `cargo test -p nhs-core module import_`

---

## M4 — Validation without applying, and the CLI apply flow  ← **accepted (2026-10-28)**

> **Approved direction with three constraints:** `--switch` is disabled for this milestone, process
> execution is confined to `proc.rs` by the type system, and validation only ever runs against an
> isolated scratch copy.

**Normative invariant (replaces "core never spawns a process")**

> The core library does not interact with the user, does not read stdin and has no GUI dependency.
> Every child process must be created and managed through `proc.rs`; the executable is restricted to
> an explicit allowlist, arguments are built by internal logic and never interpreted by a shell.
> Configuration validation only ever runs against an isolated scratch copy and must not modify the
> real configuration or activate the system. Any change that widens execution rights must update the
> safety gates and their tests.

**Decisions taken (2026-10-27)**

| # | Decision |
| --- | --- |
| 1 | Cancellation kills the process **group** via `libc::kill`. This is the only `unsafe` in the project: confined to `proc.rs`, every block preceded by a `// SAFETY:` line, gate-checked. |
| 2 | Sync `Runner` with two reader threads and an `mpsc` channel; no Tokio. The donated runner is rewritten to std and the rewrite is recorded in `PROVENANCE.md`. |
| 3 | Scratch copy at `<state_dir>/scratch/<id>/`: deleted on success, kept on failure with its path in the error. `.git` excluded, symlinks **dereferenced** so nothing inside the scratch can write through to the real tree, more than 20k files or 2 GiB refused. |
| 4 | validate argv: flake → `nix build --no-link --dry-run .#homeConfigurations.<attr>.activationPackage`; classic → `home-manager build -f <home.nix>`. cwd is the scratch copy; no `NIX_*` is injected. |
| 5 | No real `nix` run in tests. The error corpus comes from NixBox's fixtures and the Nix documentation, each entry citing its source. |
| 6 | CLI as tabulated below. `--switch` is refused outright in this milestone. |

**CLI (frozen)**

| Command | Steps | Output | Exit |
| --- | --- | --- | --- |
| `nhs plan` | render/rewrite sidecar → inject import → stage → plan | unified diff + notes, nothing written | 0 changes, 2 nothing to do, 1 error, 3 policy refusal |
| `nhs validate` | plan + materialise in scratch + build dry-run + classify | verdict + "what to do" hint, real tree untouched | 0 ok, 1 failed, 2 nothing, 3 refused |
| `nhs apply` / `nhs apply --build-only` | plan → validate → commit | snapshot id, files written, "nothing was activated" | 0 applied, 1 failed, 2 nothing, 3 refused |
| `nhs apply --switch [--yes]` | **refused** (`--yes` does not unlock it), before the environment is read | the two candidate command forms and a pointer to `nhs discover`; **no configuration is read to decide which applies** | 3, runner never called |

Flags: `--home <dir>`, `--packages a,b`, `--set <path>=<json>`, `--module <path>`, `--state-dir <dir>`, `--json`.

**Safety gates**

- **G3 (rewritten).** `Command::new` / `process::Command` / `tokio::process` may appear only in `proc.rs`, and that file has exactly one spawn site. The allowlist is a type: `CommandSpec` carries `Program::{Nix, HomeManager}`, so no other executable can be expressed; `Program::from_name` is the only name→program path and rejects everything else. `unsafe` may appear only in `proc.rs`, and the number of `unsafe` blocks equals the number of `// SAFETY:` lines.
- **G4.** The switch argv builder refuses without an explicit confirmation token, and the CLI refuses `--switch` even with `--yes` without ever calling the runner.
- **G7.** The validate argv is `["build", …]` and contains no `switch`.

**Stages**

| Stage | Content |
| --- | --- |
| M4.1 | **done (2026-10-27).** `proc.rs`: `Program` (closed enum, `from_name` is the only name→program path), `CommandSpec` (structured args, cwd, timeout, kill grace, `display()` for "would run" messages), `CancelToken`, `Runner`/`RealRunner`, two reader threads + `mpsc`, `SIGTERM`→`SIGKILL` on the process group, `try_wait` reaping. `EnvView::find_on_path` resolves the program from recorded data, so tests never mutate the real `PATH`. G3 rewritten: process creation only in `proc.rs`, exactly one spawn site, `unsafe` only there and only around `libc::kill`, `unsafe` count == `SAFETY:` count, stdin forced to `/dev/null`. Tests (`cargo +stable test -p nhs-core --test proc` → 9 passed): closed allowlist, cwd + argv, failing status with stderr, missing program, **streaming proven by a ≥300 ms gap between two lines**, cancellation and timeout killing a real grandchild, normal exit reaped, cancelled run keeps its output. Mutation: signal the child instead of `-pid` → both group tests red. Workspace total 172. |
| M4.2 | **done (2026-10-27).** `crates/nhs-core/src/donated/nix-log/{nix_log.rs,PROVENANCE.md}`: the upstream parser is copied verbatim (upstream commit `7bab22d`, `crates/nixbox-nix/src/nix_log.rs`), `rustfmt`-ed, with one doc comment reworded so gate G3's banned program name does not appear in `src/`; the upstream unit tests are kept and pass. `classify.rs`: `ErrorKind` (14 stable labels), `Diagnosis { kind, summary, hint, location }`, `diagnose(&Outcome)` and `classify_stderr()`. Structured `@nix` messages win over text, ANSI is stripped, `error:` is removed from the summary, hints are static advice. Tests (`--test classify` → 7): 13-case corpus with per-case provenance (one verbatim donor capture citing the commit, twelve labelled `Constructed` — no `nix` was run), ordering (`builder failed … no such file` is a build failure), cancellation/timeout as `Interrupted`, success → `None`, unique labels, plain-stderr ANSI. Mutations: drop structured preference → red; drop ANSI strip → red; ignore `cancelled` → red. Workspace total 184. |
| M4.3 | **done (2026-10-27).** `validation.rs`: `validate()` / `validate_with_limits()` with `Limits { max_files: 20_000, max_bytes: 2 GiB }`. Flow: create `<state>/scratch/<id>/root` → copy the tree through `WriteFs` **dereferencing every symlink** (cycle- and depth-guarded, `.git` skipped, non-regular entries refused) → translate the plan onto the copy and apply it with the same `EditEngine` that will apply it for real → build (`nix build --no-link --dry-run .#…` for flakes, `home-manager build -f …` for classic) with the copy as cwd → success removes the copy, failure keeps it and writes `<state>/validations/<id>.json`. `fs.rs` gained `FileMeta`/`FsView::metadata` and `WriteFs::remove_dir_all` so the scratch code never touches `std::fs` (G3 unchanged). Tests (`--test validation` → 11, real filesystem in temp dirs, scripted runner): copy-not-live-root (build cwd under state, planned content visible to the build, live bytes identical), failure keeps the copy + writes a record naming it + lists planned paths, symlink materialised so the real outside file is untouched, file-count and byte limits refused with no build and no leftover tree, unsupported layout refused, argv is `build` and never `switch` for both layouts, success leaves no record or scratch. Mutations: skip the success cleanup → 2 red; drop the failure record → red; skip symlinks while copying → the symlink test red. Also fixed here: an M1-era assertion in `tests/toolchain.rs` still demanded "no `Command` anywhere" (M4.1 only updated `safety.rs`), so it now asserts the allowlist instead. Total workspace 196. |
| M4.4 | **done (2026-10-28).** CLI: `plan`, `validate`, `apply [--build-only]`, `apply --switch [--yes]` (refused). Flags `--home`, `--packages`, `--set path=<json>`, `--module`, `--state-dir`, `--json`. Exit codes `0/1/2/3` (done/failed/nothing/refused). `apply` = plan → validate in the scratch copy → commit; a failed build writes nothing. `--switch` is refused **before the environment is read and before any path is looked at**: no `detect`, no file read, no process (`--yes` does not unlock it; `--yes` alone is a usage error). The refusal text is static and names both candidate commands (`--flake .#<attr>` and `-f <main file>`) plus a pointer to `nhs discover`, because an exact command would require knowing the layout — that is, reading the configuration the refusal promises not to touch. **Review round 2 (2026-10-28):** the first version called `detect()` to print the exact command, which contradicted that promise; `switch_is_refused_without_reading_the_filesystem` now asserts byte-identical output for a real configuration and for a non-existent `HOME` (mutation: put `detect()` back → red). Also fixed: `--state-dir` used to be treated as an `XDG_STATE_HOME` parent, so `--state-dir /tmp/x` landed in `/tmp/x/nix-home-studio`; it is now the final state directory (`state_dir_flag_is_the_final_state_directory`, mutation-checked). `switch.rs` adds the G4 type: `switch_command` cannot be built without a `Confirmation`, and gate G4 in `tests/safety.rs` asserts the CLI never contains `Confirmation::grant` or `switch_command(`. Tests: `tests/apply.rs` 14 end-to-end (fake `home-manager` on a private `PATH`, argv logged), `tests/switch.rs` 5 (no confirmation → no command; build and switch argv differ; preview needs no token), safety gate 6. Mutations: un-refuse `--switch` → 2 red; drop `--yes` check → red; skip validation before commit → red; let `switch_command` pass without a token → red. Workspace total 216. |

**Acceptance tests**

- `validate_runs_against_a_scratch_copy_and_never_the_live_root` — the recorded argv's cwd is a temp dir containing the planned content, and the live fixture is byte-identical afterwards.
- `a_failed_build_leaves_files_in_place_and_records_a_failed_snapshot`.
- `cancellation_stops_the_child_process_group` — a fake toolchain spawning a sleeping grandchild; both are gone.
- `a_timeout_stops_the_child_process_group` and `a_normal_exit_reaps_the_child` — the same guarantee for the other two ways a run ends.
- `exit_status_and_stderr_are_classified_into_a_known_error_kind` — golden corpus with cited sources.
- `switch_without_confirmation_is_refused` (gate G4) and `switch_is_refused_entirely_in_this_version`.
- `build_and_switch_are_distinct_subcommands_with_distinct_argv`.

**Known limitations (M4, stated rather than hidden)**

- `Ctrl-C` is not translated into cancellation yet. The child runs in its own process group (needed so a timeout can take builders down), so a terminal interrupt reaches the CLI but not the build; the build finishes unobserved. Timeouts still work. Cancellation from the UI is wired in M6.
- `validate` runs `nix build --dry-run`: it evaluates and plans, it does not build the derivations, so a build-time failure is not caught until a real `home-manager build`. That is the deliberate cost of not running long builds from a validation button.
- A flake whose evaluation reads `self.rev` will not validate from a copy, because `.git` is not copied. Reported as a build failure; the copy is kept for inspection.

**Non-goals for M4:** running real `nix` in tests, activation/`switch`, `nixos-rebuild`, `sudo`, the catalog (M5), the GUI (M6), generation rollback (M7).

## M5 — Package catalog: curated tier, then optional full tier  ← **implemented 2026-10-29, awaiting review**

> **Gate: each stage is accepted separately. M5.2–M5.4 are not authorised in advance.**

**Decisions taken (2026-10-28)**

| # | Decision |
| --- | --- |
| 1 | Curated catalogue ships **~120 entries** whose metadata can be defended. `license: Option<String>`; when unknown it is `null` and displayed as `unknown`. No licence is guessed to reach a count. The shortfall against the 200–300 target is recorded here as a **data** task. |
| 2 | Default filters exclude `unfree`, `broken` and entries that declare only other platforms. Explicit flags turn each back on. `license = null` is **shown**, not hidden: unknown is not a denial. |
| 3 | Ranking is donated: `relevance_fields` + `token_position` from `nixbox-nix/src/search.rs` (commit `7bab22d`) into `crates/nhs-core/src/donated/nix-search/` with `PROVENANCE.md`; tiers preserved (exact 0, prefix 1, token 2, substring 3, description 4, rest 6); the Tokio removal is recorded. |
| 4 | Revision resolution: explicit `--flake-ref` → the layout's `flake.lock`, **following `follows` indirection** → unqualified `nixpkgs`. The last step is **not a pinned revision and is labelled as such** in the UI and in the snapshot header. |
| 5 | CLI: `nhs search <query>` is offline (curated) and instant; `nhs search <query> --full` builds or reuses a snapshot, writes progress to stderr. `--json` is stable on both paths. |
| 6 | Production cap 300 MiB, **injectable**; the test uses a 1 MiB cap against a 2 MiB canned stream, asserting the failure and that the process group is gone. No 300 MiB fixture is generated. |

**Normative details**

- Platform matching and snapshot reuse are specified in architecture §5.1 and §5.2 (unknown platform ⇒ shown; snapshots reused only on matching format version + recomputed revision hash + `complete: true` + entry digest/count).
- A capped, cancelled, non-zero or unparseable full-tier run writes **no** snapshot.

**Stages**

| Stage | Content |
| --- | --- |
| M5.1 | **done (2026-10-28).** `crates/nhs-core/data/catalog.toml`: **129 entries** across all 12 categories, `include_str!`-embedded. `catalog.rs`: `Catalog::{builtin,from_toml}` with load-time validation (format version, non-empty, plain attr paths, known category, platform tags, duplicate attrs), `HostPlatform::{from_os_arch,from_tag}`, `PlatformSupport`, `Filters` (unfree/broken/nixos-only/platform/licence allow-deny with `resolve_target`), `search()` returning a `Page` with clamped paging. Ranking is the donated `nix-search/rank.rs` (tiers unchanged). Two behaviours found while testing: a `Filters.platform` override that `accepts()` ignored (now resolved once by `resolve_target`), and non-matching entries being returned at tier 6 (a real query now drops them; an empty query still browses everything). The G3 gate also caught `std::env` inside a doc comment, so `HostPlatform::current` was removed rather than exempted. Tests (`--test catalog` → 7): offline load + exact-name-first + non-decreasing tiers, unfree/platform/broken/allow-deny/unknown-licence-and-platform, family and `unix` aliases, paging stability and disjointness with clamping, shipped-data shape and category coverage, seven malformed catalogues, empty result. Mutations: drop the non-match filter → 2 red; treat unknown platform as unsupported → 2 red; sort by name only → ranking red. Workspace total 229. |
| M5.2 | **done (2026-10-28).** `revision.rs`: `resolve(explicit, lock_text)` → `Resolution { kind, reference, revision, pinned }`; `cache_key(format_version)` = `sha256(reference ‖ 0 ‖ version)`. Order: explicit ref (pinned only when it names a commit, `?rev=` or a 40-hex segment) → `flake.lock` → unqualified `nixpkgs` with `pinned: false`. The lock parser follows the root's `nixpkgs` input through all three shapes a lock uses for an indirection (node id, path array, `{"follows": "inputs.a.b"}`), with a value-fingerprint cycle guard; only `github`/`gitlab` locked nodes are accepted (a `path` node is refused, not guessed). Tests (`--test revision` → 7): the acceptance fixture in three forms resolves to the **followed** node's rev and owner/repo (NixBox bails on the indirection); explicit ref beats the lock and reports pinned/unpinned correctly; no lock ⇒ unpinned default; cache key changes with ref, rev and format version; six malformed locks each refused with the right variant; a one-hop indirection is fine while a self-referential path is a cycle; a real version-7 lock still resolves. Mutations: refuse arrays → 3 red; `pinned: true` always → 1 red. **Review round 2 (2026-10-28):** `pinned_revision` matched the substring `rev=`, so `?notrev=…` was read as a pin; it now splits the query on `&` and compares the key exactly (`pinned_detection_only_accepts_a_real_rev_parameter`, 6 cases + a short-hex case). `path_from` dropped non-string elements of a `follows` path with `filter_map`, silently retargeting the path; it now refuses with `InvalidLock` (`a_follows_path_with_a_non_string_element_is_refused`). Both mutation-checked. Workspace total 238. |
| M5.3 | **done (2026-10-28).** `fullcatalog.rs`: `Limits { max_output_bytes: 300 MiB, max_entries: 400_000, timeout: 30 min }`, `search_command()` (`nix search --json --extra-experimental-features "nix-command flakes" <ref> ^`), `parse_entries()` (strips `legacyPackages.<system>.` / `packages.<system>.`, sorts and de-duplicates by attr), `Snapshot { format_version, reference, revision, pinned, revision_hash, complete, entries_count, entries_sha256, entries }`, `Snapshot::is_usable_for()` implementing architecture §5.2 in full, and `build()` which reuses a valid cache, otherwise runs the command through `Runner`, cuts the stream at the cap, and stores **only** on success via `write_new` + `rename`. Tests (`--test fullcatalog` → 11): reuse when the source matches and one process only; a foreign snapshot copied to another source's key is refused and triggers a rebuild; wrong format version / `complete: false` / tampered entry / wrong count are all refused; cap exceeded (2 MiB against 1 MiB) fails with the injected limit in the message and leaves no file; cancelled and failed and unparseable runs leave no snapshot; attribute prefixes stripped; entry cap enforced during parsing; snapshot records pinned-ness and the source; temporary file lives beside the snapshot and is gone afterwards. **Review round 2 (2026-10-28):** the cap bounded only `fullcatalog`'s own buffer; `RealRunner` kept appending to `Outcome.stdout`, so memory was still unbounded. `CommandSpec::max_capture_bytes` (opt-in, `None` by default so the diagnostic path keeps the whole log) now bounds the runner's retained copy **without touching streaming**: every line still reaches the sink, the child is never blocked, and `Outcome.truncated` reports the cut. `search_command` sets it to the same limit, and `build` refuses on either signal. Tests: `retained_output_is_bounded_while_streaming_continues` (2000 lines against a 4 KiB retention: stdout ≤ 4096, `truncated`, all 2000 lines seen), `unbounded_capture_is_the_default`, `the_search_command_bounds_what_the_runner_retains` (also refuses a truncated outcome on its own). Mutations: never bound retention → 1 red; ignore `truncated` → 1 red.
Mutations: drop the source-key check → the foreign-snapshot test red (the first version passed, so that test was added); drop `complete` check → 1 red; skip `store` → 3 red; disable the cap → 2 red. Workspace total 252. |
| M5.4 | **done (2026-10-29).** `crates/nhs-cli/src/search.rs`: `nhs search <query>` (curated, offline) and `--full`, with `--json`, `--platform`, `--include-unfree/broken/nixos-only`, `--license-allow/deny`, `--page`, `--per-page`, `--flake-ref`, `--timeout`, `--home`, `--state-dir`. Frozen rules enforced: metadata filters are a **refusal (exit 3) before any lock is read or process started**; `--flake-ref`/`--timeout` without `--full` and non-positive page/timeout are usage errors (exit 1); zero results are success (0). One JSON envelope for both tiers with every key present and `null` where the tier has no data (`source.kind` = `curated` / `explicit` / `flake-lock` / `unpinned-default`; `platform` is the catalogue's tag array for curated, `null` for full). Progress, cache notices and errors go to stderr only. `fullcatalog::search()` reuses the donated ranking and the same total ordering `(tier, position, name_len, name, attr)`. Tests (`--test search` → 12): curated offline with no `nix` on PATH; filters reach the CLI; full builds once then reuses (argv log stays at one call) with `source.kind`/`pinned`/`revision`; metadata filters refused with zero calls; broken lock reported without `--flake-ref`; paging disjoint on both tiers; JSON keys stable on both; zero hits on both; full-only flags without `--full`, zero page/per-page and a missing query are usage errors; a blocked `--state-dir` reports `cannot use` and prints no partial JSON; unpinned source labelled; human output is not JSON. Mutations: drop the metadata-filter refusal → 1 red; drop the full-only flag check → 1 red; never reuse the cache → 1 red; return non-matching full entries → 1 red. Workspace total 264. |

**Acceptance tests**

- `curated_catalog_loads_offline_and_ranks_exact_names_first`
- `filters_exclude_unfree_and_wrong_platform` — plus `unknown platform` and `unknown licence` staying visible
- `revision_resolution_follows_a_follows_input` — fixture `flake.lock` where `nixpkgs` is `follows`
- `snapshot_is_reused_only_when_the_source_revision_hash_matches` — plus: wrong format version, missing `complete`, corrupt digest, wrong count ⇒ rebuild
- `catalog_build_is_cancellable_and_capped` — injected limit; process group gone; no snapshot left behind
- `pagination_returns_stable_disjoint_pages`

**Additional gates for this milestone:** `--json` output stability on both search paths, no snapshot after cap/cancel/parse failure, upstream repositories and the real user configuration untouched.

**Non-goals:** running `nix search` against the network in tests, a full catalogue as the default, GUI work (M6), `switch`.

**Evidence:** `cargo test -p nhs-core catalog`

---

## M6 — Desktop GUI on the proven core  ← **in progress: M6.1 done (2026-10-29)**

**Scope.** Toolkit choice recorded with a short written decision; package browse/select, diff review, validate, apply-confirm, snapshot history, generation list and rollback.

**Deliverables**
- A one-page `docs/gui-decision.md` comparing `iced` vs `slint` vs Tauri against: single-binary packaging, Nix packaging effort, accessibility, and runtime deps.
- GUI calls only `core` public API; no logic in the UI layer (G2).
- Screens: discover → catalog → plan/diff → validate → confirm → build log → snapshot & generations.

**Acceptance tests**
- Headless: all GUI-driven flows exercised through the core API in tests (same assertions as M2–M5), so the GUI adds no untested logic.
- Manual checklist executed in a **scratch HOME**: open app, select two packages, see diff, validate, build (no switch), see snapshot, roll back a file from the snapshot.
- `gui_has_no_direct_file_writes` — extends gate G3 to the GUI crate.

**Stages**

| Stage | Content |
| --- | --- |
| M6.1 | **done (2026-10-29).** New crate `crates/nhs-app`: the GUI's entire logic and none of its drawing. A pure state machine — `Event` in, `Response { intents, refusal }` out — with seven flows (discover, catalog, plan/diff, validate, confirm, build log, history). No toolkit, no filesystem, no process: the last test in `tests/flows.rs` scans the crate's sources and manifest for `std::fs`, `std::process`, `Command::new`, `stdin` and every candidate toolkit. The adapter (M6.3) is the only thing that calls `nhs-core`. Safety encoded in the type system and in one gate: `ticket()` is the single path to an `Apply` intent and requires an editable layout, a ready plan, a **successful validation of the same revision**, an **acknowledged diff**, and it is re-checked when the confirmation is pressed; navigation to the confirm screen goes through the same gate, so it cannot be bypassed. Selection changes bump a revision counter and drop the plan, validation and review; late results for an old revision are ignored — except an `ApplyFinished`, which is recorded because the write really happened. There is no activation intent and `switch_available()` is always false with a reason string. History is two read-only lists. The build log is capped at 5000 lines. Tests (`--test flows` → 17): initial state and discovery, flaky-vs-classic-vs-NixOS-vs-profile-only editability, search intent only, empty selection, selection-change invalidation and stale results, validation ordering, failed validation, unreviewed diff, confirmation bypass attempts (direct `ApplyConfirmed`, `StepRequested(Confirm)`), a late failed validation stopping a confirmed apply, switch always disabled, history separation and read-only, apply success/failure, a late apply result recorded, log lines never changing flow state, bounded log, settings as part of the selection, and the source/manifest boundary scan. **Review round 2 (2026-10-29):** `revision` only moved with the selection, so two plans for the *same* selection were indistinguishable — a late result for plan A could replace the validated and reviewed plan B while B's validation still authorised the write. Fixed by making a plan request an identity event: `revision` now also increments on `PlanRequested`, `PlanBuilt`/`PlanFailed` require the current revision **and** `Pending` (so a duplicate result cannot replace a finished plan), and validation carries its own attempt number (`Intent::Validate { revision, attempt }`, `ValidationFinished { revision, attempt, … }`, attempt 0 refused) so an older run cannot overwrite a newer one. `Discovered` now invalidates a plan only when the layout actually moved (kind or main file), keeping a refresh harmless. New tests: the exact A/B race (late A cannot replace B and the applied intent carries B's identity), late `PlanFailed` for an old request, duplicate plan result, validation ordering in both directions, a validation result with no request, and re-discovery with the same vs a different layout. Mutations: stop bumping on request → 1 red; drop the `Pending` check → 1 red; drop the attempt check → 1 red; drop the attempt-0 check → 1 red; drop the discovery invalidation → 1 red.

**Review round 3 (2026-10-29):** three more state-machine gaps. (1) `Discovered` invalidated only when a plan or validation *existed*, but a plan still being built has neither — so a layout change did not void the in-flight request, and the old layout's plan could land afterwards; invalidation is now unconditional on a layout change. (2) A validation request had no notion of "already produced its outcome", so a second result for the same attempt could overwrite a failure with a success; `validation_done: Option<u64>` now allows exactly one final result per request, and a new request opens a new slot. (3) `layout_identity` compared only kind and main file, but `FlakeHome.attr` decides which `homeConfigurations.<attr>` a plan targets — identity now also carries `attr`, the NixOS host/user pair and the profile directory. New tests: a *pending* plan voided by a layout change (and its late result refused), a duplicate validation result ignored while a new request is accepted, and a same-file/different-`attr` re-discovery voiding the plan. Mutations: restore the conditional invalidation → 1 red; accept duplicates → 1 red; drop `attr` from identity → 1 red. Note: the first attempt at fix (1) silently did not apply (rustfmt had reflowed the surrounding code) and the new test caught it — which is why the test exists.
Mutations: drop the review check → 1 red; drop the revision check → 1 red; drop the "must be on the confirm screen" check → 1 red; make switch available → 1 red. Workspace total 287. |
| M6.2 | **accepted 2026-10-30.** Toolkit decided: **Tauri 2 + WebKitGTK** (Rust backend, HTML/CSS/JS front end, logic in `nhs-app` + `nhs-adapter` + `nhs-core`); the iced prototype is kept for its low-memory comparison data. Acceptance record: five flows pass, keyboard-only run 12/12, focus continuity restored and re-verified, AT-SPI tree complete, disabled step never focusable, real write + snapshot produced, workspace 307 tests, JS syntax and docs checks clean. Prerequisite done: `prepare` (render/rewrite the sidecar, inject the import, stage both) moved from `nhs-cli` into `nhs-core::flow`, with the CLI delegating — the plan-building rules now have one implementation for both front ends. `flow::prepare` refuses a module outside the tree, a state directory inside it, and a system-only option *before* a plan is shown; 8 tests, including "planning writes nothing". `docs/gui-decision.md` records the measured comparison: licences read from the published crates (`tauri Apache-2.0 OR MIT`, `iced MIT`, `slint GPL-3.0-only OR royalty-free/commercial`), system libraries verified present (`webkit2gtk-4.1 2.54.1`, `gtk+-3.0 3.24.52`), Tauri's graph measured at **405 crates**, and the earlier claim that it builds offline **corrected** (`adler2` was missing). NixBox has no GUI at all (ratatui TUI), so "migrate its front end" is off the table. A probe crate in `/var/tmp` compiled the full Tauri/WebKitGTK stack and produced a binary, so the toolchain is proven on this host. **Prototype status:** `crates/nhs-gui-spike-tauri/` (own workspace, target dir in `/var/tmp`) renders seven steps over `nhs-app` via `nhs-adapter`; observed screens captured with `grim` (discover, packages, plan/diff, confirm) and a headless `--selftest` walks the whole chain (discover → search → select → plan → review → validate → confirm → apply → history) against a scratch HOME, leaving the module, the injected import and one snapshot on disk. Running it found three things a compile cannot: `nhs-app` has no "please discover" event (the shell works around it; gap to close), reviewing *before* validating was impossible because a new validation reset the review (fixed in `nhs-app`, regression test added), and the review/validate controls fall below the fold on a 760 px window. The iced prototype (`crates/nhs-gui-spike-iced`, 408 crates, ~5 min cold build, 365 MiB debug) walks the identical `--selftest` sequence and now matches the Tauri layout: the action row sits outside the scrollable and the current step is marked with `button::primary`. It also has a complete `en`/`zh` table in `src/i18n.rs` with a runtime toggle, and `--selftest` asserts that toggling does not disturb the flow. `docs/gui-decision.md` holds the filled comparison table. Accessibility was re-measured on 2026-10-30 with the desktop flag on (`org.a11y.Status.IsEnabled = true`, set through D-Bus because `gsettings` alone does not move it on this niri host; restored to `false` afterwards): Tauri still registers a full AT-SPI tree (accessible names, `Steps` landmark, headings, table semantics, live footer state) and responds to accessibility actions (language switch, screen change), while iced remains **invisible** to AT-SPI with the flag on. A standalone winit + `accesskit_winit` probe registered as `frame a11y probe / button Probe button`, so the host can do it; iced 0.14 cannot without a forked `iced_winit` — no `accesskit` in the graph, `Widget` has no a11y hooks, `run_instance` is private, and upstream #552 has been open since 2020. The earlier false negative came from `NO_AT_BRIDGE=1`; the language button was also found inside an `<h1>` and moved to a toolbar. Keyboard-only walks remain blocked (no input-injection tool, Orca cannot focus the window); the user typed into the iced window directly and its text field accepted the input. Start-up, memory and HiDPI were then measured with one script for both: warm start to window **0.98 s (Tauri) vs 1.22 s (iced)**; idle PSS **211 MiB in 6 processes vs 60 MiB in 1**; both honour a forced 2× scale factor, and neither was tested on a real HiDPI output. The last round drove both prototypes with real key events (XTEST into XWayland, because no Wayland injection client exists here) and read the AT-SPI focus stream with a `pyatspi` consumer, since no screen reader or speech synthesizer is installed: the Tauri window completes the whole five-flow chain by keyboard alone (Tab order sane, disabled Confirm skipped, Enter/Space work, focus ring visible, sticky actions reachable at 360 px height, the applied snapshot lands on disk), but **focus is dropped after every state update** so the user must re-tab from the top. The iced window receives the keys (`XGetInputFocus` confirms it) and changes by **0 pixels** on Tab/Enter/Space: stock iced 0.14 gives applications no focus system. Screen-reader speech, browse order and whether the diff's node Value is announced remain unverified. The focus-loss defect was then fixed in `dist/app.js` (stable `data-focus` keys, caret capture, heading focus on screen change, nearest-enabled fallback, no focus grab when nothing was focused) and re-verified with the same key-driven harness: **12/12** checks pass, including "stays on the search field after searching", "stays on the checkbox after selecting", and "heading of the new screen after plan/validation/apply/history", with the apply still landing on disk. **Toolkit decision (2026-10-30): Tauri 2 + WebKitGTK**, on keyboard operability, AT-SPI exposure and the cost of closing each gap; iced's measured memory advantage (~60 vs ~211 MiB PSS) is recorded but does not offset the missing focus system and accessibility tree. Accepted by the user on 2026-10-30; the iced prototype stays in the tree by that decision.

**Method agreed for the remainder (2026-10-29):** both prototypes, same five flows, same checklist; run each and observe it through Orca (start → capture → read → close), falling back to the user running it and filling the checklist by hand if window control or capture is unreliable; iced is built for real rather than judged on paper; neither candidate is favoured in advance. Recorded in `docs/gui-decision.md`. |
| M6.3 | **accepted 2026-10-31.** `crates/nhs-gui` (Tauri 2.12, own workspace, target in `/var/tmp/nhs-gui`) with `src/{bridge,service,check,runtime}.rs` and `dist/{index.html,style.css,app.js,i18n.js}`. Three commands: `state()` (read-only snapshot), `dispatch(event)` (the only state change), `cancel()` (always `cancel_unavailable` — it does not pretend to cancel). The read-only boundary is enforced before the state machine runs: `Service::ensure_allowed` accepts only discovery, so a refused event cannot even change the screen. `--check` prints the same `UiState` JSON the window renders. **Evidence:** on this host it reports `profile-only`, 3 generations, read-only, with `switch.available=false`; a scratch classic layout reports `classic-home`, editable, generations 0. 7 tests (contract: version/caps, event parsing, discovery, refusals leave state untouched, honest cancel; read-only: source scan for write/spawn APIs, `--check` hashes the whole tree before/after and asserts no state directory is created), clippy `-D warnings` with `unwrap/expect/panic/indexing/as_conversions` denied, fmt clean. UI: header, disabled-for-now steps, sticky action row, `role=status` footer, system light/dark, EN/ZH tables with persistence (`localStorage`), focus restore with `data-focus` keys — verified by keyboard: tab order `Refresh → window chrome → language → Discover`, Enter on Refresh keeps focus on Refresh. AT-SPI exposes frame, `Steps` and nav landmarks, headings, a real table and the live status bar (`步骤：发现 · 已选：0 · 只读`). Zero writes: `~/.local/state/nix-home-studio` still absent after running the window and `--check`. **Dynamic text is bilingual too (2026-10-31, review fix):** the backend now sends stable codes instead of sentences — `switch.reason_code`, `readiness.code`, `discovery_error {code, detail}`, and `BridgeError {code, detail, message, hint}` — and the UI translates them, keeping engine/OS detail verbatim under a translated heading. `RefusalCode::code()` and `App::switch_reason_code()` were added to `nhs-app` (no safety change). `state_version` is now **2**. Acceptance record: key sets identical in both languages, the eight readiness codes translated and rendered (status line), dynamic codes derived from the Rust sources and all covered, front-end syntax checks pass, `check-i18n.js` independently run by the reviewer (`87 keys each; derived 8 readiness, 6 error, 12 action, 7 step, 1 switch code`), zero-write boundary accepted on the user's evidence. The reviewer did not re-run the Cargo suites independently (cold build), and records that as a scope note rather than a pass.

Implementation note: `crates/nhs-gui/scripts/check-i18n.js` no longer keeps a hand-written list: it parses the Rust sources (`RefusalCode::code()`, `App::switch_reason_code()`, `event_key()`, `step_key()`, the `BridgeError` constructors) to derive the code set, then asserts both tables have identical keys (87 each) and that every derived code has a translation — readiness included (8 codes, now shown in the status line: `步骤：发现 · 已选：0 · 此配置在这里是只读的。` / `… · This configuration is read-only here.`). Removing one key makes the script fail with both a parity error and a missing-code error (checked by mutation). Verified on screen and through AT-SPI: `激活 / 此版本不支持激活配置（switch）…`, `步骤：发现 · 已选：0 · 只读`. Screenshots: `/var/tmp/nhs-gui-run/m63-zh.png`, `m63-en.png`. |
| M6.4 | **in progress — five sub-stages, approved 2026-10-31.** Scope: catalog/search/selection, plan/diff, async validation with diagnosics, build-only apply, `--flow-check`; no activation, no settings editor. |
| M6.4.1 | **done 2026-10-31, awaiting review (second pass).** State contract **v3** (`busy`, `catalog`, `plan`, `validation`, `reviewed`, `apply`, `log`, `history` typed in `bridge.rs`; `state_of` split into per-view helpers). One mutex guards the state machine plus the running-job slot and is **never held while a process runs**: the worker takes parameters, releases, runs, then re-acquires to feed the result through `nhs-app` and clear the slot. Results still go through `Event`s, so revision/attempt checks apply to async work unchanged; a late result from an older job id is dropped. `cancel()` binds to the current job id, returns `cancel_unavailable` when idle, and the token is **reset before every job** (new `CancelToken::reset()` + 4-line test in `nhs-core`, `Session::reset_cancel()`) so a cancelled run cannot poison the next. The shell subscribes to `nhs:state`, so the window never polls; while a job runs only Cancel is offered and any other dispatch fails with `busy`. Tests: **12** in the crate (contract 5, read-only 2, worker 5) including background validation, apply writing exactly one snapshot through core, cancel stopping a sleeping builder and a following job succeeding, busy refusal, and the notifier delivering a finished state. Window still opens and renders Discover.
| M6.4.2 | **done 2026-10-31, awaiting review.** Catalog, selection, plan and review render on the same shell. Packages: search box (Enter or button), hit table with per-package checkbox, licence/category, selected count, sticky `Build plan`. Plan: per-file heading (path, create/replace, +n −m), unified diff in a monospace pane, folding at 2000 lines behind `<details>`/`<summary>`, notes, sticky review checkbox and Validate. Navigation enables only screens this build renders (`RENDERED` in `app.js`), while `capabilities` stays the backend truth. Two defects found by running: `text-transform: uppercase` on `h3` was upper-casing file paths on screen *and* in the AT-SPI name (removed — paths are case-sensitive), and the reviewer's stage boundary was respected by leaving Validate disabled at the nav level. Tests **18** (contract 8, read-only 2, worker 8), including search→select→plan→review producing two files with non-empty diffs and a no-match search that stays a normal state. Verified by keyboard: Packages → type `firefox` → Enter → Space → Build plan → heading `Plan and diff` → Space on the review box leaves focus there and enables Validate (`+12 −0`, `+1 −1` diffs read back through AT-SPI). Screenshot `/var/tmp/nhs-gui-run/m642-plan.png`. **Review fix (same day):** the folded branch was still rendering `lines.slice(0, 2000)`, so expanding showed a prefix — `<details>` hid content instead of just hiding it. The `<pre>` now always carries the complete `file.diff`; only the initial visibility is folded. New `scripts/check-diff-fold.js` runs the real `app.js` against a small DOM stub with a 2500-line diff and asserts: collapsed by default, exactly 2500 lines in the `<pre>`, the last line present, and a short diff rendered without `<details>`; re-inserting the `slice` makes it fail with `expanded diff has 2000 lines, expected 2500`. |
| M6.4.3 | **done 2026-10-31, awaiting review.** Validation screen with three states: running (translated status, streamed log tail, Cancel), success (command, duration, result, "next: review and apply"), and failure (translated diagnosis label and hint from 14 stable `ErrorKind` codes, raw engine summary, location, kept scratch copy). Logs show the last 200 lines with a total count; `nhs-app` still caps at 5000. The **Tauri capability file** was the bug this stage found by running: without `capabilities/default.json` the window's `event.listen` is denied, so the worker finished while the UI stayed on "Validating…" forever. Added `core:default` + `core:event:default`; the finished state now arrives (`The build succeeds in the scratch copy.`). Also fixed the footer showing "read-only" when a plan is actually ready to apply. `scripts/check-all.sh` now runs every gate (node syntax, i18n, diff fold, diagnosis, both test suites, both clippy runs, both fmt checks, source scan) and passes. New JS gate `scripts/check-diagnosis.js` renders the real `app.js` against a DOM stub and asserts translated labels/hints, raw detail kept, and cancelled reading as interrupted. Verified live: success (2 ms fake build) and cancel of a sleeping builder (2240 ms, no processes left, `Interrupted`, scratch kept), read back in English and Chinese (`构建失败 / 已中断 / 该怎么办: 没有写入任何东西…`).
| M6.4.4 | **done 2026-11-01, awaiting review.** Confirm, build log and history render on the same shell; writes still go only through `EditEngine::commit` in the adapter, and no activation exists. **Core safety gap found while building this stage:** after a successful apply, `ticket()` still passed (plan, validation and review are unchanged), so returning to Confirm could write the same plan a second time. Fixed in `nhs-app`, not the GUI: `RefusalCode::AlreadyApplied` added and `ticket()` refuses a revision already in `ApplyState::Applied`; test `the_same_plan_cannot_be_applied_twice` also checks that a changed selection is applyable again. The GUI reflects the gate instead of re-implementing it: the Confirm step is enabled only when `readiness === null`, and refusals now travel to the UI as `{code: refused, detail: <refusal code>}` so the translated readiness text explains them. History is honestly read-only: the reason comes as `history_read_only`, and the screen has **no rollback control at all** because core has none (`check-apply.js` asserts zero buttons on that screen). Failure and success render differently and the raw commit message is kept. Verified end to end: keyboard drove confirm → apply → log → history, the scratch tree gained one snapshot plus one backup and the module/import, history showed `applied · 2 files`, and after a restart History still lists it while Confirm is disabled. `check-all.sh` now includes `check-apply.js`; **329 tests** (workspace 309 + gui 20) and every gate passed. |
| M6.4.5 | **done 2026-11-01, awaiting review.** `nhs-gui --flow-check` runs the whole chain through the same `Service`/adapter against a temporary HOME, a fake `home-manager` and a temp store, prints an 11-entry JSON transcript, and deletes the scratch tree. `tests/flowcheck.rs` runs the binary with the caller's `HOME` pointed at an empty directory and asserts: exit 0, all 11 steps, validation true, apply `applied`, 2 files written, snapshot id present, one history snapshot, **the caller's HOME and state/config directories untouched**, and the scratch tree gone. The write guard now treats `flowcheck.rs` as the single explicit exception (it must build a scratch HOME), with stricter rules: writes only, no `Command::new` anywhere, and the runtime isolation test as the real proof; `check-all.sh` enforces both the exception and the rule. Live keyboard run (XTEST + AT-SPI) completed discover → catalog → plan → validate → confirm → apply → history: Confirm is disabled after the write (`already_applied` in the footer), the scratch tree holds the module, the injected import, one snapshot and one backup, and History lists `applied · 2 files` with no rollback control. A failed write is deterministic at the service level: with the config directory read-only the apply ends `failed`, Core records the attempt as `outcome: failed` with its reason, restores `home.nix`, and leaves no module behind (`a_failed_write_is_rolled_back_and_recorded`); the live attempt to reproduce it by chmod was racy and is not claimed as evidence. No activation path exists in the GUI source (only `switch` data fields and refused copy), `state.switch.available` is false, and the real `~/.config/home-manager` and state directory remain absent. Two display defects found by looking: history's snapshot id wrapped to one character per line (fixed with `nowrap`), and entering History from the sidebar after a restart showed `Loading…` because navigation did not request the report (fixed: the History step now dispatches `history`). `check-all.sh` passes with **331 tests** (workspace 309, gui 22).
| M6.5 | **done 2026-11-03, awaiting review.** Real-environment acceptance is recorded in `docs/m65-real-environment.md`: real host read-only (profile-only, 3 generations, store-only toolchain, 78–89 ms discovery, 741–920 ms start-up, 230–248 MiB PSS, zero changes to `~/.config/home-manager` or the state directory, plus the app-owned WebKit data directory named explicitly), and a real-toolchain flow in a throw-away HOME (classic layout, `jq` selected, real `home-manager build` 27 s, apply writing module + import + one `applied` snapshot, history read-back, second confirmation refused). **Defect found and fixed:** validation re-resolved `home-manager` through `PATH`, so a store-only install — this host — could never validate (`home-manager was not found on PATH`). Fixed in core and adapter: `HmInvocation::binary()`, the store scan also runs when no generation is linked, and `Session::validate` prepends the resolved binary's directory for the runner (the allowlist check still applies). Regression test `validation_uses_the_store_toolchain_when_path_is_empty` failed before the fix and passes after. Also fixed a stale success message that still claimed review/apply arrived “in the next stage”. Keyboard injection was unavailable in this session (X focus present, web view received none), so B1 was driven through AT-SPI actions; A used real typing earlier. `check-all.sh` passes with **335 tests**. |

**Review fix (2026-11-01):** cleanup lived at the end of the success path, so any `?` before it left the scratch tree behind. It now belongs to a `Scratch` guard whose `Drop` removes the tree, and `Setup { tool_exit, scratch }` (doc-hidden) lets a test force a failing fake build with a known path: `a_failed_flow_still_cleans_its_scratch_tree` asserts the error *and* that the tree is gone. Disabling the `Drop` body makes both flow-check tests fail with `the scratch tree was left behind at …`, so the guard is what is being tested. **Second review fix (2026-11-02):** the guard was created only after the setup writes, leaving an initialisation window uncovered. Now the root is created **exclusively** with `fs::create_dir` (an existing directory is refused, never adopted or later deleted), the guard is created immediately, and every later setup step runs behind it. `a_failed_setup_still_cleans_its_scratch_tree` injects a setup failure and asserts the tree is gone; `an_existing_scratch_path_is_refused_not_reused` asserts a pre-existing directory is left intact. Moving the guard back after the failing step makes the first test fail, so the ordering is what is covered. Total is now **334 tests** (workspace 309, gui 25). The **first CI run** then failed in `nhs-adapter` on the runner, not locally: `Fixture::session()` used `Session::new(Some(home), …)`, which still reads `XDG_CONFIG_HOME` from the process, so GitHub's environment pointed discovery outside the fixture and the layout came back `unknown`. Reproduced locally with a decoy `XDG_CONFIG_HOME`, fixed by building `EnvView` explicitly in the fixture (audit R13), and `Session::new` now documents that it inherits the rest of the environment. |

**Review fix (same day):** `footer_ready` existed twice per language, so the later `"ready"` / `"就绪"` silently won and the footer never showed the intended text. Removed the duplicates (verified by loading the table: `ready to apply` / `可以应用`) and added **duplicate-key detection** to `check-i18n.js`: the loaded object cannot reveal overwritten keys, so it scans the source per language block and fails on repeats; re-inserting the duplicate makes it fail with `en: duplicate keys footer_ready`. Live footer read-back after a successful validation: `step: validation · selected: 1 · ready to apply` and `步骤：校验 · 已选：1 · 可以应用`. Tests: **19** in the crate (contract 8, read-only 2, worker 9) |


**Integration gaps found while building M6.1 (not fixed here)**

1. **`prepare` lives in the CLI.** Rendering the sidecar, injecting the import and staging the plan is `nhs_cli::app::prepare`; a GUI adapter would have to duplicate it. It should move into `nhs-core` (e.g. `flow::prepare`) so both front ends share one implementation. This is the main gap for M6.3.
2. **No snapshot listing API.** `SnapshotStore` can save, load and find the last applied snapshot, but not enumerate; the history screen's snapshot list would need adapter-side directory walking.
3. **Generations are only exposed for `ProfileOnly`.** `Layout::generations()` returns `&[]` for `FlakeHome`/`ClassicHome`, so a user who actually has a config would see an empty generation list.

**Evidence:** `cargo test` + a filled-in manual checklist file with dates and `scratch HOME` paths.

---

## M7 — Generations, rollback, and a disposable-VM end-to-end run

**Scope.** Real Home Manager generation listing + rollback in a disposable environment; offline packaging.

**Deliverables**
- `Generations` implementation: profile-symlink listing (works for the `ProfileOnly` layout) + `home-manager generations` cross-check; rollback via `home-manager switch --rollback`/`--switch-generation`.
- Two clearly labelled rollback actions: HM generation vs. app snapshot (architecture §7.2).
- Nix flake packaging (`nix build .#default`, `.desktop`, icon), and a second-environment migration test.
- End-to-end script that runs in a NixOS VM or a containerised scratch `HOME` with its own HM profile.

**Acceptance tests**
- `listing_generations_from_profile_symlinks_matches_home_manager_output` (fixture: a replica of the audit host's `home-manager` + `home-manager-{1,2,3}-link` set).
- `rollback_invokes_home_manager_and_not_activate_directly` (asserts argv).
- `snapshot_revert_and_generation_rollback_are_distinct_actions`.
- E2E (test-only, never CI-with-sudo, recorded as evidence): install a package in a scratch profile → build → switch → verify `bin` → roll back → verify removal.
- Portability: `nhs discover` + `nhs plan` succeed on a machine with no `/etc/nixos` and no `~/.config/home-manager` (the audit host's clean state), with an explicit "no configuration found, here is how to create one" message.

**Evidence:** recorded transcript of the E2E run, with the VM/container path and date.

---

## Non-goals (v1)

| Non-goal | Reason |
| --- | --- |
| `nixos-rebuild switch`, system package management, anything requiring `sudo` | Privilege + blast radius; audit R4. NixOS is shown read-only. |
| Git/JJ commit, push, remote creation | NixBox's `vcs.rs` is a donor for the journal only; pushing user config is out of scope (audit §2.4). |
| Flake browser / GitHub search | Needs `gh auth`; network + supply-chain risk; not needed for the beginner path. |
| Profile-mode `buildEnv` installs (`nixbox profile setup`) | Clever but adds a second activation model; revisit after v1 (audit §2.2). |
| Telemetry of any kind | Audit R7; explicitly excluded from donated code. |
| `nix-darwin`, WSL, remote builders, multi-host | Portability matrix in architecture §8.2. |
| Importing or moving a user's existing package declarations into management | NixBox's `scan`/`migrate` is the riskiest code in either project (audit §2.3); v1 only *adds* to its own module. |
| Editing options spread across multiple imported files | Requires whole-tree evaluation; refuse and explain instead. |
| Atomic compare-and-act against a concurrent writer on the same path | POSIX has no path-based compare-and-act (`renameat2(RENAME_NOREPLACE)` covers create only and is not in `std`). v1 guarantees "check immediately before the action, verify content, keep a backup and a record", not mutual exclusion — see architecture §7.1.1 for the exact window and threat model. Descriptor/`(dev,ino)`-verified deletes and quarantine-by-rename are the v2 options. |
| Windows / macOS support | Out of scope for a NixOS/Home-Manager-Linux tool. |

### M6.4.1 review fixes (2026-10-31)

(1) logs are now **streamed on a clock, not on output**: a `LogBuffer` shared by the adapter's sink and a 25 ms ticker flushes every 100 ms or 50 lines (5000-line cap still in `nhs-app`), so a build that prints one line and then goes quiet still reaches the window. Two tests cover it: one with a line every 40 ms, one with a single line followed by 800 ms of silence, asserting the line arrives while the job is still `busy`. (2) `capabilities` are all **true** — they describe backend ability; the window keeps a separate `RENDERED` set so unbuilt screens stay disabled (verified through AT-SPI: Discover enabled, the other six disabled). (3) The cancel token is reset **before** the job slot becomes visible, closing the window where an early cancel could be cleared. (4) `cancel()` refuses **apply** (`cancel_unavailable` + detail `apply`) because `Session::apply` never reads the token; the UI hides Cancel for an apply instead of offering a button that cannot work. (5) A worker that fails to spawn clears busy and, for apply, drives the state to `failed` with a message (validation logs `Progress`), verified with an injected spawner. Tests now **16**.


## Safety gates summary (must be green before release)

1. G1 dependency allowlist passes.
2. G2 dependency-direction test passes (`core` has no UI deps, no printing).
3. G3 no writes outside `EditEngine::commit` + backup store.
4. G4 `switch`/rollback require a confirmation naming the exact command.
5. G5 hash guard + backup present for every write.
6. G6 every written file re-parses.
7. G7 no test in the default suite activates anything.
8. M7 E2E evidence recorded, including a rollback.

## Recommended minimal viable version

**M1 → M2 → M3 → M4 → M5(curated tier only) → M6**, i.e. a desktop app that can:

1. discover the user's layout (including the hard `ProfileOnly` case) and say what it found;
2. let a beginner pick packages from a curated catalog that works offline;
3. write a single generated module with a previewed, backed-up, hashed, atomic diff, and insert exactly one import line;
4. validate against a scratch copy before touching anything;
5. build, and only then — after a confirmation naming the command — switch;
6. show the resulting generation and offer Home Manager's own rollback plus an app-snapshot revert.

**Deferred from the MVP:** full nixpkgs catalog, NixOS/sudo, generations history beyond the current profile, flake browser. That list is what makes the MVP buildable without inheriting either upstream's unsafe write path.
