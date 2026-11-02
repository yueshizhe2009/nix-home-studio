//! Tauri prototype for M6.2 — five screens over `nhs-app` + `nhs-adapter`.
//!
//! The Rust side owns the state machine; the web UI only sends events and renders
//! the state it gets back. That keeps every rule in `nhs-app` (already tested) and
//! makes the toolkit comparison about rendering and interaction, which is what
//! M6.2 is supposed to measure.
//!
//! This is a spike, not the product: the validation step blocks the IPC call while
//! `home-manager build` runs, and there is no settings editor.

use std::sync::Mutex;

use nhs_adapter::Session;
use nhs_app::{
    App, ApplyState, Event, Intent, LogStream, PlanState, RefusalCode, ReviewState, Step,
};
use serde_json::{json, Value};
use tauri::State;

struct Shared {
    app: Mutex<App>,
    session: Session,
}

/// Payloads the web UI may send.
fn event_from_json(value: &Value) -> Result<Event, String> {
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("event without `kind`")?;
    match kind {
        "discover" => Ok(Event::DiscoverRequested),
        "search" => Ok(Event::SearchSubmitted {
            query: value
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            filters: nhs_core::catalog::Filters::default(),
            page: 1,
            per_page: 20,
        }),
        "toggle" => Ok(Event::PackageToggled(
            value
                .get("attr")
                .and_then(Value::as_str)
                .ok_or("toggle without `attr`")?
                .to_string(),
        )),
        "plan" => Ok(Event::PlanRequested),
        "validate" => Ok(Event::ValidationRequested),
        "reviewed" => Ok(Event::DiffReviewed),
        "confirm" => Ok(Event::ConfirmationRequested),
        "apply" => Ok(Event::ApplyConfirmed),
        "cancel" => Ok(Event::ConfirmationCancelled),
        "history" => Ok(Event::HistoryRequested),
        "step" => {
            let step = match value.get("step").and_then(Value::as_str) {
                Some("discover") => Step::Discover,
                Some("catalog") => Step::Catalog,
                Some("plan") => Step::Plan,
                Some("validation") => Step::Validation,
                Some("confirm") => Step::Confirm,
                Some("log") => Step::BuildLog,
                Some("history") => Step::History,
                other => return Err(format!("unknown step {other:?}")),
            };
            Ok(Event::StepRequested(step))
        }
        other => Err(format!("unknown event kind `{other}`")),
    }
}

#[tauri::command]
fn state(shared: State<'_, Shared>) -> Value {
    let app = shared.app.lock().expect("app lock");
    state_json(&app)
}

#[tauri::command]
fn handle_event(shared: State<'_, Shared>, event: Value) -> Value {
    match event_from_json(&event) {
        Ok(event) => feed(&shared, event),
        Err(message) => json!({ "error": message }),
    }
}

/// Feed one event through the state machine and run whatever it asks for.
fn feed(shared: &Shared, parsed: Event) -> Value {
    let mut app = shared.app.lock().expect("app lock");
    let mut response = app.handle(parsed);
    // Run what the state machine asked for, feeding the result back in. Depth is
    // bounded: one event can trigger at most one round of work.
    for _ in 0..6 {
        let Some(intent) = response.intents.first().cloned() else {
            break;
        };
        let mut lines: Vec<(nhs_core::proc::Stream, String)> = Vec::new();
        let result = match intent {
            Intent::Discover => Some(shared.session.discover()),
            Intent::Search {
                query,
                filters,
                page,
                per_page,
            } => Some(shared.session.search(&query, &filters, page, per_page)),
            Intent::BuildPlan { revision } => {
                let selection = nhs_core::flow::Selection {
                    packages: app.selected().to_vec(),
                    settings: app.settings().to_vec(),
                };
                Some(shared.session.plan(&selection, revision))
            }
            Intent::Validate { revision, attempt } => {
                let mut sink = |stream: nhs_core::proc::Stream, text: &str| {
                    lines.push((stream, text.to_string()));
                };
                let event = shared.session.validate(revision, attempt, &mut sink);
                for (stream, text) in lines.drain(..) {
                    let stream = match stream {
                        nhs_core::proc::Stream::Stdout => LogStream::Stdout,
                        nhs_core::proc::Stream::Stderr => LogStream::Stderr,
                    };
                    app.handle(Event::LogLine { stream, text });
                }
                Some(event)
            }
            Intent::Apply { revision } => Some(shared.session.apply(revision)),
            Intent::LoadHistory => Some(shared.session.history()),
        };
        let Some(result) = result else { break };
        response = app.handle(result);
    }
    state_json(&app)
}

/// Walk the five flows without a window, printing what each screen would show.
///
/// This is the prototype's own evidence that the UI's data path works; rendering
/// is checked separately by looking at the window.
fn run_selftest(shared: &Shared) {
    let step = |label: &str, state: Value| {
        println!(
            "{label:<24} step={:<11} selected={} plan={} validation={} apply={}",
            state["step"].as_str().unwrap_or("?"),
            state["selected"].as_array().map_or(0, Vec::len),
            state["plan"]["files"].as_array().map_or(0, Vec::len),
            state["validation"]["success"].as_bool().map_or_else(
                || "none".to_string(),
                |ok| if ok { "ok".to_string() } else { "failed".to_string() }
            ),
            state["apply"]["state"].as_str().unwrap_or("?"),
        );
    };
    step("discover", feed(shared, shared.session.discover()));
    step(
        "search firefox",
        feed(
            shared,
            Event::SearchSubmitted {
                query: "firefox".to_string(),
                filters: nhs_core::catalog::Filters::default(),
                page: 1,
                per_page: 20,
            },
        ),
    );
    step(
        "select firefox",
        feed(shared, Event::PackageToggled("firefox".to_string())),
    );
    step("build plan", feed(shared, Event::PlanRequested));
    step("review diff", feed(shared, Event::DiffReviewed));
    step("validate", feed(shared, Event::ValidationRequested));
    step("ask confirmation", feed(shared, Event::ConfirmationRequested));
    step("apply", feed(shared, Event::ApplyConfirmed));
    step("history", feed(shared, Event::HistoryRequested));
}

/// Drive the window to a screen before it is shown (used for capture).
fn preload(shared: &Shared, screen: &str) {
    let stages = [
        "catalog",
        "plan",
        "validation",
        "confirm",
        "history",
    ];
    let Some(position) = stages.iter().position(|stage| *stage == screen) else {
        return;
    };
    feed(
        shared,
        Event::SearchSubmitted {
            query: "firefox".to_string(),
            filters: nhs_core::catalog::Filters::default(),
            page: 1,
            per_page: 20,
        },
    );
    if position >= 1 {
        feed(shared, Event::PackageToggled("firefox".to_string()));
        feed(shared, Event::PlanRequested);
        feed(shared, Event::DiffReviewed);
    }
    if position >= 2 {
        feed(shared, Event::ValidationRequested);
    }
    if position >= 3 {
        feed(shared, Event::ConfirmationRequested);
    }
    if position >= 4 {
        feed(shared, Event::HistoryRequested);
    }
}

fn state_json(app: &App) -> Value {
    let readiness = match app.apply_readiness() {
        Ok(()) => Value::Null,
        Err(code) => json!(code.message()),
    };
    json!({
        "step": step_name(app.step()),
        "switch": { "available": app.switch_available(), "reason": app.switch_reason() },
        "discovery": app.discovery().map(|discovery| json!({
            "kind": discovery.layout.kind(),
            "editable": app.editable(),
            "root": app.config_root(),
            "main_file": discovery.layout.main_file(),
            "generations": discovery.layout.generations().len(),
        })),
        "catalog": {
            "query": app.catalog().query,
            "total": app.catalog().total,
            "searched": app.catalog().searched,
            "hits": app.catalog().hits.iter().map(|hit| json!({
                "attr": hit.attr,
                "name": hit.name,
                "description": hit.description,
                "category": hit.category,
                "license": hit.license,
                "selected": app.is_selected(&hit.attr),
            })).collect::<Vec<_>>(),
        },
        "selected": app.selected(),
        "plan": app.plan().map(|plan| json!({
            "state": plan_state_name(app.plan_state()),
            "notes": plan.notes,
            "files": plan.changes.iter().map(|change| json!({
                "path": change.path,
                "kind": match change.kind {
                    nhs_core::ChangeKind::Create => "create",
                    nhs_core::ChangeKind::Replace => "replace",
                },
                "added": change.added_lines,
                "removed": change.removed_lines,
                "diff": change.unified_diff,
                "noop": change.is_noop(),
            })).collect::<Vec<_>>(),
        })),
        "validation": app.validation().map(|validation| json!({
            "success": validation.report.success,
            "command": validation.report.command,
            "millis": validation.report.elapsed.as_millis(),
            "kind": validation.report.diagnosis.as_ref().map(|d| d.kind.label()),
            "summary": validation.report.diagnosis.as_ref().map(|d| d.summary.clone()),
            "hint": validation.report.diagnosis.as_ref().map(|d| d.hint),
            "location": validation.report.diagnosis.as_ref().and_then(|d| d.location.clone()),
            "scratch": validation.report.kept_scratch,
        })),
        "reviewed": matches!(app.review(), ReviewState::Acknowledged { .. }),
        "readiness": readiness,
        "apply": match app.apply() {
            ApplyState::Idle => json!({ "state": "idle" }),
            ApplyState::Running => json!({ "state": "running" }),
            ApplyState::Applied { snapshot_id, written, .. } => json!({
                "state": "applied", "snapshot": snapshot_id, "written": written
            }),
            ApplyState::Failed { message, .. } => json!({ "state": "failed", "message": message }),
        },
        "log": {
            "status": app.build_log().status,
            "lines": app.build_log().lines.iter().map(|line| json!({
                "stream": log_stream_name(line.stream),
                "text": line.text,
            })).collect::<Vec<_>>(),
        },
        "history": app.history().map(|history| json!({
            "read_only": history.read_only(),
            "reason": history.read_only_reason(),
            "generations": history.generations.iter().map(|generation| json!({
                "id": generation.id,
                "link": generation.link,
                "target": generation.target,
            })).collect::<Vec<_>>(),
            "snapshots": history.snapshots.iter().map(|snapshot| json!({
                "id": snapshot.id,
                "created": snapshot.created_unix_secs,
                "outcome": snapshot.outcome,
                "files": snapshot.files,
            })).collect::<Vec<_>>(),
        })),
    })
}

fn step_name(step: Step) -> &'static str {
    match step {
        Step::Discover => "discover",
        Step::Catalog => "catalog",
        Step::Plan => "plan",
        Step::Validation => "validation",
        Step::Confirm => "confirm",
        Step::BuildLog => "log",
        Step::History => "history",
    }
}

fn plan_state_name(state: &PlanState) -> &'static str {
    match state {
        PlanState::None => "none",
        PlanState::Pending => "pending",
        PlanState::Ready => "ready",
        PlanState::Failed(_) => "failed",
    }
}

fn log_stream_name(stream: LogStream) -> &'static str {
    match stream {
        LogStream::Stdout => "stdout",
        LogStream::Stderr => "stderr",
        LogStream::Status => "status",
    }
}

#[allow(dead_code)]
fn refusal_name(code: RefusalCode) -> &'static str {
    code.message()
}

fn main() {
    let shared = Shared {
        app: Mutex::new(App::new()),
        session: Session::new(None, None),
    };

    if std::env::args().any(|arg| arg == "--selftest") {
        run_selftest(&shared);
        return;
    }
    // The window asks for detection on load; the preload path drives it here so a
    // screenshot lands on a populated screen.
    if let Ok(screen) = std::env::var("NHS_SPIKE_SCREEN") {
        feed(&shared, shared.session.discover());
        preload(&shared, &screen);
    }

    tauri::Builder::default()
        .manage(shared)
        .invoke_handler(tauri::generate_handler![state, handle_event])
        .run(tauri::generate_context!())
        .expect("tauri runtime");
}
