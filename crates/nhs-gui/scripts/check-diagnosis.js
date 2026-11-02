#!/usr/bin/env node
// Checks the validation screen: diagnosis labels and hints come from the
// translated tables, the raw engine detail is kept, and the cancelled case reads
// as interrupted rather than as a mystery failure.
//
// Run: node crates/nhs-gui/scripts/check-diagnosis.js

const { load, Node, find, distDir } = require("./dom-stub");

const sandbox = load(distDir);

function base(validation, overrides = {}) {
  return {
    version: 3,
    step: "validation",
    selected: 1,
    reviewed: true,
    busy: null,
    capabilities: { catalog: true, plan: true, validate: true, apply: true },
    catalog: { query: "", total: 0, searched: false, hits: [] },
    plan: { state: "ready", notes: [], files: [] },
    validation,
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

function render(state) {
  const panel = new Node("section");
  sandbox.__renderValidation(state, panel);
  return panel.textContent;
}

function expect(label, text, needles, forbidden = []) {
  for (const needle of needles) {
    if (!text.includes(needle)) {
      console.error(`${label}: missing ${JSON.stringify(needle)} in:\n${text}`);
      process.exit(1);
    }
  }
  for (const needle of forbidden) {
    if (text.includes(needle)) {
      console.error(`${label}: unexpected ${JSON.stringify(needle)} in:\n${text}`);
      process.exit(1);
    }
  }
}

sandbox.__setLang("zh");
const failed = render(
  base({
    success: false,
    command: "home-manager build -f home.nix",
    millis: 1200,
    kind: { code: "syntax-error", label: "syntax-error" },
    summary: "error: syntax error, unexpected '}'",
    hint: "Fix the Nix syntax at the reported location.",
    location: "/tmp/home.nix:4:1",
    scratch: "/tmp/scratch-1",
  }),
);
expect(
  "zh diagnosis",
  failed,
  ["构建失败", "语法错误", "请修正报告位置处的 Nix 语法。", "error: syntax error", "/tmp/home.nix:4:1", "/tmp/scratch-1"],
  ["Fix the Nix syntax"],
);

sandbox.__setLang("en");
const failedEn = render(
  base({
    success: false,
    command: "home-manager build -f home.nix",
    millis: 900,
    kind: { code: "network", label: "network" },
    summary: "error: unable to download",
    hint: "The build needed the network and could not reach it",
    location: null,
    scratch: null,
  }),
);
expect("en diagnosis", failedEn, ["The build failed", "Network problem", "check the connection"]);

const succeeded = render(
  base({
    success: true,
    command: "home-manager build -f home.nix",
    millis: 800,
    kind: null,
    summary: null,
    hint: null,
    location: null,
    scratch: null,
  }),
);
expect("success", succeeded, ["The build succeeds in the scratch copy"]);

sandbox.__setLang("zh");
const cancelled = render(
  base({
    success: false,
    command: "home-manager build -f home.nix",
    millis: 300,
    kind: { code: "interrupted", label: "interrupted" },
    summary: "the command was stopped before it finished",
    hint: "Nothing was applied. Run the build again when you are ready.",
    location: null,
    scratch: null,
  }),
);
expect("cancelled", cancelled, ["已中断", "没有写入任何东西"]);

// While the worker runs, the screen says so and shows the streamed log.
const running = render(
  base(null, {
    busy: { kind: "validation", revision: 1, attempt: 1 },
    log: {
      status: null,
      lines: [
        { stream: "stdout", text: "building..." },
        { stream: "stderr", text: "warning: something" },
      ],
    },
  }),
);
expect("running", running, ["正在临时副本中校验", "building...", "warning: something"]);

console.log("diagnosis ok: labels and hints translated, raw detail kept, cancelled reads as interrupted");
