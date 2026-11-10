#!/usr/bin/env node
// M7 (hm) WP1: every recognised layout must explain itself, and an unknown
// layout must show the engine's own reason instead of a generic guess.
//
// Run: node crates/nhs-gui/scripts/check-discovery.js

const { load, Node, distDir } = require("./dom-stub");

const sandbox = load(distDir);
sandbox.__setLang("en");

function base(discovery) {
  return {
    version: 3,
    app: { name: "Nix Home Studio", version: "0.1.0", identifier: "dev.local.nhs.studio" },
    step: "discover",
    selected: 0,
    reviewed: false,
    busy: null,
    capabilities: { catalog: true, plan: true, validate: true, apply: true },
    catalog: { query: "", total: 0, searched: false, hits: [] },
    plan: null,
    validation: null,
    apply: { state: "idle", snapshot: null, written: [], message: null },
    log: { status: null, lines: [] },
    history: null,
    discovery,
    discovery_error: null,
    readiness: null,
    switch: { available: false, reason_code: "switch_disabled", reason: "disabled" },
  };
}

function render(discovery, lang = "en") {
  sandbox.__setLang(lang);
  const panel = new Node("section");
  sandbox.__renderContent(base(discovery), panel);
  return panel.textContent;
}

function fail(message) {
  console.error(message);
  process.exit(1);
}

const cases = [
  ["classic-home", "classic layout"],
  ["flake-home", "defined in a flake"],
  ["flake-nixos", "system NixOS flake"],
  ["profile-only", "No Home Manager configuration file"],
  ["unknown", "refused to guess"],
];

for (const [kind, phrase] of cases) {
  const text = render({
    kind,
    editable: kind === "classic-home" || kind === "flake-home",
    root: null,
    main_file: null,
    generations: 0,
    reason: null,
    current_generation: null,
    current_generation_link: null,
  });
  if (!text.includes(phrase)) {
    fail(`${kind}: explanation missing ${JSON.stringify(phrase)} in ${JSON.stringify(text.slice(0, 160))}`);
  }
}

const reason = "config_home_flake and config_home_classic were both accepted";
const unknown = render({
  kind: "unknown",
  editable: false,
  root: null,
  main_file: null,
  generations: 0,
  reason,
});
if (!unknown.includes(reason)) {
  fail("unknown layout does not show the engine reason");
}
if (!unknown.includes("Read-only layout")) {
  fail("unknown layout is not marked read-only");
}

const zh = render(
  {
    kind: "profile-only",
    editable: false,
    root: "/home/t/.local/state/nix/profiles",
    main_file: null,
    generations: 3,
    reason: null,
  },
  "zh",
);
if (!zh.includes("没有找到 Home Manager 配置文件")) {
  fail("Chinese profile-only explanation is missing");
}

const current = render({
  kind: "profile-only",
  editable: false,
  root: "/home/t/.local/state/nix/profiles",
  main_file: null,
  generations: 3,
  reason: null,
  current_generation: 3,
  current_generation_link: "/home/t/.local/state/nix/profiles/home-manager-3-link",
});
if (!current.includes("Generation (resolved or inferred)")) {
  fail("generation label does not distinguish resolved from inferred");
}
if (!current.includes("highest generation id")) {
  fail("generation resolution note is missing");
}
if (!current.includes("home-manager-3-link")) {
  fail("current generation link is missing");
}

console.log(
  "discovery ok: five layout explanations, unknown reason, read-only notice, resolved/inferred generation",
);
