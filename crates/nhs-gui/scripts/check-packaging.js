#!/usr/bin/env node
// Packaging checks that do not need Nix: one version source for the release
// packages, no hard-coded Nix version, and an honest platform declaration.

const fs = require("fs");
const path = require("path");

const repo = path.join(__dirname, "..", "..", "..");
const read = (relative) => fs.readFileSync(path.join(repo, relative), "utf8");

function fail(message) {
  console.error(`packaging: ${message}`);
  process.exit(1);
}

function section(text, name) {
  const marker = `[${name}]`;
  const start = text.indexOf(marker);
  if (start < 0) {
    fail(`missing ${marker}`);
  }
  const rest = text.slice(start + marker.length);
  const end = rest.search(/^\[/m);
  return end < 0 ? rest : rest.slice(0, end);
}

function version(text, name) {
  const match = /^\s*version\s*=\s*"([^"]+)"/m.exec(section(text, name));
  if (!match) {
    fail(`${name} has no version`);
  }
  return match[1];
}

const rootVersion = version(read("Cargo.toml"), "workspace.package");
const guiVersion = version(read("crates/nhs-gui/Cargo.toml"), "package");
if (rootVersion !== guiVersion) {
  fail(`root version ${rootVersion} != GUI version ${guiVersion}`);
}

const tauri = JSON.parse(read("crates/nhs-gui/tauri.conf.json"));
if (Object.prototype.hasOwnProperty.call(tauri, "version") && tauri.version !== guiVersion) {
  fail(`tauri.conf.json version ${tauri.version} != Cargo version ${guiVersion}`);
}

const cliNix = read("nix/cli.nix");
const guiNix = read("nix/gui.nix");

if (!cliNix.includes("lib.importTOML ../Cargo.toml")) {
  fail("nix/cli.nix must derive its version from the root Cargo.toml");
}
if (!guiNix.includes("lib.importTOML ../crates/nhs-gui/Cargo.toml")) {
  fail("nix/gui.nix must derive its version from the GUI Cargo.toml");
}
for (const [name, text] of [
  ["nix/cli.nix", cliNix],
  ["nix/gui.nix", guiNix],
]) {
  if (/^\s*version\s*=\s*"/m.test(text)) {
    fail(`${name} still hard-codes a version`);
  }
  if (!/platforms\s*=\s*\[\s*"x86_64-linux"\s*\]/.test(text)) {
    fail(`${name} must declare only x86_64-linux`);
  }
}

if (!/"x86_64-linux"/.test(read("flake.nix"))) {
  fail("flake.nix does not expose x86_64-linux");
}

console.log(`packaging ok: version ${rootVersion}; platforms x86_64-linux`);
