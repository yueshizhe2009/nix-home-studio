// A DOM stub large enough to run the real dist/app.js under Node.
//
// The point is to test the actual renderer, not a copy of it: the sandbox loads
// dist/i18n.js and dist/app.js unmodified and exposes the functions under test.

const fs = require("fs");
const path = require("path");
const vm = require("vm");

class Node {
  constructor(tag) {
    this.tagName = tag.toUpperCase();
    this.children = [];
    this.attributes = {};
    this.dataset = {};
    this.style = {};
    this._text = "";
    this.className = "";
    this.checked = false;
    this.disabled = false;
    this.value = "";
    this.id = "";
  }

  set textContent(value) {
    this._text = String(value);
    this.children = [];
  }

  get textContent() {
    if (this.children.length === 0) return this._text;
    return this.children.map((child) => child.textContent).join("");
  }

  append(...nodes) {
    for (const node of nodes) {
      if (node !== null && node !== undefined) this.children.push(node);
    }
  }

  appendChild(node) {
    this.append(node);
    return node;
  }

  replaceChildren(...nodes) {
    this.children = [];
    this.append(...nodes);
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
  }

  getAttribute(name) {
    return this.attributes[name];
  }

  addEventListener() {}

  focus() {
    sandbox.document.activeElement = this;
  }

  querySelector() {
    return null;
  }

  querySelectorAll() {
    return [];
  }
}

function makeDocument() {
  const byId = new Map();
  for (const id of [
    "app-title",
    "app-version",
    "lang",
    "steps",
    "panel-content",
    "actions",
    "status",
    "screen-title",
  ]) {
    const node = new Node("div");
    node.id = id;
    byId.set(id, node);
  }
  return {
    documentElement: {},
    body: new Node("body"),
    activeElement: null,
    createElement: (tag) => new Node(tag),
    getElementById: (id) => byId.get(id) ?? null,
    querySelector: () => null,
    querySelectorAll: () => [],
  };
}

const sandbox = {
  document: makeDocument(),
  localStorage: { getItem: () => null, setItem: () => {} },
  CSS: { escape: (value) => value },
  requestAnimationFrame: (callback) => callback(),
  window: {
    __TAURI__: {
      core: { invoke: async () => ({}) },
      event: { listen: async () => () => {} },
    },
  },
  console,
};
sandbox.window.addEventListener = () => {};

function load(distDir) {
  vm.createContext(sandbox);
  for (const file of ["i18n.js", "app.js"]) {
    const source = fs.readFileSync(path.join(distDir, file), "utf8");
    vm.runInContext(source, sandbox, { filename: file });
  }
  vm.runInContext(
    "globalThis.__renderPlan = renderPlan;" +
      "globalThis.__renderValidation = renderValidation;" +
      "globalThis.__renderConfirm = renderConfirm;" +
      "globalThis.__renderLog = renderLog;" +
      "globalThis.__renderHistory = renderHistory;" +
      "globalThis.__setLang = applyLanguage;" +
      "globalThis.__t = t;",
    sandbox,
  );
  return sandbox;
}

function findAll(node, predicate, out = []) {
  if (predicate(node)) out.push(node);
  for (const child of node.children) findAll(child, predicate, out);
  return out;
}

function find(node, predicate) {
  return findAll(node, predicate)[0] ?? null;
}

module.exports = { load, Node, findAll, find, distDir: path.join(__dirname, "..", "dist") };
