#!/usr/bin/env node
// Checks that both language tables are complete and that every dynamic code the
// backend can send has a translation.
//
// The code lists are *derived from the Rust sources* — the `RefusalCode::code()`
// match, `App::switch_reason_code()`, `event_key()`, `step_key()` and the
// `BridgeError` constructors — so adding a code without a translation fails here
// instead of slipping through a hand-written list.
//
// Run: node crates/nhs-gui/scripts/check-i18n.js

const fs = require("fs");
const path = require("path");
const vm = require("vm");

const here = __dirname;
const repo = path.join(here, "..", "..", "..");
const read = (relative) => fs.readFileSync(path.join(repo, relative), "utf8");

// --- the tables ------------------------------------------------------------

const source = read("crates/nhs-gui/dist/i18n.js");
const sandbox = {
  document: { documentElement: {} },
  localStorage: { getItem: () => null, setItem: () => {} },
};
vm.createContext(sandbox);
// `const` stays lexical inside the sandbox; export it explicitly.
vm.runInContext(`${source}\n;globalThis.__nhsI18N = I18N;`, sandbox);
const I18N = sandbox.__nhsI18N;

const languages = Object.keys(I18N);

// Duplicate keys are legal JavaScript and silently overwrite, so the loaded
// object cannot reveal them. Scan the source: keys sit at exactly four spaces
// inside a two-space language block (`en: {` … `},`).
function duplicateKeys(source) {
  const duplicates = {};
  let language = null;
  const seen = new Map();
  for (const line of source.split("\n")) {
    const start = line.match(/^ {2}([A-Za-z_]+): \{$/);
    if (start) {
      language = start[1];
      seen.set(language, new Map());
      continue;
    }
    if (language && line === "  },") {
      language = null;
      continue;
    }
    if (!language) continue;
    const key = line.match(/^ {4}([A-Za-z0-9_]+):/);
    if (!key) continue;
    const counts = seen.get(language);
    const name = key[1];
    counts.set(name, (counts.get(name) ?? 0) + 1);
    if (counts.get(name) === 2) {
      duplicates[language] = duplicates[language] ?? [];
      duplicates[language].push(name);
    }
  }
  return duplicates;
}

const duplicates = duplicateKeys(source);
if (Object.keys(duplicates).length > 0) {
  for (const [language, keys] of Object.entries(duplicates)) {
    console.error(`${language}: duplicate keys ${keys.join(", ")}`);
  }
  process.exit(1);
}
if (languages.length < 2) {
  console.error("expected at least two languages, found", languages);
  process.exit(1);
}

// --- code lists, parsed from Rust -----------------------------------------

/// All string literals in the arm bodies of `fn <name>(`, up to the closing brace
/// of its match. Rust here is simple: `Self::Variant => "code",`.
function armsOf(relative, fnName, pattern) {
  const text = read(relative);
  const start = text.indexOf(`fn ${fnName}(`);
  if (start < 0) throw new Error(`fn ${fnName} not found in ${relative}`);
  const body = text.slice(start, text.indexOf("\n    }", start));
  const codes = new Set();
  for (const match of body.matchAll(pattern)) codes.add(match[1]);
  return [...codes].sort();
}

const refusalCodes = armsOf(
  "crates/nhs-app/src/lib.rs",
  "code",
  /Self::[A-Za-z]+ => "([a-z_]+)",/g,
);
const switchCodes = armsOf(
  "crates/nhs-app/src/lib.rs",
  "switch_reason_code",
  /"([a-z_]+)"/g,
);
// Job kinds, used in `busy` errors as `action_<kind>`.
const bridgeSource = read("crates/nhs-gui/src/bridge.rs");
const jobKinds = [...bridgeSource.matchAll(/JOB_KINDS: \[&str; \d+\] = \[([^\]]+)\]/g)]
  .flatMap((match) => [...match[1].matchAll(/"([a-z_]+)"/g)].map((inner) => inner[1]))
  .sort();
// Steps, from `step_name`'s arms.
const stepKeys = [...bridgeSource.matchAll(/Step::[A-Za-z]+ => "([a-z]+)",/g)]
  .map((match) => match[1])
  .sort();

const diagnosisCodes = armsOf(
  "crates/nhs-core/src/classify.rs",
  "label",
  /Self::[A-Za-z]+ => "([a-z-]+)",/g,
);
const bridge = read("crates/nhs-gui/src/bridge.rs");
const bridgeCodes = [...bridge.matchAll(/code: "([a-z_]+)"/g)].map((m) => m[1]);
// The UI can also report a state-version mismatch on its own.
bridgeCodes.push("state_version");

if (
  refusalCodes.length === 0 ||
  stepKeys.length === 0 ||
  jobKinds.length === 0 ||
  diagnosisCodes.length === 0
) {
  console.error("could not derive the code lists from the Rust sources");
  process.exit(1);
}

const required = new Set(["discovery_failed_desc"]);
for (const code of refusalCodes) required.add(`readiness_${code}`);
for (const code of new Set(bridgeCodes)) {
  if (code === "discovery_failed") {
    // Discovery failures render through the dedicated block, not the generic
    // error mapping.
    required.add("discovery_failed_title");
    required.add("discovery_failed_desc");
    continue;
  }
  required.add(`error_${code}`);
  required.add(`error_${code}_hint`);
}
for (const code of switchCodes) required.add(code);
for (const key of jobKinds) required.add(`action_${key}`);
for (const step of stepKeys) required.add(`nav_${step}`);
for (const code of diagnosisCodes) {
  const key = `diagnosis_${code.replaceAll("-", "_")}`;
  required.add(key);
  required.add(`${key}_hint`);
}
for (let index = 0; index < 7; index += 1) required.add(`step_${stepKeys[index] ?? ""}`);

// --- checks ---------------------------------------------------------------

const base = new Set(Object.keys(I18N.en));
let failed = false;

for (const language of languages) {
  const keys = new Set(Object.keys(I18N[language]));
  const missing = [...base].filter((key) => !keys.has(key));
  const extra = [...keys].filter((key) => !base.has(key));
  if (missing.length || extra.length) {
    failed = true;
    console.error(
      `${language}: missing ${missing.join(", ") || "-"}; extra ${extra.join(", ") || "-"}`,
    );
  }
}

for (const language of languages) {
  const absent = [...required].filter((key) => !(key in I18N[language]));
  if (absent.length) {
    failed = true;
    console.error(`${language}: missing dynamic keys ${absent.join(", ")}`);
  }
}

if (failed) process.exit(1);
console.log(
  `i18n ok: ${languages.length} languages, ${base.size} keys each; ` +
    `derived ${refusalCodes.length} readiness, ${new Set(bridgeCodes).size} error, ` +
    `${jobKinds.length} action, ${stepKeys.length} step, ` +
    `${diagnosisCodes.length} diagnosis, ${switchCodes.length} switch code(s)`,
);
