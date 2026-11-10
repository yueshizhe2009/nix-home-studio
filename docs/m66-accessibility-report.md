# M6.6 — interaction and accessibility report

Date: 2026-11-04. Builds on `docs/m66-accessibility-audit.md` (M6.6.1).
Scope: GUI layer only. No core, adapter, safety or Nix behaviour changed.

## Fixes (all with regression tests)

| ID | Defect | Fix | Test |
| --- | --- | --- | --- |
| F1 | The Build-log screen was routed to the log-lines renderer, so the applied-snapshot message and the screen heading never rendered; focus had no target after apply | `renderContent` routes `log` to `renderLog` | `check-a11y.js` renders through the router and requires the `Build log` heading and `Applied snapshot …` |
| F2 | A refused action drew an error but nothing announced it | error notice carries `role="alert"` (AT-SPI exposes it as `notification`) | `check-a11y.js` asserts the role |
| F3 | A finished validation changed the screen silently | the result block is focusable and receives focus when the validation worker finishes | `check-a11y.js` asserts focus lands on `#validation-result`; real desktop confirmed |
| F4 | Navigating to another screen left focus on the nav button, so the new heading was never announced | a step change always focuses the new screen heading; same-screen updates still keep the current control | real desktop: activating `Packages` now leaves `heading 'Packages'` focused |
| F5 | No busy indication for AT while a job ran | `aria-busy="true"` on the content area while `state.busy` is set | `check-a11y.js` asserts it is set and removed |
| F6 | Tables had no accessible name; headers had no scope | `aria-label` on the catalog/history tables, `scope="col"` on headers | `check-a11y.js`; AT-SPI now reports `table 'Packages'` |
| — | `el()` assigned `aria-label` as a JS property, which never reaches the DOM — the search box had **no** label beyond its placeholder | `aria-*` (and `scope`) are set with `setAttribute` | covered by the same checks |

## Real-desktop verification (this host, niri, 1440×900, scale 1)

Keyboard-only walk on a fixture HOME: Packages → type `ripgrep` → Enter → Space →
Build plan → Space (review) → Validate → Confirm → Apply → History.

| Observation | Result |
| --- | --- |
| Tab order | `Switch to Chinese` → nav (Confirm skipped while disabled) → content → actions → window chrome → wrap |
| Enter / Space | activation and checkbox toggling work; Space keeps focus on the checkbox |
| Step change | heading of the new screen is focused (`heading 'Packages'`, `heading 'Confirm'`) |
| Validation | result takes focus; success text present; footer live region reports the running state |
| Apply | applied snapshot message and `Build log` heading present; focus on `heading 'Build log'` |
| Refusal | notice exposed as AT-SPI `notification` |
| Tables | catalog table exposed as `table 'Packages'`; cell text remains in the AT-SPI Value |

## What is not verified

- **Speech output.** No screen reader and no synthesiser on this host: `/usr/bin/orca`
  (GNOME Orca) is absent — the `orca` binary here is the agent CLI — and there is
  no `speech-dispatcher`. Only AT-SPI semantics were checked; actual pronunciation,
  browse-mode order and verbosity are unverified.
- **125% / 150% scaling.** The host has one 1× output; changing the compositor
  scale would modify the desktop, which is out of scope.
- **200% on a real HiDPI panel.** At `GDK_SCALE=2` the UI renders at 2× correctly,
  but a 1100×700 logical window becomes 2200×1400 physical, larger than this
  1440×900 panel, so the edges clip. That is a screen-size limit, not a scaling bug.
- **Screen-reader table navigation** (headers are value-only).

## Close-out (2026-11-09)

The two product follow-ups listed in the table were fixed in this change. No
core, adapter, safety or Nix behaviour changed.

| Item | Change | Evidence |
| --- | --- | --- |
| Skip link | `<a class="skip-link" id="skip-to-content" href="#main-content">` before `<nav>`; `<main id="main-content" tabindex="-1">`; activation moves focus to the screen heading (`#screen-title`) or, when absent, the main landmark | `check-a11y.js` asserts order, target, `tabindex="-1"`, Chinese localisation and focus landing; the running app exposes `link 'Skip to content'` in the AT-SPI tree |
| Default window | 1180×760 → 1100×700; `minWidth/minHeight` remain 900×600 | `crates/nhs-gui/tauri.conf.json` |

Real desktop: the GUI started on this host and the accessibility tree contains
the skip link as a `link` before the headings. The compositor here is a tiling
window manager, so it sized the window itself and the 1100×700 default geometry
could not be observed directly. During the keyboard check, one AT-SPI synthetic
key sequence was delivered to another window (an observed input change) before
the focus target was confirmed; it was stopped immediately. That may have
affected that window's input. No real keyboard walk is claimed in this run; the
automated focus regression is the evidence for the focus move. A manual
keyboard walk remains to be repeated when a floating or size-preserving session
is available.

## Remaining follow-ups

- Table header cells expose values rather than names; verify with a real screen
  reader before adding more ARIA.

## Harness notes

Earlier "keyboard does not work" reports were false: focus was read from an
ancestor node, and a window on a non-active workspace received no keys. With
deepest-node focus tracking and a verified active workspace, keyboard input works.
The audit and this report record only runs that passed those preconditions.

## Gates

`sh crates/nhs-gui/scripts/check-all.sh` → `exit 0`: 335 tests across both
workspaces, clippy `-D warnings`, formatting, i18n (179 keys × 2 with duplicate
detection), diff folding, diagnosis rendering, apply/history, the new
accessibility checks, the write-boundary scan, `--check` and `--flow-check`.
