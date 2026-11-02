//! The JSON contract between the Rust state machine and the web UI.
//!
//! Everything the front end may know about comes through [`UiState`]; everything
//! it may ask for comes through [`event_from_json`] → `nhs_app::Event`. The front
//! end never sends an intent, a revision or a safety decision — those stay in
//! `nhs-app`, which is why the UI cannot bypass the review/validation gate.

use nhs_app::{App, ApplyState, Event, LogStream, PlanState, ReviewState, Step};
use serde::Serialize;
use serde_json::Value;

/// Bumped whenever the shape below changes incompatibly. The front end refuses to
/// render a version it does not know instead of showing half a state.
pub const STATE_VERSION: u32 = 3;

pub const APP_NAME: &str = "Nix Home Studio";
pub const APP_IDENTIFIER: &str = "dev.local.nhs.studio";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Serialize)]
pub struct UiState {
    pub version: u32,
    pub app: AppInfo,
    pub step: &'static str,
    pub switch: SwitchInfo,
    pub discovery: Option<DiscoveryInfo>,
    pub discovery_error: Option<Failure>,
    /// What this build can actually do. `false` here is the honest answer for
    /// M6.3: the screens exist in the navigation but are disabled.
    pub capabilities: Capabilities,
    pub selected: usize,
    /// Why the apply button is not ready, as a code plus the engine's wording.
    pub readiness: Option<Readiness>,
    /// The background job, while one runs. `None` means the window may dispatch.
    pub busy: Option<BusyInfo>,
    pub catalog: CatalogView,
    pub plan: Option<PlanView>,
    pub validation: Option<ValidationView>,
    pub reviewed: bool,
    pub apply: ApplyView,
    pub log: LogView,
    pub history: Option<HistoryView>,
}

#[derive(Debug, Serialize)]
pub struct CatalogView {
    pub query: String,
    pub total: usize,
    pub searched: bool,
    pub hits: Vec<HitView>,
}

#[derive(Debug, Serialize)]
pub struct HitView {
    pub attr: String,
    pub name: String,
    pub description: String,
    pub category: Option<String>,
    pub license: Option<String>,
    pub selected: bool,
}

#[derive(Debug, Serialize)]
pub struct PlanView {
    /// `none`, `pending`, `ready` or `failed`.
    pub state: &'static str,
    pub notes: Vec<String>,
    pub files: Vec<FileView>,
}

#[derive(Debug, Serialize)]
pub struct FileView {
    pub path: String,
    /// `create` or `replace`.
    pub kind: &'static str,
    pub added: usize,
    pub removed: usize,
    pub diff: String,
    pub noop: bool,
}

#[derive(Debug, Serialize)]
pub struct ValidationView {
    pub success: bool,
    pub command: String,
    pub millis: u128,
    /// Stable code from `ErrorKind::label()`, plus its English label.
    pub kind: Option<DiagnosisView>,
    pub summary: Option<String>,
    pub hint: Option<&'static str>,
    pub location: Option<String>,
    pub scratch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DiagnosisView {
    pub code: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ApplyView {
    /// `idle`, `running`, `applied` or `failed`.
    pub state: &'static str,
    pub snapshot: Option<String>,
    pub written: Vec<String>,
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LogView {
    pub status: Option<String>,
    pub lines: Vec<LogLineView>,
}

#[derive(Debug, Serialize)]
pub struct LogLineView {
    /// `stdout`, `stderr` or `status`.
    pub stream: &'static str,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct HistoryView {
    pub reason_code: &'static str,
    pub reason: &'static str,
    pub generations: Vec<GenerationView>,
    pub snapshots: Vec<SnapshotView>,
}

#[derive(Debug, Serialize)]
pub struct GenerationView {
    pub id: Option<u32>,
    pub link: String,
    pub target: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SnapshotView {
    pub id: String,
    pub created: u64,
    pub outcome: String,
    pub files: usize,
}

/// The job the worker is running. `attempt` is 0 for apply, which has no retry
/// counter of its own.
#[derive(Debug, Serialize)]
pub struct BusyInfo {
    pub kind: &'static str,
    pub revision: u64,
    pub attempt: u64,
}

/// Which job the cancelled request was bound to, so the UI can tell a stale
/// acknowledgement from a current one.
#[derive(Debug, Serialize)]
pub struct CancelAck {
    pub job: u64,
}

/// The kinds of background job. Kept as a list so the i18n check can derive the
/// `action_*` keys the UI needs for `busy` errors.
pub const JOB_KINDS: [&str; 2] = ["validation", "apply"];

#[derive(Debug, Serialize)]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub identifier: &'static str,
}

#[derive(Debug, Serialize)]
pub struct SwitchInfo {
    pub available: bool,
    /// Stable key (`switch_disabled`); the UI translates it.
    pub reason_code: &'static str,
    /// The engine's English sentence, kept as a fallback for logs and new codes.
    pub reason: &'static str,
}

/// A failure with a stable code and a raw detail. The code is translated by the
/// UI; the detail (an OS message, a step name, an offending token) is shown
/// verbatim and never needs translating.
#[derive(Debug, Serialize)]
pub struct Failure {
    pub code: &'static str,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct Readiness {
    pub code: &'static str,
    pub message: &'static str,
}

#[derive(Debug, Serialize)]
pub struct DiscoveryInfo {
    /// `classic-home`, `flake-home`, `flake-nixos`, `profile-only` or `unknown`.
    pub kind: &'static str,
    pub editable: bool,
    pub root: Option<String>,
    pub main_file: Option<String>,
    pub generations: usize,
}

/// A capability set is naturally several booleans; splitting it would make the
/// JSON harder to read for no gain.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Serialize)]
pub struct Capabilities {
    pub catalog: bool,
    pub plan: bool,
    pub validate: bool,
    pub apply: bool,
}

/// A failure the UI can show as-is. Never a bare string: the code is stable, the
/// message is for the person, the hint says what to do about it.
#[derive(Debug, Serialize)]
pub struct BridgeError {
    pub code: &'static str,
    /// Stable detail key or raw text (the action's name, an unknown token, an OS
    /// message). The UI translates what it knows and shows the rest verbatim.
    pub detail: Option<String>,
    pub message: String,
    pub hint: &'static str,
}

impl BridgeError {
    #[must_use]
    pub fn bad_event(message: impl Into<String>) -> Self {
        Self {
            code: "bad_event",
            detail: Some(message.into()),
            message: "The interface sent an event the application does not understand.".to_string(),
            hint: "This is a bug in the interface, not something you did.",
        }
    }

    /// The state machine refused the request; the stable refusal code travels as
    /// the detail so the UI can explain it.
    #[must_use]
    pub fn refused(code: &str) -> Self {
        Self {
            code: "refused",
            detail: Some(code.to_string()),
            message: format!("The application refused this action ({code})."),
            hint: "The reason is shown with the action it belongs to.",
        }
    }

    #[must_use]
    pub fn busy(kind: &str) -> Self {
        Self {
            code: "busy",
            detail: Some(kind.to_string()),
            message: format!("A background job ({kind}) is still running."),
            hint: "Wait for it to finish, or cancel it first.",
        }
    }

    /// The job exists but cannot be interrupted (applies write without reading the
    /// cancellation token).
    #[must_use]
    pub fn cancel_unsupported(kind: &str) -> Self {
        Self {
            code: "cancel_unavailable",
            detail: Some(kind.to_string()),
            message: format!("A running {kind} cannot be cancelled."),
            hint: "This step is short and finishes on its own; nothing is left half-written.",
        }
    }

    #[must_use]
    pub fn cancel_unavailable() -> Self {
        Self {
            code: "cancel_unavailable",
            detail: None,
            message: "Cancel is not implemented in this build.".to_string(),
            hint: "No long-running operation exists yet, so nothing was cancelled.",
        }
    }

    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: "internal",
            detail: Some(message.into()),
            message: "The application hit an internal error.".to_string(),
            hint: "Restart the application. If it happens again, report it with this message.",
        }
    }
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {} — {}", self.code, self.message, self.hint)
    }
}

impl std::error::Error for BridgeError {}

/// Parse one event from the web layer.
///
/// The whole event set is accepted here so that the contract is stable for M6.4;
/// what this *build* refuses is decided in `Service::dispatch`, where the refusal
/// can also promise that nothing was executed.
///
/// # Errors
/// Returns [`BridgeError`] with code `bad_event` when the payload is not an object,
/// has no `kind`, or names something unknown.
pub fn event_from_json(value: &Value) -> Result<Event, BridgeError> {
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| BridgeError::bad_event("Event without a `kind`."))?;
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
        "toggle" => value
            .get("attr")
            .and_then(Value::as_str)
            .map(|attr| Event::PackageToggled(attr.to_string()))
            .ok_or_else(|| BridgeError::bad_event("`toggle` without `attr`.")),
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
                other => {
                    return Err(BridgeError::bad_event(format!("Unknown step {other:?}.")));
                }
            };
            Ok(Event::StepRequested(step))
        }
        other => Err(BridgeError::bad_event(format!(
            "Unknown event kind {other:?}."
        ))),
    }
}

/// A snapshot of everything the UI renders. Read-only by construction: it only
/// borrows from the state machine.
#[must_use]
pub fn state_of(app: &App) -> UiState {
    UiState {
        version: STATE_VERSION,
        app: AppInfo {
            name: APP_NAME,
            version: APP_VERSION,
            identifier: APP_IDENTIFIER,
        },
        step: step_name(app.step()),
        switch: SwitchInfo {
            available: app.switch_available(),
            reason_code: app.switch_reason_code(),
            reason: app.switch_reason(),
        },
        discovery: discovery_view(app),
        discovery_error: app.discovery_error().map(|detail| Failure {
            code: "discovery_failed",
            detail: detail.to_string(),
        }),
        capabilities: Capabilities {
            // The backend implements all four; whether the window renders them is
            // a front-end concern, kept out of this contract on purpose.
            catalog: true,
            plan: true,
            validate: true,
            apply: true,
        },
        selected: app.selected().len(),
        readiness: app.apply_readiness().err().map(|refusal| Readiness {
            code: refusal.code(),
            message: refusal.message(),
        }),
        busy: None,
        catalog: catalog_view(app),
        plan: plan_view(app),
        validation: validation_view(app),
        reviewed: matches!(app.review(), ReviewState::Acknowledged { .. }),
        apply: apply_view(app),
        log: log_view(app),
        history: history_view(app),
    }
}

fn discovery_view(app: &App) -> Option<DiscoveryInfo> {
    app.discovery().map(|found| DiscoveryInfo {
        kind: found.layout.kind(),
        editable: app.editable(),
        root: app
            .config_root()
            .map(|path| path.to_string_lossy().into_owned()),
        main_file: found
            .layout
            .main_file()
            .map(|path| path.to_string_lossy().into_owned()),
        generations: found.layout.generations().len(),
    })
}

fn catalog_view(app: &App) -> CatalogView {
    CatalogView {
        query: app.catalog().query.clone(),
        total: app.catalog().total,
        searched: app.catalog().searched,
        hits: app
            .catalog()
            .hits
            .iter()
            .map(|hit| HitView {
                attr: hit.attr.clone(),
                name: hit.name.clone(),
                description: hit.description.clone(),
                category: hit.category.clone(),
                license: hit.license.clone(),
                selected: app.is_selected(&hit.attr),
            })
            .collect(),
    }
}

fn plan_view(app: &App) -> Option<PlanView> {
    app.plan().map(|plan| PlanView {
        state: plan_state_name(app.plan_state()),
        notes: plan.notes.clone(),
        files: plan
            .changes
            .iter()
            .map(|change| FileView {
                path: change.path.to_string_lossy().into_owned(),
                kind: match change.kind {
                    nhs_core::ChangeKind::Create => "create",
                    nhs_core::ChangeKind::Replace => "replace",
                },
                added: change.added_lines,
                removed: change.removed_lines,
                diff: change.unified_diff.clone(),
                noop: change.is_noop(),
            })
            .collect(),
    })
}

fn validation_view(app: &App) -> Option<ValidationView> {
    app.validation().map(|validation| {
        let report = &validation.report;
        ValidationView {
            success: report.success,
            command: report.command.clone(),
            millis: report.elapsed.as_millis(),
            kind: report.diagnosis.as_ref().map(|diagnosis| DiagnosisView {
                code: diagnosis.kind.label(),
                label: diagnosis.kind.label(),
            }),
            summary: report
                .diagnosis
                .as_ref()
                .map(|diagnosis| diagnosis.summary.clone()),
            hint: report.diagnosis.as_ref().map(|diagnosis| diagnosis.hint),
            location: report
                .diagnosis
                .as_ref()
                .and_then(|diagnosis| diagnosis.location.clone()),
            scratch: report
                .kept_scratch
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        }
    })
}

fn apply_view(app: &App) -> ApplyView {
    match app.apply() {
        ApplyState::Idle => ApplyView {
            state: "idle",
            snapshot: None,
            written: Vec::new(),
            message: None,
        },
        ApplyState::Running => ApplyView {
            state: "running",
            snapshot: None,
            written: Vec::new(),
            message: None,
        },
        ApplyState::Applied {
            snapshot_id,
            written,
            ..
        } => ApplyView {
            state: "applied",
            snapshot: Some(snapshot_id.clone()),
            written: written
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            message: None,
        },
        ApplyState::Failed { message, .. } => ApplyView {
            state: "failed",
            snapshot: None,
            written: Vec::new(),
            message: Some(message.clone()),
        },
    }
}

fn log_view(app: &App) -> LogView {
    LogView {
        status: app.build_log().status.clone(),
        lines: app
            .build_log()
            .lines
            .iter()
            .map(|line| LogLineView {
                stream: log_stream_name(line.stream),
                text: line.text.clone(),
            })
            .collect(),
    }
}

fn history_view(app: &App) -> Option<HistoryView> {
    app.history().map(|history| HistoryView {
        reason_code: history.read_only_code(),
        reason: history.read_only_reason(),
        generations: history
            .generations
            .iter()
            .map(|generation| GenerationView {
                id: generation.id,
                link: generation.link.to_string_lossy().into_owned(),
                target: generation
                    .target
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
            })
            .collect(),
        snapshots: history
            .snapshots
            .iter()
            .map(|snapshot| SnapshotView {
                id: snapshot.id.clone(),
                created: snapshot.created_unix_secs,
                outcome: snapshot.outcome.clone(),
                files: snapshot.files,
            })
            .collect(),
    })
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

#[must_use]
pub fn step_name(step: Step) -> &'static str {
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
