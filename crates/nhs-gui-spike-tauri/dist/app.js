// The whole UI: send an event, render the state that comes back.
// Every visible string goes through `t()` from i18n.js.

const { invoke } = window.__TAURI__.core;

const STEPS = [
  ["discover", "nav_discover"],
  ["catalog", "nav_catalog"],
  ["plan", "nav_plan"],
  ["validation", "nav_validate"],
  ["confirm", "nav_confirm"],
  ["log", "nav_log"],
  ["history", "nav_history"],
];

let last = null;

function el(tag, props = {}, children = []) {
  const node = document.createElement(tag);
  Object.assign(node, props);
  for (const child of children) node.append(child);
  return node;
}

// A stable key per focusable control. The DOM nodes are thrown away on every
// render, so the key is what survives; ids would do the same but not every
// control has one, and keys must be unique across screens.
function focusKey(node, key) {
  node.dataset.focus = key;
  return node;
}

function isTextControl(node) {
  return node instanceof HTMLInputElement && typeof node.selectionStart === "number";
}

function captureFocus() {
  const active = document.activeElement;
  if (!active || active === document.body || !active.dataset) return null;
  const key = active.dataset.focus || null;
  if (!key) return null;
  // Caret position matters: restoring focus to a search box must not put the
  // cursor back at the end after the user typed in the middle.
  const start = isTextControl(active) ? active.selectionStart : null;
  const end = isTextControl(active) ? active.selectionEnd : null;
  return { key, start, end };
}

// The heading of the current screen. Focusing it (tabindex -1) is the standard
// "new page" announcement: a reader says the heading, and the next Tab starts
// at the first control of that screen instead of the top of the document.
function screenTitle() {
  const heading = document.getElementById("screen-title");
  if (!heading) return;
  heading.focus({ preventScroll: false });
}

function enabledFocusables() {
  return [...document.querySelectorAll("[data-focus]")].filter(
    (node) => !node.disabled && node.offsetParent !== null,
  );
}

// When the previously focused control is gone or disabled, move to the nearest
// enabled control in document order rather than forcing focus onto something
// that cannot take it.
function neighbourFocus(key) {
  const all = [...document.querySelectorAll("[data-focus]")];
  const index = all.findIndex((node) => node.dataset.focus === key);
  if (index < 0) return false;
  for (let offset = 1; offset < all.length; offset += 1) {
    const after = all[index + offset];
    if (after && !after.disabled && after.offsetParent !== null) {
      after.focus();
      return true;
    }
    const before = all[index - offset];
    if (before && !before.disabled && before.offsetParent !== null) {
      before.focus();
      return true;
    }
  }
  return false;
}

function restoreFocus(previous, previousStep, state) {
  if (!previous) return;
  const target = document.querySelector(`[data-focus="${CSS.escape(previous.key)}"]`);

  if (target && !target.disabled) {
    target.focus();
    if (isTextControl(target) && previous.start !== null) {
      const end = previous.end === null ? previous.start : previous.end;
      target.setSelectionRange(previous.start, end);
    }
    return;
  }

  // A screen change: the control cannot exist on the new screen. Announce the
  // new screen, do not hunt for an equivalent button.
  if (previousStep !== state.step) {
    screenTitle();
    return;
  }

  if (target && target.disabled) {
    if (!neighbourFocus(previous.key)) screenTitle();
    return;
  }

  // Same screen, control gone (for example a result that no longer matches):
  // stay on the screen, do not jump back to the top of the document.
  screenTitle();
}

async function send(event) {
  const state = await invoke("handle_event", { event });
  if (state.error) {
    document.getElementById("status").textContent = state.error;
    return;
  }
  render(state);
}

function nav(state) {
  const nav = document.getElementById("steps");
  nav.replaceChildren();
  for (const [id, key] of STEPS) {
    const button = focusKey(el("button", { type: "button", textContent: t(key) }), `nav:${id}`);
    if (state.step === id) button.setAttribute("aria-current", "step");
    // Confirm is not a free step: it opens only through the review gate.
    if (id === "confirm") {
      button.disabled = !(state.validation && state.validation.success && state.reviewed);
    }
    button.addEventListener("click", () => {
      if (id === "confirm") send({ kind: "confirm" });
      else send({ kind: "step", step: id });
    });
    nav.append(button);
  }
}

function render(state) {
  const previous = captureFocus();
  const previousStep = last ? last.step : null;
  last = state;
  document.getElementById("app-title").textContent = t("app_title");
  document.getElementById("prototype-tag").textContent = t("prototype");
  const lang = document.getElementById("lang");
  lang.textContent = t("language");
  focusKey(lang, "lang");
  lang.onclick = async () => {
    toggleLanguage();
    render(last);
  };
  document.getElementById("switch-notice").textContent = state.switch.reason;
  document.getElementById("status").textContent = t("footer", {
    step: state.step,
    selected: state.selected.length,
    readiness: state.readiness || t("ready"),
  });

  nav(state);
  const panel = document.getElementById("panel");
  panel.replaceChildren();
  const view = {
    discover: renderDiscover,
    catalog: renderCatalog,
    plan: renderPlan,
    validation: renderValidation,
    confirm: renderConfirm,
    log: renderLog,
    history: renderHistory,
  }[state.step] || renderDiscover;
  view(state, panel);

  // The content is rebuilt from scratch, so whatever had focus is gone. Put it
  // back on the control with the same key, or on the new screen's heading when
  // the screen changed. Never focus anything if nothing was focused before.
  const heading = panel.querySelector("h2");
  if (heading) {
    heading.id = "screen-title";
    heading.tabIndex = -1;
    // Its own key, so an update that arrives while the heading has focus puts
    // focus back on the heading instead of dropping it on <body>.
    focusKey(heading, "screen-title");
  }
  restoreFocus(previous, previousStep, state);
}

function renderDiscover(state, panel) {
  panel.append(el("h2", { textContent: t("discover_heading") }));
  if (!state.discovery) {
    panel.append(el("p", { textContent: t("nothing_detected") }));
    panel.append(el("div", { className: "actions" }, [
      focusKey(el("button", { className: "action", textContent: t("detect"), onclick: () => send({ kind: "discover" }) }), "detect"),
    ]));
    return;
  }
  const d = state.discovery;
  panel.append(el("table", {}, [
    el("tr", {}, [el("th", { textContent: t("layout") }), el("td", { textContent: d.kind })]),
    el("tr", {}, [el("th", { textContent: t("editable") }), el("td", { textContent: d.editable ? t("yes") : t("no_read_only") })]),
    el("tr", {}, [el("th", { textContent: t("root") }), el("td", { textContent: d.root || "—" })]),
    el("tr", {}, [el("th", { textContent: t("main_file") }), el("td", { textContent: d.main_file || "—" })]),
    el("tr", {}, [el("th", { textContent: t("generations") }), el("td", { textContent: d.generations })]),
  ]));
  panel.append(el("p", { className: "muted", textContent: state.switch.reason }));
  panel.append(el("div", { className: "actions" }, [
    focusKey(el("button", { className: "ghost", textContent: t("detect_again"), onclick: () => send({ kind: "discover" }) }), "detect-again"),
    focusKey(el("button", { className: "action", textContent: t("choose_packages"), onclick: () => send({ kind: "step", step: "catalog" }) }), "choose"),
  ]));
}

function renderCatalog(state, panel) {
  panel.append(el("h2", { textContent: t("catalog_heading") }));
  const input = focusKey(el("input", { type: "search", placeholder: t("search_placeholder"), value: state.catalog.query }), "query");
  const search = () => send({ kind: "search", query: input.value });
  input.addEventListener("keydown", (event) => { if (event.key === "Enter") search(); });
  panel.append(el("div", { className: "row" }, [
    input,
    focusKey(el("button", { className: "action", textContent: t("search"), onclick: search }), "search"),
  ]));

  if (!state.catalog.searched) {
    panel.append(el("p", { className: "muted", textContent: t("offline_note") }));
  } else if (state.catalog.hits.length === 0) {
    panel.append(el("p", { textContent: t("no_matches") }));
  }

  const list = el("ul", { className: "hits" });
  for (const hit of state.catalog.hits) {
    const box = focusKey(el("input", { type: "checkbox", checked: hit.selected, id: `pkg-${hit.attr}` }), `pkg:${hit.attr}`);
    box.addEventListener("change", () => send({ kind: "toggle", attr: hit.attr }));
    list.append(el("li", {}, [
      box,
      el("div", {}, [
        el("label", { htmlFor: `pkg-${hit.attr}`, textContent: `${hit.attr} — ${hit.description}` }),
        el("div", {
          className: "meta",
          textContent: `${hit.category || t("uncategorised")} · ${t("licence")} ${hit.license || t("licence_unknown")}`,
        }),
      ]),
    ]));
  }
  panel.append(list);

  const plan = focusKey(el("button", { className: "action", textContent: t("build_plan_for", { n: state.selected.length }) }), "plan");
  plan.disabled = state.selected.length === 0;
  plan.addEventListener("click", () => send({ kind: "plan" }));
  panel.append(el("div", { className: "actions" }, [plan]));
}

function renderPlan(state, panel) {
  panel.append(el("h2", { textContent: t("plan_heading") }));
  if (!state.plan) {
    panel.append(el("p", { className: "muted", textContent: t("no_plan") }));
    return;
  }
  if (state.plan.state === "pending") {
    panel.append(el("p", { textContent: t("plan_pending") }));
    return;
  }
  for (const file of state.plan.files) {
    if (file.noop) continue;
    panel.append(el("h3", { textContent: `${file.path} (${file.kind}, +${file.added} −${file.removed})` }));
    panel.append(el("pre", { className: "diff", textContent: file.diff }));
  }
  for (const note of state.plan.notes) {
    panel.append(el("p", { className: "muted", textContent: `${t("note")}: ${note}` }));
  }

  const reviewed = focusKey(el("input", { type: "checkbox", checked: state.reviewed, id: "reviewed" }), "reviewed");
  reviewed.addEventListener("change", () => send({ kind: "reviewed" }));
  const validate = focusKey(el("button", { className: "action", textContent: t("validate_scratch") }), "validate");
  validate.disabled = !state.reviewed;
  validate.addEventListener("click", () => send({ kind: "validate" }));
  panel.append(el("div", { className: "actions" }, [
    el("div", { className: "row" }, [reviewed, el("label", { htmlFor: "reviewed", textContent: t("reviewed") })]),
    validate,
  ]));
}

function renderValidation(state, panel) {
  panel.append(el("h2", { textContent: t("validation_heading") }));
  if (!state.validation) {
    panel.append(el("p", { className: "muted", textContent: t("not_validated") }));
    return;
  }
  const v = state.validation;
  panel.append(el("p", { textContent: `${v.command} — ${v.millis} ms` }));
  if (v.success) {
    panel.append(el("div", { className: "ok", textContent: t("build_succeeds") }));
  } else {
    panel.append(el("div", { className: "diag" }, [
      el("div", { textContent: `${v.kind}: ${v.summary}` }),
      el("div", { className: "muted", textContent: v.location || "" }),
      el("div", { textContent: `${t("what_to_do")}: ${v.hint}` }),
      el("div", { className: "muted", textContent: v.scratch ? t("copy_kept", { path: v.scratch }) : "" }),
    ]));
  }
  const next = focusKey(el("button", { className: "action", textContent: t("review_and_apply") }), "review-apply");
  next.disabled = !v.success || !state.reviewed;
  next.addEventListener("click", () => send({ kind: "confirm" }));
  panel.append(el("div", { className: "actions" }, [next]));
}

function renderConfirm(state, panel) {
  panel.append(el("h2", { textContent: t("confirm_heading") }));
  panel.append(el("div", { className: "diag", textContent: t("confirm_warning") }));
  if (state.plan) {
    const list = el("ul");
    for (const file of state.plan.files) {
      if (file.noop) continue;
      list.append(el("li", { textContent: `${file.path} (${file.kind})` }));
    }
    panel.append(list);
  }
  const apply = focusKey(el("button", { className: "action", textContent: t("apply_build_only") }), "apply");
  apply.addEventListener("click", () => send({ kind: "apply" }));
  panel.append(el("div", { className: "actions" }, [
    apply,
    focusKey(el("button", { className: "ghost", textContent: t("cancel"), onclick: () => send({ kind: "cancel" }) }), "cancel"),
  ]));
}

function renderLog(state, panel) {
  panel.append(el("h2", { textContent: t("log_heading") }));
  panel.append(el("p", { textContent: state.log.status || "—" }));
  const pre = el("pre", { className: "log" });
  pre.textContent = state.log.lines.map((line) => `[${line.stream}] ${line.text}`).join("\n") || t("empty");
  panel.append(pre);
  if (state.apply.state === "applied") {
    panel.append(el("div", {
      className: "ok",
      textContent: t("applied", { snapshot: state.apply.snapshot, n: state.apply.written.length }),
    }));
  }
  if (state.apply.state === "failed") {
    panel.append(el("div", { className: "diag", textContent: state.apply.message }));
  }
  panel.append(el("div", { className: "actions" }, [
    focusKey(el("button", { className: "ghost", textContent: t("history_button"), onclick: () => send({ kind: "history" }) }), "history"),
  ]));
}

function renderHistory(state, panel) {
  panel.append(el("h2", { textContent: t("history_heading") }));
  const history = state.history;
  if (!history) {
    panel.append(el("p", { className: "muted", textContent: t("loading") }));
    return;
  }
  panel.append(el("p", { className: "muted", textContent: history.reason }));

  panel.append(el("h3", { textContent: t("generations_heading") }));
  if (history.generations.length === 0) {
    panel.append(el("p", { className: "muted", textContent: t("no_generations") }));
  } else {
    const table = el("table", {}, [el("tr", {}, [
      el("th", { textContent: t("col_id") }),
      el("th", { textContent: t("col_link") }),
      el("th", { textContent: t("col_target") }),
    ])]);
    for (const generation of history.generations) {
      table.append(el("tr", {}, [
        el("td", { textContent: generation.id ?? "?" }),
        el("td", { textContent: generation.link }),
        el("td", { textContent: generation.target || "—" }),
      ]));
    }
    panel.append(table);
  }

  panel.append(el("h3", { textContent: t("snapshots_heading") }));
  if (history.snapshots.length === 0) {
    panel.append(el("p", { className: "muted", textContent: t("no_snapshots") }));
  } else {
    const table = el("table", {}, [el("tr", {}, [
      el("th", { textContent: t("col_id") }),
      el("th", { textContent: t("col_outcome") }),
      el("th", { textContent: t("col_files") }),
    ])]);
    for (const snapshot of history.snapshots) {
      table.append(el("tr", {}, [
        el("td", { textContent: snapshot.id }),
        el("td", { textContent: snapshot.outcome }),
        el("td", { textContent: snapshot.files }),
      ]));
    }
    panel.append(table);
  }
}

window.addEventListener("DOMContentLoaded", async () => {
  render(await invoke("state"));
  send({ kind: "discover" });
});
