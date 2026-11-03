// The whole UI: ask Rust for state, render it, send events back.
//
// Contract: `state` is read-only, `dispatch` is the only way anything changes, and
// the front end never decides *whether* an action is allowed — it only reflects the
// capabilities and refusals the backend reports. Focus is restored across renders
// by stable `data-focus` keys, the same pattern verified in the M6.2 prototype.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const STATE_VERSION = 3;

// What this build of the window can draw. The backend advertises every capability
// it implements (catalog, plan, validate, apply); a screen is only reachable once
// its rendering exists, so the two facts are kept apart on purpose.
const RENDERED = new Set(["discover", "catalog", "plan", "validation", "confirm", "log", "history"]);

const STEPS = [
  { id: "discover", key: "nav_discover", capability: null },
  { id: "catalog", key: "nav_catalog", capability: "catalog" },
  { id: "plan", key: "nav_plan", capability: "plan" },
  { id: "validation", key: "nav_validation", capability: "validate" },
  { id: "confirm", key: "nav_confirm", capability: "apply" },
  { id: "log", key: "nav_log", capability: "apply" },
  { id: "history", key: "nav_history", capability: "catalog" },
];

let last = null;
let busy = false;

function el(tag, props = {}, children = []) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    // `aria-*` are attributes, not DOM properties: assigning the property does
    // nothing, so a label written that way never reaches the accessibility tree.
    if (key.startsWith("aria-") || key === "scope") node.setAttribute(key, value);
    else node[key] = value;
  }
  for (const child of children) node.append(child);
  return node;
}

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
  return {
    key,
    start: isTextControl(active) ? active.selectionStart : null,
    end: isTextControl(active) ? active.selectionEnd : null,
  };
}

function screenTitle() {
  const heading = document.getElementById("screen-title");
  if (heading) heading.focus();
}

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
  if (previousStep !== state.step) {
    screenTitle();
    return;
  }
  const target = document.querySelector(`[data-focus="${CSS.escape(previous.key)}"]`);
  if (target && !target.disabled) {
    target.focus();
    if (isTextControl(target) && previous.start !== null) {
      const end = previous.end === null ? previous.start : previous.end;
      target.setSelectionRange(previous.start, end);
    }
    return;
  }
  if (previousStep !== state.step || !target) {
    screenTitle();
    return;
  }
  if (!neighbourFocus(previous.key)) screenTitle();
}

function showFatal(error) {
  const panel = document.getElementById("panel-content");
  if (!panel) return;
  panel.replaceChildren(renderError(error));
}

// Translate by stable code; fall back to the backend's English sentence only when
// this build has no translation for that code.
function translated(key, fallback) {
  const text = t(key);
  return text === key ? fallback : text;
}

function errorCopy(error) {
  const code = error && error.code ? error.code : "unknown";
  return {
    code,
    message: translated(`error_${code}`, (error && error.message) || String(error)),
    hint: translated(`error_${code}_hint`, (error && error.hint) || ""),
  };
}

// `not_available` carries a stable detail key: an action name, or `step:<id>`.
function detailText(detail) {
  if (!detail) return "";
  if (detail.startsWith("step:")) {
    const step = detail.slice("step:".length);
    return translated(`nav_${step}`, step);
  }
  return translated(`action_${detail}`, detail);
}

function renderError(error) {
  const copy = errorCopy(error);
  // A refusal carries a stable state-machine code; its explanation is the same
  // readiness text the UI already shows next to the disabled control.
  const detail =
    error && error.code === "refused" && error.detail
      ? translated(`readiness_${error.detail}`, error.detail)
      : detailText(error && error.detail);
  const box = el("div", { className: "notice error" }, [
    el("h3", { textContent: t("error_title") }),
    el("p", { textContent: detail ? `${copy.message} (${detail})` : copy.message }),
  ]);
  // A refused action must be announced, not just drawn above the fold.
  box.setAttribute("role", "alert");
  if (copy.hint) {
    box.append(el("p", { className: "muted", textContent: `${t("error_hint")}: ${copy.hint}` }));
  }
  box.append(
    el("p", {
      className: "muted",
      textContent: `${t("error_code")}: ${copy.code}`,
    }),
  );
  // Raw text from the operating system, if any, is shown as-is.
  const raw = error && error.code === "internal" ? error.detail : null;
  if (raw) box.append(el("p", { className: "path", textContent: raw }));
  return box;
}

function nav(state) {
  const nav = document.getElementById("steps");
  nav.replaceChildren();
  nav.setAttribute("aria-label", t("nav_label"));
  for (const step of STEPS) {
    const implemented =
      step.capability === null || Boolean(state.capabilities[step.capability]);
    // Confirmation is gated by the backend's readiness, not by a local rule:
    // `readiness === null` means the ticket is available right now.
    const gated = step.id === "confirm" ? state.readiness !== null : false;
    const rendered = RENDERED.has(step.id) && !gated;
    const button = focusKey(
      el("button", {
        type: "button",
        textContent: t(step.key),
        disabled: !implemented || !rendered,
      }),
      `nav:${step.id}`,
    );
    if (!implemented) button.title = t("nav_locked");
    else if (!rendered) button.title = t("nav_not_rendered");
    if (state.step === step.id) button.setAttribute("aria-current", "step");
    button.addEventListener("click", () => {
      // History is a report, not just a screen: entering it must load the two
      // lists, otherwise a restart shows "Loading…" forever.
      if (step.id === "history") dispatch({ kind: "history" });
      else dispatch({ kind: "step", step: step.id });
    });
    nav.append(button);
  }
}

function render(state) {
  if (!state || state.version !== STATE_VERSION) {
    showFatal({
      code: "state_version",
      message: `The interface expects state version ${STATE_VERSION}, the application sent ${
        state && state.version
      }.`,
      hint: "Update the application and the interface together.",
    });
    return;
  }

  const previous = captureFocus();
  const previousStep = last ? last.step : null;
  const wasBusy = last && last.busy ? last.busy.kind : null;
  last = state;

  document.getElementById("app-title").textContent = t("app_title");
  document.getElementById("app-version").textContent = `v${state.app.version}`;
  const lang = document.getElementById("lang");
  lang.textContent = t("language");
  lang.setAttribute("aria-label", t("language_label"));
  focusKey(lang, "lang");
  lang.onclick = () => {
    toggleLanguage();
    render(last);
  };

  nav(state);

  const content = document.getElementById("panel-content");
  content.replaceChildren();
  if (state.busy) content.setAttribute("aria-busy", "true");
  else content.removeAttribute("aria-busy");
  renderContent(state, content);

  const heading = content.querySelector("h2");
  if (heading) {
    heading.id = "screen-title";
    heading.tabIndex = -1;
    focusKey(heading, "screen-title");
  }

  const actions = document.getElementById("actions");
  actions.replaceChildren();
  if (state.busy) {
    // While a job runs only cancellation is accepted; the backend refuses
    // anything else with `busy`. Applies cannot be interrupted, so no button is
    // offered for them rather than one that would fail.
    if (state.busy.kind !== "apply") {
      actions.append(
        focusKey(
          el("button", {
            className: "ghost",
            type: "button",
            textContent: t("cancel_running"),
            onclick: () => cancelRunning(),
          }),
          "cancel",
        ),
      );
    }
  } else if (state.step === "discover") {
    const refresh = focusKey(
      el("button", {
        className: "action",
        type: "button",
        textContent: busy ? t("retrying") : t("refresh"),
        onclick: () => dispatch({ kind: "discover" }),
      }),
      "refresh",
    );
    actions.append(refresh);
  } else if (state.step === "catalog") {
    const plan = focusKey(
      el("button", {
        className: "action",
        type: "button",
        textContent: t("build_plan", { n: state.selected }),
        disabled: state.selected === 0,
        onclick: () => dispatch({ kind: "plan" }),
      }),
      "plan",
    );
    actions.append(plan);
    if (state.selected === 0) {
      actions.append(el("span", { className: "muted", textContent: t("no_selection") }));
    }
  } else if (state.step === "confirm") {
    const apply = focusKey(
      el("button", {
        className: "action",
        type: "button",
        textContent: t("apply_build_only"),
        onclick: () => dispatch({ kind: "apply" }),
      }),
      "apply",
    );
    const cancel = focusKey(
      el("button", {
        className: "ghost",
        type: "button",
        textContent: t("cancel_apply"),
        onclick: () => dispatch({ kind: "cancel" }),
      }),
      "cancel-apply",
    );
    actions.append(apply, cancel);
  } else if (state.step === "log") {
    actions.append(
      focusKey(
        el("button", {
          className: "action",
          type: "button",
          textContent: t("history_button"),
          onclick: () => dispatch({ kind: "history" }),
        }),
        "history",
      ),
    );
  } else if (state.step === "plan") {
    const reviewed = focusKey(
      el("input", { type: "checkbox", checked: state.reviewed, id: "reviewed" }),
      "reviewed",
    );
    reviewed.addEventListener("change", () => dispatch({ kind: "reviewed" }));
    actions.append(
      el("div", { className: "row" }, [
        reviewed,
        el("label", { htmlFor: "reviewed", textContent: t("reviewed") }),
      ]),
    );
    const validate = focusKey(
      el("button", {
        className: "action",
        type: "button",
        textContent: t("validate"),
        disabled: !state.reviewed,
        onclick: () => dispatch({ kind: "validate" }),
      }),
      "validate",
    );
    actions.append(validate);
  }

  const mode = state.busy
    ? t("busy_running", { kind: translated(`action_${state.busy.kind}`, state.busy.kind) })
    : state.readiness
      ? translated(`readiness_${state.readiness.code}`, state.readiness.message)
      : t("footer_ready");
  document.getElementById("status").textContent = t("footer", {
    step: t(`step_${state.step}`) || state.step,
    selected: state.selected,
    mode,
  });

  restoreFocus(previous, previousStep, state);

  // A finished validation replaces the running text with a result. Move focus
  // to it so the result is read out; the user started the job, so this is not a
  // surprise focus grab.
  if (wasBusy === "validation" && !state.busy && state.validation) {
    const result = document.getElementById("validation-result");
    if (result) result.focus();
  }
}

function factRow(label, value) {
  const row = el("tr", {}, [
    el("th", { textContent: label, scope: "row" }),
    el("td", {}, [value]),
  ]);
  return row;
}

function renderContent(state, panel) {
  switch (state.step) {
    case "catalog":
      renderCatalog(state, panel);
      return;
    case "plan":
      renderPlan(state, panel);
      return;
    case "validation":
      renderValidation(state, panel);
      return;
    case "confirm":
      renderConfirm(state, panel);
      return;
    case "log":
      renderLog(state, panel);
      return;
    case "history":
      renderHistory(state, panel);
      return;
    default:
      renderDiscover(state, panel);
  }
}

function renderCatalog(state, panel) {
  panel.append(el("h2", { textContent: t("catalog_heading") }));

  const input = focusKey(
    el("input", {
      type: "search",
      placeholder: t("search_placeholder"),
      value: state.catalog.query,
      "aria-label": t("search_placeholder"),
    }),
    "query",
  );
  const submit = () => dispatch({ kind: "search", query: input.value });
  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter") submit();
  });
  panel.append(
    el("div", { className: "row" }, [
      input,
      focusKey(
        el("button", {
          className: "action",
          type: "button",
          textContent: t("search"),
          onclick: submit,
        }),
        "search",
      ),
    ]),
  );
  panel.append(el("p", { className: "muted", textContent: t("search_hint") }));

  if (!state.catalog.searched) return;

  if (state.catalog.hits.length === 0) {
    panel.append(el("p", { textContent: t("no_matches") }));
    return;
  }

  const table = el("table", {
    className: "hits",
    "aria-label": t("catalog_heading"),
  }, [
    el("thead", {}, [
      el("tr", {}, [
        el("th", { textContent: "", scope: "col" }),
        el("th", { textContent: t("catalog_heading"), scope: "col" }),
        el("th", { textContent: t("licence"), scope: "col" }),
      ]),
    ]),
  ]);
  const body = el("tbody");
  for (const hit of state.catalog.hits) {
    const id = `pkg-${hit.attr}`;
    const box = focusKey(
      el("input", { type: "checkbox", checked: hit.selected, id }),
      `pkg:${hit.attr}`,
    );
    box.addEventListener("change", () => dispatch({ kind: "toggle", attr: hit.attr }));
    body.append(
      el("tr", {}, [
        el("td", {}, [box]),
        el("td", {}, [
          el("label", { htmlFor: id, textContent: hit.name }),
          el("div", {
            className: "muted",
            textContent: `${hit.description} · ${hit.category || t("uncategorised")}`,
          }),
        ]),
        el("td", { textContent: hit.license || t("licence_unknown") }),
      ]),
    );
  }
  table.append(body);
  panel.append(table);
  panel.append(
    el("p", { className: "muted", textContent: t("selected_count", { n: state.selected }) }),
  );
}

const DIFF_LIMIT = 2000;

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
    panel.append(
      el("h3", {
        textContent: `${file.path} (${file.kind}, +${file.added} −${file.removed})`,
      }),
    );
    const lines = file.diff.split("\n");
    if (lines.length > DIFF_LIMIT) {
      // Collapsed by default: a single huge file must not bury the actions or
      // the rest of the plan. <details> only hides the content — it is always
      // complete, so expanding shows the whole diff, not a prefix.
      const details = el("details", {}, [
        el("summary", { textContent: t("show_diff", { n: lines.length }) }),
        el("p", { className: "muted", textContent: t("folded_note", { n: lines.length }) }),
      ]);
      details.append(el("pre", { className: "diff", textContent: file.diff }));
      panel.append(details);
    } else {
      panel.append(el("pre", { className: "diff", textContent: file.diff }));
    }
  }

  for (const note of state.plan.notes) {
    panel.append(el("p", { className: "muted", textContent: `${t("note")}: ${note}` }));
  }
}

const LOG_TAIL = 200;

function renderLogLines(state, panel) {
  panel.append(el("h3", { textContent: t("validation_log") }));
  const lines = state.log.lines;
  if (lines.length === 0) {
    panel.append(el("p", { className: "muted", textContent: t("empty_log") }));
    return;
  }
  const shown = lines.slice(-LOG_TAIL);
  if (shown.length !== lines.length) {
    panel.append(
      el("p", {
        className: "muted",
        textContent: t("validation_log_tail", {
          shown: shown.length,
          total: lines.length,
        }),
      }),
    );
  }
  panel.append(
    el("pre", {
      className: "log",
      textContent: shown.map((line) => `[${line.stream}] ${line.text}`).join("\n"),
    }),
  );
}

function renderValidation(state, panel) {
  panel.append(el("h2", { textContent: t("validation_heading") }));

  if (state.busy && state.busy.kind === "validation") {
    panel.append(el("p", { textContent: t("validation_running") }));
    renderLogLines(state, panel);
    return;
  }

  if (!state.validation) {
    panel.append(el("p", { className: "muted", textContent: t("not_validated") }));
    return;
  }

  const report = state.validation;
  // Focusable result container: the worker finishing moves focus here so the
  // outcome is announced.
  const result = el("div", { id: "validation-result", tabIndex: -1 });
  result.append(
    el("p", {
      className: "muted",
      textContent: `${t("validation_command")}: ${report.command} · ${report.millis} ms`,
    }),
  );

  if (report.success) {
    result.append(
      el("div", { className: "notice ok" }, [
        el("p", { textContent: t("validation_success") }),
      ]),
    );
    result.append(el("p", { className: "muted", textContent: t("validation_next") }));
    panel.append(result);
    renderLogLines(state, panel);
    return;
  }

  // Diagnosis codes are stable kebab-case strings from core; translate by code
  // and keep the engine's summary as raw detail.
  const code = report.kind ? report.kind.code.replaceAll("-", "_") : "unknown";
  const label = translated(`diagnosis_${code}`, report.kind ? report.kind.label : "");
  const hint = translated(`diagnosis_${code}_hint`, report.hint || "");

  const block = el("div", { className: "notice error" }, [
    el("h3", { textContent: t("validation_failed_title") }),
    el("p", { textContent: label }),
  ]);
  if (report.summary) {
    block.append(el("p", { className: "path", textContent: report.summary }));
  }
  if (hint) {
    block.append(el("p", { textContent: `${t("validation_hint")}: ${hint}` }));
  }
  if (report.location) {
    block.append(
      el("p", { className: "muted", textContent: `${t("validation_location")}: ${report.location}` }),
    );
  }
  if (report.scratch) {
    block.append(
      el("p", { className: "muted", textContent: `${t("validation_scratch")}: ${report.scratch}` }),
    );
  }
  result.append(block);
  panel.append(result);
  renderLog(state, panel);
}

function renderConfirm(state, panel) {
  panel.append(el("h2", { textContent: t("confirm_heading") }));
  panel.append(el("div", { className: "notice warn" }, [
    el("p", { textContent: t("confirm_warning") }),
  ]));

  panel.append(el("h3", { textContent: t("files_changed") }));
  if (state.plan) {
    const list = el("ul", { className: "paths" });
    for (const file of state.plan.files) {
      if (file.noop) continue;
      list.append(el("li", { className: "path", textContent: `${file.path} (${file.kind})` }));
    }
    panel.append(list);
  }
}

function renderLog(state, panel) {
  panel.append(el("h2", { textContent: t("log_heading") }));
  if (state.log.status) {
    panel.append(el("p", { textContent: state.log.status }));
  }
  if (state.apply.state === "applied") {
    panel.append(
      el("div", { className: "notice ok" }, [
        el("p", {
          textContent: t("applied_snapshot", {
            snapshot: state.apply.snapshot,
            n: state.apply.written.length,
          }),
        }),
      ]),
    );
  } else if (state.apply.state === "failed") {
    panel.append(
      el("div", { className: "notice error" }, [
        el("h3", { textContent: t("apply_failed") }),
        el("p", { className: "path", textContent: state.apply.message || "" }),
      ]),
    );
  }
  renderLogLines(state, panel);
}

function renderHistory(state, panel) {
  panel.append(el("h2", { textContent: t("history_heading") }));
  if (!state.history) {
    panel.append(el("p", { className: "muted", textContent: t("loading") }));
    return;
  }
  const history = state.history;
  // The backend has no rollback: say so instead of showing a control that
  // cannot work.
  panel.append(
    el("div", { className: "notice" }, [
      el("p", {
        textContent: translated(history.reason_code, history.reason),
      }),
    ]),
  );

  panel.append(el("h3", { textContent: t("history_generations") }));
  if (history.generations.length === 0) {
    panel.append(el("p", { className: "muted", textContent: t("history_no_generations") }));
  } else {
    const table = el("table", {
      className: "hits",
      "aria-label": t("history_generations"),
    }, [
      el("thead", {}, [
        el("tr", {}, [
          el("th", { textContent: t("history_col_id"), scope: "col" }),
          el("th", { textContent: t("history_col_link"), scope: "col" }),
          el("th", { textContent: t("history_col_target"), scope: "col" }),
        ]),
      ]),
    ]);
    const body = el("tbody");
    for (const generation of history.generations) {
      body.append(
        el("tr", {}, [
          el("td", { textContent: generation.id ?? "—" }),
          el("td", { className: "path nowrap", textContent: generation.link }),
          el("td", { className: "path nowrap", textContent: generation.target || "—" }),
        ]),
      );
    }
    table.append(body);
    panel.append(table);
  }

  panel.append(el("h3", { textContent: t("history_snapshots") }));
  if (history.snapshots.length === 0) {
    panel.append(el("p", { className: "muted", textContent: t("history_no_snapshots") }));
  } else {
    const table = el("table", {
      className: "hits",
      "aria-label": t("history_snapshots"),
    }, [
      el("thead", {}, [
        el("tr", {}, [
          el("th", { textContent: t("history_col_id"), scope: "col" }),
          el("th", { textContent: t("history_col_outcome"), scope: "col" }),
          el("th", { textContent: t("history_col_files"), scope: "col" }),
        ]),
      ]),
    ]);
    const body = el("tbody");
    for (const snapshot of history.snapshots) {
      body.append(
        el("tr", {}, [
          el("td", { className: "path nowrap", textContent: snapshot.id }),
          el("td", { textContent: snapshot.outcome }),
          el("td", { textContent: String(snapshot.files) }),
        ]),
      );
    }
    table.append(body);
    panel.append(table);
  }
}

function renderDiscover(state, panel) {
  panel.append(el("h2", { textContent: t("discover_heading") }));
  panel.append(el("p", { className: "lede", textContent: t("discover_lede") }));

  if (state.discovery_error) {
    const failure = state.discovery_error;
    panel.append(
      el("div", { className: "notice error" }, [
        el("h3", { textContent: t("discovery_failed_title") }),
        el("p", { textContent: t("discovery_failed_desc") }),
        el("p", { className: "path", textContent: failure.detail }),
        el("p", { className: "muted", textContent: `${t("error_code")}: ${failure.code}` }),
      ]),
    );
    const retry = focusKey(
      el("button", {
        className: "ghost",
        type: "button",
        textContent: t("retry"),
        onclick: () => dispatch({ kind: "discover" }),
      }),
      "retry",
    );
    panel.append(el("div", { className: "panel-actions-inline" }, [retry]));
    return;
  }

  if (!state.discovery) {
    panel.append(el("p", { className: "muted", textContent: t("loading") }));
    return;
  }

  const found = state.discovery;
  const kind = t(`kind_${found.kind.replaceAll("-", "_")}`);
  if (!found.editable) {
    panel.append(
      el("div", { className: "notice warn" }, [
        el("h3", { textContent: t("read_only_title") }),
        el("p", { textContent: t(`notice_${found.kind.replaceAll("-", "_")}`) }),
      ]),
    );
  }

  const table = el("table", { className: "facts" }, [
    el("tbody", {}, [
      factRow(t("layout"), el("span", { textContent: kind })),
      factRow(
        t("editable"),
        el("span", { textContent: found.editable ? t("editable_yes") : t("editable_no") }),
      ),
      factRow(
        t("root"),
        el("span", {
          className: "path",
          textContent: found.root || t("none"),
        }),
      ),
      factRow(
        t("main_file"),
        el("span", {
          className: "path",
          textContent: found.main_file || t("none"),
        }),
      ),
      factRow(t("generations"), el("span", { textContent: String(found.generations) })),
    ]),
  ]);
  panel.append(table);

  panel.append(
    el("div", { className: "notice" }, [
      el("h3", { textContent: t("activation_title") }),
      el("p", {
        textContent: translated(state.switch.reason_code, state.switch.reason),
      }),
    ]),
  );
}

async function dispatch(event) {
  if (busy) return;
  busy = true;
  document.body.classList.add("busy");
  // Let the busy state paint before the IPC round trip. The refresh control stays
  // enabled for the focus-restore step below; the `busy` guard prevents a second
  // request, so it cannot be double-fired.
  await new Promise((resolve) => requestAnimationFrame(resolve));
  try {
    const state = await invoke("dispatch", { event });
    // Clear `busy` before rendering: a control that is disabled during the render
    // cannot take focus back, which would send focus to the heading instead.
    busy = false;
    document.body.classList.remove("busy");
    render(state);
  } catch (error) {
    busy = false;
    document.body.classList.remove("busy");
    showFatal(error);
  }
}

async function cancelRunning() {
  try {
    await invoke("cancel");
    // The worker emits the updated state; nothing else to do here.
  } catch (error) {
    showFatal(error);
  }
}

// The worker emits every state change; the window never polls.
function subscribe() {
  listen("nhs:state", (event) => {
    if (event && event.payload) render(event.payload);
  }).catch((error) => showFatal(error));
}

async function boot() {
  applyLanguage(initialLanguage());
  subscribe();
  try {
    const state = await invoke("state");
    render(state);
    if (!state.discovery && !state.discovery_error) {
      await dispatch({ kind: "discover" });
    }
  } catch (error) {
    showFatal(error);
  }
}

window.addEventListener("DOMContentLoaded", boot);
