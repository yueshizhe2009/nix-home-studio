# GUI toolkit decision (M6.2)

Status: **draft — decided on measured facts, prototypes still outstanding.** This
file is the record the M6 plan asks for: a one-page comparison of the candidates
on single-binary packaging, Nix packaging effort, accessibility and runtime
dependencies, plus what was actually verified on this machine.

## The correction that changes the comparison

Earlier planning listed "migrate NixBox's front end" as an option. It is not one:

| Claim | Evidence |
| --- | --- |
| NixBox has **no desktop GUI** | `upstream/nixbox/crates/nixbox-tui/` depends on `ratatui 0.30` and `crossterm 0.29`; its sources are `app.rs`, `ui/`, `theme.rs`, `vim.rs` — a terminal UI |
| So nothing can be migrated to a desktop window | its interaction patterns (navigation, state machine, theme, vim keys) can inform the design; the code cannot |

## Candidates

| | Tauri 2 + web front end | iced 0.14 | Slint 1.18 | NixBox TUI |
| --- | --- | --- | --- | --- |
| Licence (from the published crate) | `Apache-2.0 OR MIT` | `MIT` | `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | Apache-2.0 |
| Fits this project's Apache-2.0 | yes | yes (attribution) | **only** under the royalty-free/commercial terms; the GPL option would relicense the whole app | yes |
| Single binary | app binary + **system** webview (WebKitGTK) | one static binary, no system UI library | one binary + platform windowing (winit) | n/a (terminal) |
| Runtime deps on Linux | `webkit2gtk-4.1`, `gtk+-3.0`, `libsoup-3`, `javascriptcoregtk-4.1` | none beyond GPU/display (`wgpu` → Vulkan/GL) | none beyond display | none |
| Nix packaging effort | needs a `wrapGAppsHook`/webkit fixups; common but fiddly | `cargo build` in a normal derivation | same, plus a `.slint` compile step | trivial |
| Accessibility | webview gives HTML semantics, focus order and screen-reader support for free; quality depends on the HTML written | own widget set; accessibility support exists in the ecosystem but must be verified for the widgets used | limited desktop a11y story | n/a |
| Dependencies in the graph | **405 crates** (measured: `cargo` reported 405 after resolving `tauri 2.12`) | not measured yet (needs download) | not measured yet | small |
| Buildable offline on this host | **no** — 405 crates, `adler2` was not in the local cache | no (not cached) | no (not cached) | n/a |
| System libraries present on this host | yes: `webkit2gtk-4.1 2.54.1`, `gtk+-3.0 3.24.52`, `javascriptcoregtk-4.1 2.54.1` | n/a | n/a | n/a |

## What was verified, and how

- Licences: downloaded each crate's published `Cargo.toml` from `static.crates.io`
  and read the `license` field. Not taken from documentation.
- System libraries: `pkg-config --modversion webkit2gtk-4.1` and `gtk+-3.0`.
- Dependency count: `cargo`'s own resolution output for a probe crate
  (`tauri 2.12` + `tauri-build 2.7`), 405 packages.
- Offline claim: `cargo build --offline` failed on `adler2 2.0.1`, so the earlier
  assumption "Tauri is fully cached" was wrong and is corrected here.
- NixBox: read its workspace manifests and source layout.

## What is **not** verified

- Whether iced or Slint build here; neither has been downloaded. Their dependency
  counts and a11y support are therefore unknown, not "worse".
- Whether a Tauri window renders acceptably with this host's WebKitGTK.
- Any performance comparison. Nothing has been benchmarked.
- Accessibility in practice: no screen reader was run against anything.

## Working recommendation (to be confirmed by the prototypes)

**Tauri**, with iced as the challenger, and Slint excluded unless the licence
question is settled in writing. Reasons, in order:

1. **The UI is where a beginner judges the app.** HTML/CSS gives layout, focus,
   scaling and screen-reader semantics without hand-building a widget toolkit;
   `nhs-app` already produces the exact view model such a UI renders.
2. **Licence is unambiguous** (`Apache-2.0 OR MIT`), unlike Slint's default.
3. **The runtime dependency is the known cost**, and it is a *known* one: Nix
   users already have WebKitGTK in their closure when they use such apps, and the
   flake can carry it.
4. iced remains attractive for a single self-contained binary with no webview;
   that is exactly the trade the prototype must measure.

The decision is **not** finalised: the M6 plan requires the same five flows
(search, module configuration, diff review, error presentation, rollback history)
built twice and compared, not screenshots. Until that exists this file records
evidence and a leaning, not a verdict.

## Tauri prototype — built and observed (2026-10-29)

`crates/nhs-gui-spike-tauri/` (excluded from the workspace; its own target dir
under `/var/tmp`). One window, seven steps, driving `nhs-app` through
`nhs-adapter`; the web layer only sends events and renders the state it gets back.

| Measurement | Result |
| --- | --- |
| Dependency graph | 405 crates |
| Cold build (after the graph was fetched) | ~13 minutes on this host |
| Warm rebuild after a UI change | 5–21 s |
| Binary (debug) | 214 MiB |
| UI code for the five flows | one `main.rs` (~330 lines: commands + state → JSON) and `dist/` (HTML 20, CSS 60, JS 330) |
| Extra runtime deps | `webkit2gtk-4.1`, `gtk+-3.0`, `javascriptcoregtk-4.1` (all present) |
| Running it | launches and renders; captured with `grim` |

Observed screens (screenshots taken on this machine, `/tmp/nhs-spike/shot-*.png`):

- **Discover** — layout `classic-home`, editable *yes*, root and main file, generations count.
- **Packages** — offline search for `firefox` returns one hit with its licence; the plan button is disabled until something is selected.
- **Plan & diff** — two diffs with `+12 −0` and `+1 −1`, real unified diff text.
- **Confirm** — warning, the two files, "Apply (build only)" and Cancel; the nav item "5 · Confirm" stays disabled until the review gate opens.

Headless five-flow run (`--selftest`, same code path, no window):

```
discover        step=discover    selected=0 plan=0 validation=none apply=idle
search firefox  step=catalog     selected=0 plan=0 validation=none apply=idle
select firefox  step=catalog     selected=1 plan=0 validation=none apply=idle
build plan      step=plan        selected=1 plan=2 validation=none apply=idle
review diff     step=plan        selected=1 plan=2 validation=none apply=idle
validate        step=validation  selected=1 plan=2 validation=ok   apply=idle
ask confirmation step=confirm    selected=1 plan=2 validation=ok   apply=idle
apply           step=log         selected=1 plan=2 validation=ok   apply=applied
history         step=history     selected=1 plan=2 validation=ok   apply=applied
```

After that run the scratch tree really contains the module, the injected import
and one snapshot, so the flow is not a mock.

### What running it found (things a compile cannot)

1. **`nhs-app` has no "please discover" event.** `StepRequested(Discover)` only
   navigates, so a UI cannot ask for detection. The prototype works around it by
   discovering in the shell at start-up; **this is a gap to close in `nhs-app`**.
2. **Reviewing before validating was impossible.** A new validation reset
   `ReviewState`, so the natural order (read the diff, then validate, then
   confirm) could never reach the confirmation screen. Fixed in `nhs-app` with a
   regression test (`reviewing_before_validating_survives_the_validation`).
3. **Layout:** with two diffs on a 760 px window the "I have read this diff"
   checkbox and the Validate button fall below the fold. The panel scrolls, but a
   real design needs the actions pinned.
4. **Colour:** the `switch` notice is rendered in the error colour on every
   screen; it is a limitation, not an error, and should read as neutral.

## iced prototype — built and observed (2026-10-29)

`crates/nhs-gui-spike-iced/` (own workspace, target dir `/var/tmp/nhs-iced`).
`main.rs` + `i18n.rs`, seven steps, the same `nhs-app` + `nhs-adapter`, the same
`--selftest` sequence and the same scratch HOME.

| Measurement | Result |
| --- | --- |
| Dependency graph | 408 crates |
| Cold build | ~5 min (4 m 15 s for the graph, 25 s for the app) |
| Warm rebuild after a UI change | 12–25 s |
| Binary (debug) | 365 MiB |
| UI code for the five flows | `main.rs` + `i18n.rs`, ~820 lines total (of which ~215 are the two string tables) |
| Extra runtime deps | none: no WebKitGTK, no GTK (`webkit`/`gtk` appear 0 times in its lock) |
| Accessibility support in the graph | **no `accesskit`** (0 occurrences); see the feasibility table below |
| Running it | launches and renders; captured with `grim` (discover, packages in `zh`, via `NHS_SPIKE_SCREEN` / `NHS_SPIKE_LANG`) |

Headless run (`--selftest`), identical to the Tauri transcript:

```
discover … step=discover   plan=0 validation=none
search …   step=catalog    plan=0
select …   step=catalog    selected=1
plan …     step=plan       plan=2
validate … step=validation validation=ok
confirm …  step=confirm    validation=ok
apply …    step=log        apply=applied
history …  step=history    apply=applied
```

Observed screens: the same **Plan & diff** and **Confirm** as the Tauri spike,
rendered in iced's default dark theme, plus **Discover** and **Packages** with the
Chinese table (active step in primary blue, actions in a fixed row below the
scrollable).

### What the iced prototype showed

1. **Rendering is not the problem.** Monospace diffs, wrapped long paths, buttons
   and a checkbox all render cleanly with no styling code beyond widget calls.
2. **No sticky footer** (found on the first run). iced lays widgets out in a
   column, so the review checkbox and Validate button scrolled away with the
   diffs. **Now fixed:** each screen returns content *and* an action row, and the
   action row is rendered outside the `scrollable`. Tauri got the same result
   from one CSS rule.
3. **All seven steps looked active** (found on the first run), because iced's
   default `button` style is the primary one. **Now fixed:** the current step
   uses `button::primary`, the others `button::secondary`.
4. **No accessibility bridge in the graph** — still true, and now measured
   rather than inferred (see below).

## Accessibility, measured (2026-10-29, re-run 2026-10-30)

Two environment facts had to be established first, because both can make an
application look inaccessible when it is not:

1. **`NO_AT_BRIDGE=1` was set in this shell**, which disables GTK's AT-SPI
   bridge. With it unset, GTK apps register.
2. **`org.a11y.Status.IsEnabled` was `false`.** GTK can be forced with
   `GTK_A11Y=1`; **AccessKit has no such override** — it only registers while
   this property is true and it reacts to the property-change signal. On this
   niri host `gsettings set org.gnome.desktop.interface toolkit-accessibility
   true` did **not** move the property (no GNOME settings daemon to sync it);
   it only changed after
   `Properties.Set org.a11y.Status IsEnabled true` on `org.a11y.Bus`.
   The flags were restored to `false` after the checks.

A standalone probe (`/var/tmp/nhs-a11y-probe`, winit 0.30 + `accesskit_winit`
0.29.2 + `accesskit` 0.21, no iced) proves the platform path works on this
machine: once `IsEnabled` was true, `orca computer list-apps` listed
`a11y-probe`, and the tree read back as

```
0 frame a11y probe
    1 button Probe button
```

So the missing tree in the iced prototype is **not** a host limitation.

| Check | Tauri 2 + WebKitGTK | iced 0.14 |
| --- | --- | --- |
| Registered with AT-SPI | **yes** — appears in `orca computer list-apps`, with the desktop a11y flag on | **no** — running, flag on, still absent from `list-apps` |
| Accessible names | every control: `button 1 · 发现`, `entry 在精选目录中搜索`, `check box firefox — Web browser`, `button Switch language` | none exposed |
| Structure | landmarks (`Steps`), headings (检测结果), a real `table → table row → column header / cell` tree | none exposed |
| State visible to AT | footer landmark `步骤：catalog · 已选：0 · Build a plan first.`, which changes as the flow advances | none |
| Input through AT | `click --element-index` switched the language (heading 检测结果/软件包) and the screen (footer `步骤：discover`) | not possible |
| Keyboard typing | injected with XTEST and typed into the search field | typed into the search field by the user after clicking it (no Tab focus exists) |
| Keyboard-only walk | **done** — the whole flow works, see below | **done** — Tab/Enter/Space do nothing, see below |

Reading the tree also found a defect in my own markup: the language button sat
**inside** the `<h1>` and was therefore not exposed as a button. It is now a
toolbar button with `aria-label="Switch language"`, read back as
`button Switch language`.

What this does **not** prove: that a screen reader reads either app *usefully*
(no Orca speech session was run) or that keyboard focus order is sane (no key
events could be injected). It also says nothing about HiDPI, memory or start-up
time.

### Can iced 0.14 be given AccessKit? (investigated 2026-10-30)

| Question | Finding | How it was established |
| --- | --- | --- |
| Is AccessKit already wired into iced 0.14? | **No.** Zero `accesskit` references in `iced`, `iced_core`, `iced_widget`, `iced_winit`, `iced_renderer`, `iced_runtime`; no `a11y` feature in `iced-0.14.0/Cargo.toml` | source of the vendored crates |
| Does the widget API expose anything to build a11y nodes from? | **No.** `Widget` exposes only `tag/state/children/operate`; `Application`/`Program` have no a11y or window-event hook | `iced_core-0.14.0/src/widget.rs`, `iced-0.14.0/src/application.rs` |
| Is there a hook below the widget layer? | **No.** `iced_winit` exposes only `pub fn run<P: Program>()`; `run_instance` (which owns the event loop and creates the window) is private | `iced_winit-0.14.1/src/lib.rs` |
| Could `accesskit_winit` be dropped in? | **The crates are compatible** — iced uses winit 0.30.13 and `accesskit_winit` 0.29.2/0.34.2 need winit `^0.30` — but there is no place to call it from | `Cargo.lock` + the probe above |
| What would it take? | Fork/vendor `iced_winit` to create the adapter at window creation and forward `WindowEvent`s, **plus** a widget→node mapping (none exists; widgets would need a11y metadata or the app a parallel tree). That is a maintained fork, not prototype work | inference from the two points above |
| Upstream state | issue **#552** (`Implement accessibility support`) open since 2020-10-05; PRs #1849 and #3111 open, #3281 closed unmerged on 2026-03-14 with "I'll work on this myself"; roadmap places screen-reader support after the next release | GitHub issue/PR metadata |

**Conclusion for this milestone:** iced 0.14 cannot expose a widget tree to
AT-SPI without forking the windowing crate. This is a **framework limitation at
this version**, not a property of the prototype's code — which is why the
prototype is kept and the limitation is recorded instead of being used as a
silent elimination criterion.

## Bilingual UI

Both prototypes keep every visible string in one table and switch at runtime.

| | Tauri | iced |
| --- | --- | --- |
| Where the strings live | `dist/i18n.js` (`en`/`zh` objects, `t(key, values)`, `{name}` placeholders) | `src/i18n.rs` (`EN`/`ZH` consts, `fill()` with positional `{}`) |
| Keys | ~55 | ~55, same names |
| Toggle | toolbar button (`button Switch language`) | `Message::ToggleLanguage`, header button |
| Verified | switched **through the accessibility tree**; tree then reports `Nix 家庭工作室 原型`, `1 · 发现`, `在精选目录中搜索`, and the footer keeps `步骤：catalog · 已选：0` | rendered with `NHS_SPIKE_LANG=zh`; the toggle itself cannot be clicked through AT (no tree), so its effect is asserted headlessly in `--selftest`: toggling does not change the step |
| Cost | edited one JS file, no Rust changes | new module, every `view_*` signature had to take the table, plus an `impl Display`-free `fill()` helper |

Data that comes back from the engine (paths, diagnostics, the `switch`
refusal, `Build a plan first.`) stays in the engine's English wording in both
prototypes — translating it would mean translating core messages, which is a
product decision, not a toolkit one.

## Start-up, memory and HiDPI, measured (2026-10-30)

Same method for both, script at `/tmp/nhs-measure.py` and `/tmp/nhs-scale.py`
(throw-away HOME/state, no root, no compositor reconfiguration):

- **Start-up** = spawn → the compositor lists the window. Under niri a window is
  listed when the client commits its first buffer, so this is close to
  "first frame visible" rather than "process alive".
- **Memory** = summed `VmRSS` and summed `Pss` over the whole process tree after
  six idle seconds. PSS is the fairer number: it splits shared library pages
  instead of counting them once per process.
- **HiDPI** = the toolkit's own scale path, because the compositor has one 1×
  output here: `GDK_SCALE=2` for Tauri, `WINIT_X11_SCALE_FACTOR=2` on the X11
  backend for iced.

| Measurement | Tauri 2 | iced 0.14 |
| --- | --- | --- |
| Start-up, warm cache (n=5) | **median 0.98 s** (0.84–2.32) | **median 1.22 s** (1.07–1.38) |
| Start-up, first launch after a build | 2.3 s | 9.1 s (GPU/shader cache cold) |
| Memory, Discover (RSS / PSS / processes) | 421 MiB / **211 MiB** / 6 | 92 MiB / **60 MiB** / 1 |
| Memory, Plan & diff (RSS / PSS) | 422 / 210 MiB | 95 / **63 MiB** |
| Debug binary | 214 MiB | 365 MiB |
| HiDPI 2× forced | scales (text and controls doubled) | scales (same) |
| At 2× with a 714×768 px window | both clip: the logical viewport halves to 357×384 and the fixed 190 px nav plus the body no longer fit | same |
| Fractional scaling (1.25/1.5) | not tested | not tested |

Reading these honestly:

1. **iced starts slower than Tauri here** (~+0.24 s median), which contradicts the
   "no webview, therefore faster start" intuition. The first launch after a build
   is the opposite way round by a wide margin (9 s vs 2 s) — that is wgpu building
   its pipeline cache, and it is a one-off per build.
2. **iced uses roughly a third of the resident memory** (60 vs 211 MiB PSS) and a
   single process instead of six. This is the clearest measured advantage iced has.
3. **HiDPI is a wash at integer 2×**: both honour it, both were forced through an
   env-var path because Wayland scale comes from the compositor. What the test
   does *not* show is behaviour on a real HiDPI panel: the window would still be
   1180×760 *logical*, whereas this test halved the logical viewport and exposed a
   layout assumption in my own prototype (fixed 190 px nav), not a toolkit defect.
   True Wayland 2× and fractional scaling remain unverified — testing them means
   changing the session's output scale, which this project does not do.

## Keyboard and assistive-technology walkthrough (2026-10-30)

### How it was done, and what it is not

- **No screen reader and no speech synthesizer are installed** on this host (no
  GNOME Orca, no `speech-dispatcher`). Installing them is a system package
  change, which this project does not do. The substitute is an AT-SPI consumer:
  `pyatspi` attached to the same accessibility bus, printing the role, name,
  value and state of every focus change — the interface a screen reader consumes,
  **not** its speech output.
- **Keys were injected with XTEST** into the XWayland window (`/tmp/xkey`), because
  no Wayland key-injection client (`wtype`/`ydotool`) exists here and niri has no
  key action. Both prototypes therefore ran with `GDK_BACKEND=x11` (Tauri) and
  without `WAYLAND_DISPLAY` (iced). The applications' own key handling is the
  same, the input transport is not.
- The desktop is in use while these tests run: the Orca app window re-activates
  every second or two, and focus stolen mid-sequence sends keys to the wrong
  window. Every key is therefore sent as "focus target, 50 ms, inject, 350 ms,
  verify", and the walkthrough asserts the effect (screen/focus changed) instead
  of trusting the injection.
- Injected synthetic events via `XSendEvent` were tried first and **are dropped by
  GTK/WebKit**, so they are not used.

### Tauri 2 + WebKitGTK — keyboard only: works

The whole five-flow chain was driven with no pointer: Packages → type `firefox`
→ Enter → Space to select → Enter to build the plan → Space to acknowledge the
diff → Enter to validate → Enter to review and apply → Enter to apply → Enter to
open History. Evidence: the AT-SPI tree reached `heading History`, the history
table shows `applied` with 2 files, and the scratch tree contains the applied
snapshot — so this was a real apply, not a mock.

| Check | Result |
| --- | --- |
| Tab order | `Switch language` → `1 · Discover` → `2 · Packages` → `3 · Plan & diff` → `4 · Validate` → *(skips disabled `5 · Confirm`)* → `Build log` → `6 · History` → `Detect again` → `Choose packages`; wraps to the browser chrome and back |
| Focus visibility | visible focus ring (screenshot `kb-tauri-focus.png`) |
| Enter / Space | Enter activates buttons and submits the search; Space toggles the package checkbox and the review checkbox |
| Typing | real key presses filled the search field and the query was submitted with Enter |
| Scroll area | at 360 px height the sticky action row stays visible and the diff area scrolls with focus (`scroll2-before/after.png`); the actions never fall below the fold |
| Dialogs | none: nothing in the flow is modal, the confirmation is a screen, so there is no trap to test |
| **Defect found, then fixed** | focus was lost after every state update (DOM rebuild), so a keyboard user had to re-tab from the top after each step. Fixed on 2026-10-30 and re-verified — see *Focus restore* below. |

### iced 0.14 — keyboard only: does not work

| Check | Result |
| --- | --- |
| Keys reach the window | **yes** — `XGetInputFocus` reports the iced window (`0x800004`, title matches) |
| Tab traversal | **no** — three Tabs change the screen by **0 pixels** |
| Enter / Space | **no** — no pixel change, no state change |
| Focus indicator | none exists to show |
| Mouse | the user clicked the search field and typed into it, so the widget itself accepts text once focused by pointer |
| Dialogs | none |

So iced's gap is not input delivery and not drawing: the prototype has no focus
system, and stock iced 0.14 gives the application nothing to build one from
(`Widget` has no focus/a11y hooks, and there is no keyboard-navigation API in the
crate). This is the same root cause as the missing accessibility tree.

### What a screen reader can read (Tauri, AT-SPI)

The focus event stream during the walkthrough — what an AT consumer receives, and
the basis of any announcement:

```
[focus] button: Switch language
[focus] button: 1 · Discover
[focus] button: 2 · Packages
[focus] entry: Search the curated catalogue
[focus] button: Search
[focus] check box: firefox — Web browser
[focus] check box: firefox — Web browser (checked)
[focus] button: Build plan for 1 package(s)
[focus] check box: I have read this diff (checked)
[focus] button: Validate in a scratch copy
[focus] button: Review and apply
[focus] button: Apply (build only)
[focus] button: History
```

| Item the user asked about | Exposed to AT? |
| --- | --- |
| Page title | frame name `Nix Home Studio — prototype` (window title) and `document web` name |
| Step | `Steps` landmark with seven named buttons; the current step is `aria-current` in the DOM, which AT-SPI does not carry, so a reader cannot announce "current" |
| Search box | named entry `Search the curated catalogue`, plus its value |
| Package selection | `check box firefox — Web browser`, state `checked` after Space |
| Diff | file headings (path + `+12 −0`) as headings; the diff text itself is exposed as the section's **Value** (`--- a/nhs-packages.nix …`), not as a name or text node — readable through the AT-SPI value interface, but whether a reader announces it while browsing is **unverified** |
| Validation result | static text in the panel (`The build succeeds in the scratch copy.`) — present in the tree, no live region |
| Confirmation | `button Apply (build only)` and `button Cancel`, both named |
| Status line | footer landmark value `step: catalog · selected: 0 · Build a plan first.` — updated on every step, but **not a live region**, so a reader is not told when it changes |

**Still unverified:** actual speech (no synthesizer), browse-mode reading order and
verbosity, whether the diff's Value is announced, and whether a reader notices the
footer updates. Those need Orca installed, or the user driving a reader on a
machine that has one.

### Focus restore — fixed and re-verified (2026-10-30)

`dist/app.js` now captures focus **before** the panel is rebuilt and restores it
**after**, using a stable key per control rather than a node reference:

- every focusable control carries `data-focus` (`lang`, `nav:<step>`,
  `query`, `search`, `pkg:<attr>`, `plan`, `reviewed`, `validate`,
  `review-apply`, `apply`, `cancel`, `history`, …);
- the text cursor position is captured and restored for the search field;
- same screen → same control is focused again;
- screen changed → the new screen's `<h2>` (`tabindex="-1"`) takes focus, so a
  reader announces the new page and the next Tab starts at that screen's first
  control instead of the document top;
- the recorded control exists but is now disabled → focus moves to the nearest
  enabled control in document order, never forced onto a disabled one;
- nothing was focused before → nothing is focused after (no surprise focus grab,
  no loop back to the language button).

Re-run of the acceptance table, driven with real keys and read from AT-SPI
(`/var/tmp/nhs-interaction/walk2.py`):

| Check | Result |
| --- | --- |
| after navigating to Packages | `button '2 · Packages'` |
| search field focused | `entry 'Search the curated catalogue'` |
| after searching | **stays** in `entry 'Search the curated catalogue'` |
| package focused | `check box 'firefox — Web browser'` |
| after selecting the package | **stays** on `check box 'firefox — Web browser'` |
| after building the plan | `heading 'Plan and diff'` |
| after marking the diff read | **stays** on `check box 'I have read this diff'` |
| after validation | `heading 'Validation'` |
| after confirming | `heading 'Confirm'` |
| after applying | `heading 'Build log'` |
| after history | `heading 'History'` |
| disabled `5 · Confirm` | never takes focus while tabbing |

**12/12 passed**, and the run still applied for real: the scratch tree contains
`modules/nhs-packages.nix`, the injected import and one snapshot.

One measurement trap found and documented: querying AT-SPI through a freshly
spawned process took ~1 s, long enough for the Orca window to steal the seat and
for WebKit to clear the focused state, which produced false "(nothing focused)"
readings. The checks are only valid when the query is immediate and the window is
active — the failing intermediate runs were harness noise, not app behaviour.

## Comparison on the agreed checklist

| Dimension | Tauri 2 + web UI | iced 0.14 |
| --- | --- | --- |
| Cost | 405 crates; Rust 330 + JS 330 + HTML/CSS 80 lines; one extra language (JS) | 408 crates; ~820 lines of Rust across `main.rs` + `i18n.rs`; one language |
| Cold build | ~13 min | ~5 min |
| Warm build | 5–21 s | 12–25 s |
| Warm start (to window) | 0.98 s median | 1.22 s median |
| Memory (PSS idle) | 211 MiB in 6 processes | 60 MiB in 1 process |
| HiDPI 2× | scales; untested on a real HiDPI output | scales; untested on a real HiDPI output |
| Debug binary | 214 MiB | 365 MiB |
| Runtime deps | WebKitGTK 4.1, GTK 3, JavaScriptCoreGTK | none beyond display/GPU |
| Interaction | native HTML form controls, scroll, text selection | custom widgets; every affordance hand-built |
| Sticky actions | one CSS rule (done) | explicit layout row (done) |
| Current-step marker | `aria-current="step"` + CSS (done) | `button::primary` (done) |
| Keyboard / focus | **works end-to-end** (Tab/Enter/Space/typing, visible ring, focus restored after each update) | **does not work**: Tab/Enter/Space change nothing; no focus system |
| Screen-reader exposure | full AT-SPI tree, focus events, diff as node Value; no live region; speech unverified | **none**; adding it needs a forked `iced_winit` |
| i18n | one JS table + DOM re-render | one Rust table threaded through every view |
| Licence | Apache-2.0 OR MIT | MIT |
| Upgrade risk | webview behaviour varies by distro version | wgpu/GPU driver surface; pure Rust toolchain |
| Maintenance | two languages, one DOM framework choice later | one language, no webview |

**Not measured:** screen-reader speech output (no synthesizer and no screen
reader installed), browse-mode reading order, true Wayland HiDPI (one 1× output,
and the project does not change the session's scale), fractional scaling,
cold-cache start-up (needs root to drop the page cache), memory under a
long-running session.

## Prototype plan (remaining)

**Method decided (2026-10-29):**

1. Build both prototypes with the same five flows and the same checklist.
2. Run each one, and observe it through Orca (start, capture the screen, read it,
   close). Real running beats a compile: a successful build says nothing about
   layout, interaction or legibility.
3. If Orca cannot reliably control the window or capture what is on screen, fall
   back to the user running it and filling the checklist by hand.
4. Record what was actually observed, plus what could not be verified. An empty
   probe window is **not** evidence that a five-screen app works.

**iced is in scope:** built for real rather than compared on paper. Neither
candidate was favoured in advance.

Comparison checklist (identical for both):

| Dimension | What is checked |
| --- | --- |
| Cost | lines of code, adapter glue and extra dependencies for the five flows |
| Interaction | page switching, forms, search, error presentation, confirmation |
| Accessibility | keyboard operation, focus order, what a screen reader can identify |
| Build and run | actual start-up, dependency build, resource use, failure modes |
| Maintenance risk | licence, system dependencies, upgrade cost |

| Step | Work | Status |
| --- | --- | --- |
| 1 | Shared adapter (`nhs-core` + `nhs-app` → events/intents) so both UIs drive identical logic | done (`crates/nhs-adapter`) |
| 2 | Tauri spike: five screens on real data | done, observed |
| 3 | iced spike: the same five screens | done, observed |
| 4 | Fill the table above from actually running both | done, except screen-reader speech |
| 5 | Record the decision | **decided 2026-10-30: Tauri** |

## Decision (2026-10-30): Tauri 2 + WebKitGTK

**Chosen: Tauri 2.** The user decided after the keyboard and assistive-technology
round; this section records the evidence behind it, not a re-litigation.

| Basis | Tauri | iced 0.14 |
| --- | --- | --- |
| Keyboard-only operation | complete five-flow run, 12/12 checks | Tab/Enter/Space change 0 pixels while the keys demonstrably reach the window |
| Assistive-technology exposure | full AT-SPI tree, named controls, states, focus events, AT actions work | nothing exposed, and nothing to expose it with in 0.14 |
| Cost of closing the gap | a focus-restore fix in the prototype (done) | a maintained fork of `iced_winit` plus a widget→node mapping, with upstream #552 open since 2020 |
| Memory | ~211 MiB PSS, 6 processes | **~60 MiB PSS, 1 process — measured advantage, kept on the record** |
| Start-up | 0.98 s median | 1.22 s median; 9 s on the first launch after a build |
| Build | ~13 min cold, 405 crates, needs WebKitGTK 4.1 + GTK 3 | ~5 min cold, 408 crates, no webview |

Why the memory advantage did not outweigh the rest: a desktop app that cannot be
driven from the keyboard and exposes nothing to a screen reader fails the two
requirements that are hardest to retrofit. The webview dependency is a packaging
cost; the missing focus system and accessibility tree are a capability cost.

What this does **not** settle: screen-reader speech quality, browse-mode order,
whether the diff's Value is announced, true Wayland HiDPI, and packaging a
WebKitGTK runtime for the target distros. Those are follow-up work, not reasons
to reopen the toolkit choice without new evidence.

**Gate:** M6.1 and **M6.2 are accepted (2026-10-30)**. The toolkit is locked:
Tauri 2 + WebKitGTK; the iced prototype is kept deliberately for its low-memory
comparison data, not as a fallback. Formal GUI development starts at M6.3 with a
runnable application skeleton.
