# M7 (hm) — Home Manager daily experience: task cards

Status: **WP1 and WP2 implemented 2026-11-09; WP3 next.** Source of scope:
`docs/product-roadmap.md` §10. This file turns the eight scope items into work
packages with an observable acceptance test. It does **not** reopen `M7-gens`
(generations, packaging, isolation E2E) and does not add an activation path.

## Boundary (applies to every WP)

- No `home-manager switch`, no rollback execution, no `nixos-rebuild`, no `sudo`.
- No new process program; `proc.rs` keeps `Nix` + `HomeManager` and the existing
  `build` usage.
- Writes still go only through the existing EditEngine path (diff, backup,
  conflict guard, snapshot). A WP that only reads must not add a write path.
- Every WP must name the daily step it improves. If it cannot, it is not done.
- GUI changes need a real-desktop check; fixture/DOM-only evidence is labelled
  as such.

## WP1 — Layout recognition and guidance

**Daily step:** after opening the app, the user knows which Home Manager layout
was found, what NHS may edit, and what to do when nothing usable was found.

**Current evidence:** `discovery.rs` classifies `classic-home`, `flake-home`,
`flake-nixos`, `profile-only` and `unknown`; `bridge.rs` exposes `kind`,
`editable`, `root`, `main_file`, `generations`. The GUI shows a generic
`notice_unknown`; the concrete `Unknown { reason }` and the candidate attempts
are not shown.

**Change:**
- expose the `Unknown` reason and the ambiguity candidates to the GUI (no new
  detection rule, no new file reads);
- one plain-language panel per layout: what it is, what is editable, what is
  read-only and why;
- `profile-only` keeps the existing creation hint and generation list;
- `unknown` shows the reason plus one next step, never a guess.

**Acceptance (done 2026-11-09):**
- `DiscoveryInfo.reason` carries the engine wording; `check-discovery.js`
  renders five kinds and asserts the unknown reason appears verbatim.
- Five explanations in both languages; `check-i18n.js` key parity passes.
- `check-a11y.js` still passes; full `check-all.sh` passes.
- Real desktop: the app started on a scratch HOME; an empty HOME shows the
  profile-only explanation and read-only notice. Fixture classic/flake runs are
  covered by the DOM test, not claimed as real-machine acceptance.

**Not in WP1:** creating `home.nix`, parsing flake inputs, editing `flake.nix`.

## WP2 — Configuration status

**Daily step:** the user can answer “what am I editing, where does it live, is it
editable, and which generation is current?” without reading Nix files.

**Current evidence:** the Discover screen shows kind, editable, root, main file
and a generation count. `generation_state()` already resolves the current
generation from the `home-manager` profile symlink, but `DiscoveryInfo` does not
carry it, and `Layout::generations()` is only populated for `profile-only`.

**Change:**
- add `current_generation` (id and link) from the adapter's read-only
  `generation_state()`, for every layout;
- add the read-only reason (layout kind plus engine wording) instead of a
  generic notice;
- show the exact file NHS would edit, or “none” with the reason.

**Acceptance (done 2026-11-09):**
- `Discovery` carries a read-only `GenerationState` for every layout; core test
  asserts `current == Some(3)` for the profile-only fixture.
- `DiscoveryInfo` exposes `current_generation` and `current_generation_link`;
  the generation count now comes from that snapshot rather than only
  `Layout::ProfileOnly`.
- `--check` on the real host reports `profile-only`, 3 generations, current `3`,
  link `…/home-manager-3-link`, matching `readlink` of the profile.
- `check-discovery.js` asserts the label and link; no process or write added.

**Not in WP2:** generation switching, rollback buttons, history changes.

## WP3 — Package flow

**Daily step:** search, select, remove and spot duplicates/conflicts before a
plan is built.

**Current evidence:** curated catalogue, per-package checkbox, selected count,
plan builder. Removal is only “unselect”; there is no duplicate/conflict
diagnosis.

**Change:**
- make removal explicit (selected list with remove action) without adding a
  second package source;
- detect duplicate package names in the selection and explain what will happen;
- keep the curated/offline tier as the default; full-tier remains optional.

**Acceptance:**
- Rust tests for duplicate detection and stable ordering;
- JS test for the selected list and duplicate notice;
- no change to the apply gate; `nix profile`-installed packages are **not**
  claimed as removable.

## WP4 — Error diagnosis

**Daily step:** a failed build tells the user what to do next, with the raw
output still available.

**Current evidence:** `classify.rs` has 14 stable `ErrorKind` codes; the
validation screen shows a translated label, hint, raw summary, location and the
kept scratch copy.

**Change:**
- review the 14 hints against real build failures; add a “next step” that names
  the file and line when the engine reports one;
- never replace the raw output; keep the scratch-copy path visible;
- add a test per newly worded code; no new parser framework.

**Acceptance:** failing fixture builds map to the intended code and hint; the
raw text is still present; real build failure check on the fixture HOME.

## WP5 — Diff and backup experience

**Daily step:** the user can read a plan, fold large diffs, and understand what
the backup can restore.

**Current evidence:** per-file diff, `+n −m`, 2000-line fold behind
`<details>`, content-addressed backups outside the config tree.

**Change:**
- show backup availability and restore scope next to the planned files;
- keep folding and paths exactly as rendered (no uppercasing);
- explain that a backup is the pre-apply bytes, not a generation rollback.

**Acceptance:** JS test for the backup note and fold; real desktop read of a
multi-file plan; no new restore execution path.

## WP6 — Apply reliability

**Daily step:** a repeated or failed apply cannot silently write twice.

**Current evidence:** `RefusalCode::AlreadyApplied` refuses a revision already
applied; the confirm step reflects readiness. Failure recovery and snapshot
consistency are covered by tests but not by a real daily scenario.

**Change:** add real-scenario tests (apply, second apply refused, changed
selection applyable again) against the fixture HOME; no permission change.

**Acceptance:** the three scenarios pass with the real adapter; no activation.

## WP7 — Generation and history cross-check

**Daily step:** the History screen agrees with `home-manager generations`.

**Current evidence:** `M7-gens` lists profile symlinks and marks the current
one; the named cross-check is deferred. `home-manager generations` is a process
call; the allowlist already contains `HomeManager`, but the subcommand and
failure policy need review before use.

**Change:** run the cross-check only if the WP review approves a read-only
`home-manager generations` invocation; otherwise parse the same profile
symlinks and state the limitation. Never infer a generation that the profile
does not contain.

**Acceptance:** on the real host, the cross-check reports identical ids/current
or an explicit “engine output unavailable”; no write, no switch.

## WP8 — Flake Home Manager compatibility assessment

**Daily step:** a flake-home user gets an honest statement of what M7 can and
cannot do before M8.

**Current evidence:** discovery text-probes `flake.nix` for
`homeConfigurations.<user>` and imports; there is no inputs/outputs model.

**Change:** a read-only assessment section: detected attr, main file, whether
the flake references `home.nix`, and that input parsing/outputs belong to M8.
No flake evaluation, no `nix flake metadata`, no edits.

**Acceptance:** fixture flake-home shows the attr and the M8 boundary; unknown
or unsupported flake shapes are labelled as such.

## Ordering

1. WP1 + WP2 (read-only discovery/status, highest daily value).
2. WP3 (package flow: search, selection, removal, duplicates).
3. WP4 (error diagnosis) and WP5 (diff/backup) on existing flows.
4. WP6 (apply reliability tests).
5. WP7 only after the process-policy review; WP8 as the M8 hand-off note.

## Gate for every WP

`sh crates/nhs-gui/scripts/check-all.sh` plus the WP-specific tests above.
Report what was inspected, what ran, and what remains unverified; no fixture or
Xvfb result is presented as real-machine acceptance.
