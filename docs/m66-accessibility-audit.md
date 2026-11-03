# M6.6.1 — accessibility and interaction audit

Date: 2026-11-03. Scope: the formal Tauri 2 + WebKitGTK application
(`crates/nhs-gui`), all screens, keyboard, AT-SPI semantics, HiDPI, screen
reader availability. This is an audit: no product code changed for it.

## Method

- Static reading of `dist/index.html`, `dist/app.js`, `dist/style.css`.
- **Real desktop**: CachyOS, niri (Wayland), one output `eDP-1` at scale 1,
  1440×900. Application launched with the real `HOME` for read-only screens, and
  with a throw-away `HOME` plus a fake `home-manager` for flows that write.
- **AT-SPI** through `pyatspi` and the Orca computer-use provider: roles, names,
  values, states, and accessibility actions.
- **Keyboard** through XTEST into the XWayland window, with the window verified to
  be on the active workspace and compositor-focused before each key.

### Harness corrections (important for reading older notes)

Two earlier "keyboard does not work" conclusions were **harness errors**, not
product defects:

1. Focus was read from an ancestor node (`scroll pane`) instead of the deepest
   focused node, so a working Tab looked like "nothing focused".
2. The window had been moved to another workspace; keys went to that workspace's
   focused window, not to the application.

With both corrected, keyboard input and focus traversal work, and the recorder
below is a real Tab sequence.

## Screen and control inventory

| Screen | Controls (accessible role · name) |
| --- | --- |
| All | `button · Switch to Chinese/English`; `landmark · Steps` with 7 `button`s (Confirm disabled when the gate is closed); `status bar` (role=status, aria-live=polite) |
| Discover | `heading · Discover`; facts `table` (row headers + cells); notice; `button · Refresh` |
| Packages | `heading · Packages`; `entry · Search the curated catalogue`; `button · Search`; `table` with 20 rows: `check box · <name>` + description/licence cells; `button · Build plan for N package(s)` |
| Plan & diff | `heading · Plan and diff`; per file an `h3` with path and counts; diff `pre`; `check box · I have read this diff`; `button · Validate in a scratch copy` |
| Validation | `heading · Validation`; command/duration; success or diagnosis block; `heading · Build output` + log `pre`; while running: `button · Cancel` |
| Confirm | `heading · Confirm`; warning; file list; `button · Apply (build only)`; `button · Cancel` |
| Build log | (see F1) `heading · Build output`, log `pre`, `button · History` |
| History | `heading · History`; read-only note; generations table; snapshots table; no rollback control |

No modal dialogs exist: "Confirm" is a screen, so Escape-to-close and
dialog-focus-trap requirements are **not applicable** to this build.

## Keyboard, measured

| Check | Result |
| --- | --- |
| Tab order (History screen) | `Switch to Chinese` → `Discover` → `Packages` → `Plan & diff` → `Validate` → *(Confirm skipped: disabled)* → `Build log` → `History` → window chrome → wrap |
| Enter activation | Enter on the focused `Packages` nav button changed the screen |
| Space activation | Space toggled the focused package checkbox; focus stayed on it; the plan button count updated |
| Typing | `ripgrep` typed into the focused search entry; Enter submitted; one hit returned; focus stayed in the entry |
| Disabled control | `Confirm` (gate closed) is not focusable and reports disabled |
| Shift+Tab | moved backwards (History → Build log) |
| Busy state | pressing Validate moves focus to the `heading · Validation`; `Cancel` exists; footer announces `running Validation…` |
| After completion | focus stays on `heading · Validation`; success text is static |
| After apply | **focus becomes `(none)`** — see F1 |

## Findings

### F1 — Build-log screen is routed to the log-lines renderer (product, high)

`renderContent()` maps `case "log"` to `renderLogLines()`, while the screen
renderer `renderLog()` — which adds the `Build log` heading, the status line and
the `Applied snapshot … Nothing was activated.` notice — is only called from the
validation failure branch.

Repro (fixture HOME, fake tool): complete plan → validate → apply, then open
Build log. The screen contains only `heading · Build output`, `(no output yet)`
and the History button. The applied snapshot message never appears, and because
there is no `h2`, the focus restore on entering the screen has no target:
AT-SPI reports no focused element.

Evidence: `orca computer get-app-state` headings on the applied screen are
`['Nix Home Studio', 'Build output']`; the DOM check `check-apply.js` calls
`renderLog()` directly, so it never exercised the router.

Fix: route `log` to `renderLog()`; add a router-level regression test that goes
through `renderContent`.

### F2 — Errors are visible but not announced (product, high for AT)

A refused action renders a notice with `h3 · Something went wrong` and
paragraphs, but the block has no `role="alert"` / live region, and focus stays on
the control that triggered it. The only announcement is the footer status
changing to the readiness sentence.

Repro: on the profile-only host, select a package and press `Build plan`. The
heading changes to "Something went wrong" above the fold while focus remains on
`Build plan for 2 package(s)`; a screen reader is not told an error appeared.

Fix: mark the error notice `role="alert"` (or focus its heading), and keep the
footer live region for state, not for errors.

### F3 — Validation completion is not announced (product, medium)

While running, focus moves to the `Validation` heading and the footer announces
the busy state. When the worker finishes, the result paragraphs replace the busy
text, but nothing is focused or live; a screen reader hears nothing unless it
navigates back into the content.

Repro: slow fake build; start validation, wait for completion, watch AT-SPI: the
focused node is still the heading, and the result paragraph is not in a live
region.

Fix: put the result (success or diagnosis summary) in a polite live region, or
move focus to it on completion (without stealing focus if the user has moved on
— the screen is still the same step).

### F4 — Navigating between screens can leave focus on the nav button (design, medium)

`restoreFocus` keeps the control with the same `data-focus` key. Nav buttons
exist on every screen, so clicking `Packages` leaves focus on that button and the
new screen's heading is never announced. Focus is only put on the heading when
the previous control disappears.

Repro: focus `Packages` (keyboard or `grab_focus`), activate it; AT-SPI still
reports `button · Packages` focused on the Packages screen.

Fix: when `previousStep !== state.step`, focus the new screen's heading first,
even if the previous control still exists. Same-screen updates keep the current
control.

### F5 — Busy state lacks `aria-busy` (product, low/medium)

The content area has no `aria-busy="true"` while a job runs, so AT may read a
half-updated region. Cancel is reachable in one Tab from the heading, but is not
focused.

Fix: set `aria-busy` on the content container while `state.busy` is non-null.

### F6 — Table semantics are value-only (product, low; screen reader unverified)

Catalog and history tables expose column headers and cells with empty accessible
**names**; the text sits in the AT-SPI **Value** (e.g. `column header · Value:
licence`, `table cell · Value: fd Simple, fast alternative to find · development`).
The first catalog header cell is empty (checkbox column) and tables have no
caption or `scope`.

Fix (low risk): add `scope="col"` to headers and an `aria-label`/caption naming
each table; verify with a screen reader before adding more.

### F7 — No skip link (product, low)

The step sidebar precedes the main content in DOM order; a keyboard user tabs
through eight controls before reaching content. Not a blocker at this size.

### F8 — Default window size vs small screens (product, low)

The window asks for 1180×760 logical pixels. At 100% on a 768-pixel-high panel the
footer would be at the edge; at 200% on this 1440×900 panel the window is larger
than the screen and content is clipped.

## HiDPI

| Scale | Result |
| --- | --- |
| 100% (native) | correct; layout and text normal |
| 200% (`GDK_SCALE=2`, X11 path) | UI renders at 2× (text and controls double), but the 1180×760 logical window becomes 2360×1520 physical — larger than this 1440×900 panel, so the right and bottom are clipped |
| 125% / 150% | **not verified**: the host has one output at scale 1, and changing the session's compositor scale would modify the user's desktop |

The 200% run shows scaling is honoured; the clipping is a screen-size limit, not
a scaling defect. A real HiDPI panel would still give 1180 logical pixels.

## Screen reader

Not installed: no GNOME Orca (`/usr/bin/orca` absent; the `orca` on this host is
the agent CLI), no `speech-dispatcher`, no synthesizer. Only AT-SPI checks are
possible, and they are reported as such. Basic semantics look sound (frame,
landmarks, headings, named controls, role=status, table values), but actual
speech output, browse mode and verbosity remain unverified.

## Fix plan (M6.6.2–M6.6.6)

1. F1 — route the log screen correctly; router-level test.
2. F2 — error notice as `role="alert"`; test that the rendered block carries it.
3. F3 — live region (polite) for validation results; test.
4. F4 — focus the heading on step changes; verify on the real desktop.
5. F5 — `aria-busy` while running; test.
6. F6 — `scope`/labels on tables; test.
7. Re-run the full gate and the real desktop keyboard walk; keep F7/F8 as
   documented follow-ups.

All fixes are small and stay inside the GUI layer; no core, adapter or safety
behaviour changes.
