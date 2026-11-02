#!/usr/bin/env node
// Regression test for the folded diff: <details> hides the diff, it does not
// truncate it. Runs the real app.js against the DOM stub.
//
// Run: node crates/nhs-gui/scripts/check-diff-fold.js

const { load, Node, find, distDir } = require("./dom-stub");

const sandbox = load(distDir);
sandbox.__setLang("en");

const total = 2500;
const lines = Array.from({ length: total }, (_, index) => `+ line ${index + 1}`);
lines[total - 1] = "+ THE LAST LINE";
const state = {
  version: 3,
  step: "plan",
  selected: 1,
  reviewed: false,
  busy: null,
  capabilities: { catalog: true, plan: true, validate: true, apply: true },
  catalog: { query: "", total: 0, searched: false, hits: [] },
  plan: {
    state: "ready",
    notes: [],
    files: [
      {
        path: "/tmp/example/home.nix",
        kind: "replace",
        added: total,
        removed: 0,
        diff: lines.join("\n"),
        noop: false,
      },
    ],
  },
  validation: null,
  apply: { state: "idle", snapshot: null, written: [], message: null },
  log: { status: null, lines: [] },
  history: null,
  switch: { available: false, reason_code: "switch_disabled", reason: "" },
  discovery: null,
  discovery_error: null,
  readiness: null,
};

const panel = new Node("section");
sandbox.__renderPlan(state, panel);

const details = find(panel, (node) => node.tagName === "DETAILS");
if (!details) {
  console.error("no <details> for a 2500-line diff");
  process.exit(1);
}
if (details.getAttribute("open") !== undefined) {
  console.error("<details> must start collapsed for a long diff");
  process.exit(1);
}

const pre = find(details, (node) => node.tagName === "PRE");
if (!pre) {
  console.error("the folded diff has no <pre>");
  process.exit(1);
}

const rendered = pre.textContent.split("\n");
if (rendered.length !== total) {
  console.error(`expanded diff has ${rendered.length} lines, expected ${total}`);
  process.exit(1);
}
if (!rendered.includes("+ THE LAST LINE")) {
  console.error("the last line of the diff is missing after expansion");
  process.exit(1);
}

// A short diff must not be wrapped in <details> at all.
const short = JSON.parse(JSON.stringify(state));
short.plan.files[0].diff = lines.slice(0, 10).join("\n");
const shortPanel = new Node("section");
sandbox.__renderPlan(short, shortPanel);
if (find(shortPanel, (node) => node.tagName === "DETAILS")) {
  console.error("a short diff must render directly, without <details>");
  process.exit(1);
}

console.log(`diff fold ok: ${total} lines kept, collapsed by default, last line present`);
