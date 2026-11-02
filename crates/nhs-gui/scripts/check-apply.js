#!/usr/bin/env node
// Checks the apply / history screens: the snapshot is shown, a failure is shown
// as a failure, and history never offers a rollback the backend does not have.
//
// Run: node crates/nhs-gui/scripts/check-apply.js

const { load, Node, findAll, find, distDir } = require("./dom-stub");

const sandbox = load(distDir);

function state(overrides = {}) {
  return {
    version: 3,
    step: "log",
    selected: 1,
    reviewed: true,
    busy: null,
    capabilities: { catalog: true, plan: true, validate: true, apply: true },
    catalog: { query: "", total: 0, searched: false, hits: [] },
    plan: {
      state: "ready",
      notes: [],
      files: [
        { path: "/tmp/hm/modules/nhs-packages.nix", kind: "create", added: 12, removed: 0, diff: "", noop: false },
        { path: "/tmp/hm/home.nix", kind: "replace", added: 1, removed: 1, diff: "", noop: false },
      ],
    },
    validation: { success: true, command: "home-manager build", millis: 12, kind: null, summary: null, hint: null, location: null, scratch: null },
    apply: { state: "applied", snapshot: "1793373332-1-1", written: ["/tmp/hm/home.nix", "/tmp/hm/modules/nhs-packages.nix"], message: null },
    log: { status: "The files were written. Nothing was activated.", lines: [{ stream: "status", text: "written" }] },
    history: null,
    switch: { available: false, reason_code: "switch_disabled", reason: "" },
    discovery: null,
    discovery_error: null,
    readiness: null,
    ...overrides,
  };
}

function render(step, overrides) {
  const panel = new Node("section");
  const value = state(overrides);
  value.step = step;
  if (step === "confirm") sandbox.__renderConfirm(value, panel);
  if (step === "log") sandbox.__renderLog(value, panel);
  if (step === "history") sandbox.__renderHistory(value, panel);
  return panel;
}

sandbox.__setLang("en");

// 1. Confirm lists every file that will be written, and nothing about activation.
const confirm = render("confirm");
const confirmText = confirm.textContent;
for (const needle of ["Confirm", "Files to write", "nhs-packages.nix", "home.nix", "does not activate them"]) {
  if (!confirmText.includes(needle)) {
    console.error(`confirm: missing ${JSON.stringify(needle)} in:\n${confirmText}`);
    process.exit(1);
  }
}

// 2. A successful apply names the snapshot and the file count.
const applied = render("log");
const appliedText = applied.textContent;
if (!appliedText.includes("1793373332-1-1") || !appliedText.includes("2 files")) {
  console.error(`log: snapshot line missing in:\n${appliedText}`);
  process.exit(1);
}

// 3. A failed apply is shown as a failure with its message, not as success.
const failed = render("log", {
  apply: { state: "failed", snapshot: null, written: [], message: "commit failed: permission denied" },
  log: { status: "writing failed", lines: [] },
});
const failedText = failed.textContent;
if (!failedText.includes("Applying failed") || !failedText.includes("permission denied")) {
  console.error(`log: failure not shown in:\n${failedText}`);
  process.exit(1);
}
if (failedText.includes("Nothing was activated")) {
  console.error("log: a failed apply must not claim success");
  process.exit(1);
}

// 4. History reflects the backend: read-only note, no rollback control, and the
//    snapshot it really has.
const history = render("history", {
  history: {
    reason_code: "history_read_only",
    reason: "Rolling back is not available in this version.",
    generations: [{ id: 3, link: "/home/t/.local/state/nix/profiles/home-manager-3-link", target: null }],
    snapshots: [{ id: "1793373332-1-1", created: 0, outcome: "applied", files: 2 }],
  },
});
const historyText = history.textContent;
for (const needle of ["History", "Rolling back is not available", "home-manager-3-link", "1793373332-1-1", "applied"]) {
  if (!historyText.includes(needle)) {
    console.error(`history: missing ${JSON.stringify(needle)} in:\n${historyText}`);
    process.exit(1);
  }
}
const historyButtons = findAll(history, (node) => node.tagName === "BUTTON");
if (historyButtons.length !== 0) {
  console.error(
    `history: found ${historyButtons.length} button(s); rollback is not implemented, so none may be offered`,
  );
  process.exit(1);
}

// 5. Chinese is not just an English fallback.
sandbox.__setLang("zh");
const zhHistory = render("history", {
  history: {
    reason_code: "history_read_only",
    reason: "Rolling back is not available in this version.",
    generations: [],
    snapshots: [],
  },
});
if (!zhHistory.textContent.includes("此版本不支持回滚")) {
  console.error(`history zh: missing translated read-only note in:\n${zhHistory.textContent}`);
  process.exit(1);
}

console.log("apply/history ok: snapshot shown, failure honest, history read-only with no rollback control");
