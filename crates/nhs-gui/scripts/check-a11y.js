#!/usr/bin/env node
// Accessibility regressions found by the M6.6 audit:
//  - the Build log screen must go through the full renderer (heading, status,
//    applied snapshot), not only the log lines;
//  - errors are announced (role=alert);
//  - the content area is marked busy while a job runs;
//  - a finished validation takes focus so the result is read;
//  - tables carry a name and column scopes.
//
// Run: node crates/nhs-gui/scripts/check-a11y.js

const { load, Node, findAll, find, distDir } = require("./dom-stub");
const fs = require("fs");
const path = require("path");

const sandbox = load(distDir);
sandbox.__setLang("en");

function base(overrides = {}) {
  return {
    version: 3,
    app: { name: "Nix Home Studio", version: "0.1.0", identifier: "dev.local.nhs.studio" },
    step: "discover",
    selected: 1,
    reviewed: true,
    busy: null,
    capabilities: { catalog: true, plan: true, validate: true, apply: true },
    catalog: { query: "", total: 0, searched: false, hits: [] },
    plan: { state: "ready", notes: [], files: [] },
    validation: null,
    apply: { state: "idle", snapshot: null, written: [], message: null },
    log: { status: null, lines: [] },
    history: null,
    switch: { available: false, reason_code: "switch_disabled", reason: "" },
    discovery: null,
    discovery_error: null,
    readiness: null,
    ...overrides,
  };
}

function fail(message) {
  console.error(message);
  process.exit(1);
}

// 1. Router: the log step must use renderLog, not renderLogLines.
const logPanel = new Node("section");
sandbox.__renderContent(
  base({
    step: "log",
    apply: { state: "applied", snapshot: "SNAP-1", written: ["a", "b"], message: null },
  }),
  logPanel,
);
const logHeadings = findAll(logPanel, (n) => n.tagName === "H2").map((n) => n.textContent);
if (!logHeadings.includes("Build log")) {
  fail(`log screen is missing its heading: ${JSON.stringify(logHeadings)}`);
}
if (!logPanel.textContent.includes("Applied snapshot SNAP-1")) {
  fail("log screen does not show the applied snapshot");
}

// 2. Errors are announced.
const error = sandbox.__renderError({ code: "refused", detail: "layout_not_editable", message: "x", hint: "y" });
if (error.getAttribute("role") !== "alert") {
  fail("error notice is not role=alert");
}

// 3. aria-busy while running, removed afterwards.
const busy = base({ step: "validation", busy: { kind: "validation", revision: 1, attempt: 1 } });
sandbox.__render(busy);
const content = sandbox.document.getElementById("panel-content");
if (content.getAttribute("aria-busy") !== "true") {
  fail("content is not marked aria-busy while a job runs");
}
const done = base({
  step: "validation",
  validation: {
    success: true,
    command: "home-manager build",
    millis: 5,
    kind: null,
    summary: null,
    hint: null,
    location: null,
    scratch: null,
  },
});
sandbox.__render(done);
if (content.getAttribute("aria-busy") !== undefined) {
  fail("aria-busy is still set after the job finished");
}

// 4. A finished validation takes focus.
const active = sandbox.document.activeElement;
if (!active || active.id !== "validation-result") {
  fail(`validation result did not take focus (active=${active && active.id})`);
}

// 5. Table semantics.
const historyPanel = new Node("section");
sandbox.__renderHistory(
  base({
    step: "history",
    history: {
      reason_code: "history_read_only",
      reason: "read-only",
      generations: [
        { id: 3, link: "/home/t/.local/state/nix/profiles/home-manager-3-link", target: "/nix/store/x-home-manager-generation", current: true },
      ],
      snapshots: [{ id: "S1", created: 0, outcome: "applied", files: 2 }],
    },
  }),
  historyPanel,
);
const tables = findAll(historyPanel, (n) => n.tagName === "TABLE");
if (tables.length === 0) fail("no history table");
for (const table of tables) {
  if (!table.attributes["aria-label"]) fail("history table has no accessible name");
  for (const th of findAll(table, (n) => n.tagName === "TH")) {
    if (th.getAttribute("scope") !== "col") fail("history header lacks scope=col");
  }
}

// The generation list marks the current profile target, and the section
// headings carry counts so the two lists cannot be confused.
const historyText = historyPanel.textContent;
if (!historyText.includes("Home Manager generations (1)")) {
  fail(`generation section count missing: ${historyText.slice(0, 200)}`);
}
if (!historyText.includes("Edit snapshots from this application (1)")) {
  fail("snapshot section count missing");
}
if (!historyText.includes("current")) {
  fail("current generation is not marked");
}

// 6. Skip link: present before the navigation, points at the main landmark,
// and moves focus to the screen heading instead of leaving it on the link.
const html = fs.readFileSync(path.join(distDir, "index.html"), "utf8");
const skipTag = html.match(/<a[^>]*class="skip-link"[^>]*>/);
if (!skipTag) fail("index.html has no skip link");
if (!/href="#main-content"/.test(skipTag[0])) {
  fail("skip link does not target #main-content");
}
const skipAt = html.indexOf('class="skip-link"');
const navAt = html.indexOf('id="steps"');
if (!(skipAt >= 0 && navAt > skipAt)) {
  fail("skip link is not before the navigation");
}
const mainTag = html.match(/<main[^>]*id="main-content"[^>]*>/);
if (!mainTag) fail("index.html has no #main-content landmark");
if (!/tabindex="-1"/.test(mainTag[0])) {
  fail("main landmark is not programmatically focusable");
}

sandbox.__setLang("zh");
sandbox.__render(base());
const skip = sandbox.document.getElementById("skip-to-content");
if (skip.textContent !== "跳到主要内容") {
  fail(`skip link is not localised: ${JSON.stringify(skip.textContent)}`);
}
sandbox.document.activeElement = null;
sandbox.__skipToContent();
const afterSkip = sandbox.document.activeElement;
if (!afterSkip || afterSkip.id !== "screen-title") {
  fail(`skip link did not focus the screen heading (active=${afterSkip && afterSkip.id})`);
}

// Run the real boot path so this also fails if `wireSkipLink()` is removed from
// `boot()`, then activate the link through its registered click listener.
(async () => {
  await sandbox.__boot();
  sandbox.document.activeElement = null;
  skip.click();
  const afterClick = sandbox.document.activeElement;
  if (!afterClick || afterClick.id !== "screen-title") {
    fail(`skip link click did not focus the screen heading (active=${afterClick && afterClick.id})`);
  }
  console.log(
    "a11y ok: log routing, alert errors, aria-busy, result focus, table semantics, skip link click",
  );
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
