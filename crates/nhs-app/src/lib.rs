//! The application layer: what the window knows, and what it is allowed to ask for.
//!
//! This crate is the whole of the GUI's *logic* and none of its drawing. It has no
//! toolkit, no filesystem, no processes and no shell: an adapter turns user input
//! into [`Event`]s, calls this state machine, and performs the [`Intent`]s it gets
//! back by calling `nhs-core`. That split is what makes the flows testable without
//! a window, and what lets the toolkit be chosen (M6.2) without touching
//! correctness.
//!
//! Two rules are enforced here rather than trusted to the adapter:
//!
//! * **nothing applies without a reviewed, validated, explicitly confirmed plan**,
//!   and every one of those facts is bound to the revision of the selection it was
//!   produced from — change the selection and the older facts stop counting;
//! * **activation does not exist.** There is no `Intent` for it, [`App::switch_available`]
//!   is always false, and the reason is a string the UI must show.
//!
//! Discovery is requested with [`Event::DiscoverRequested`] and reported back as
//! [`Event::Discovered`]; a layout change invalidates anything built against the
//! old one, exactly as a selection change does.

pub mod strings;

use std::path::PathBuf;

use nhs_core::catalog::Filters;
use nhs_core::discovery::{Discovery, Layout};
use nhs_core::edit::Plan;
use nhs_core::validation::ValidationReport;
use nhs_core::Setting;

/// How many log lines the view model keeps before dropping the oldest.
pub const MAX_LOG_LINES: usize = 5_000;

/// Which part of the flow the window is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Discover,
    Catalog,
    Plan,
    Validation,
    Confirm,
    BuildLog,
    History,
}

/// Why an action was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalCode {
    EmptySelection,
    NoPlan,
    PlanNotReady,
    NotValidated,
    ValidationFailed,
    DiffNotReviewed,
    NotConfirmed,
    LayoutNotEditable,
    AlreadyApplied,
}

impl RefusalCode {
    /// A stable, translatable key for this refusal. Front ends map it to their
    /// own wording; the English [`Self::message`] stays for logs and fallbacks.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::EmptySelection => "empty_selection",
            Self::NoPlan => "no_plan",
            Self::PlanNotReady => "plan_not_ready",
            Self::NotValidated => "not_validated",
            Self::ValidationFailed => "validation_failed",
            Self::DiffNotReviewed => "diff_not_reviewed",
            Self::NotConfirmed => "not_confirmed",
            Self::LayoutNotEditable => "layout_not_editable",
            Self::AlreadyApplied => "already_applied",
        }
    }

    /// The sentence the UI shows next to the disabled control.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::EmptySelection => strings::EMPTY_SELECTION,
            Self::NoPlan => strings::NO_PLAN,
            Self::PlanNotReady => strings::PLAN_NOT_READY,
            Self::NotValidated => strings::NOT_VALIDATED,
            Self::ValidationFailed => strings::VALIDATION_FAILED,
            Self::DiffNotReviewed => strings::DIFF_NOT_REVIEWED,
            Self::NotConfirmed => strings::NOT_CONFIRMED,
            Self::LayoutNotEditable => strings::LAYOUT_NOT_EDITABLE,
            Self::AlreadyApplied => strings::ALREADY_APPLIED,
        }
    }
}

/// A refusal, ready to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal {
    pub code: RefusalCode,
    pub message: &'static str,
}

impl Refusal {
    fn new(code: RefusalCode) -> Self {
        Self {
            code,
            message: code.message(),
        }
    }
}

/// Work for the adapter. The adapter is the only thing that touches the world.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    /// Read the layout, toolchain and generations.
    Discover,
    /// Search the catalogue. `filters` are core's own, so the meaning is shared.
    Search {
        query: String,
        filters: Filters,
        page: usize,
        per_page: usize,
    },
    /// Render the sidecar, inject the import and stage the changes.
    BuildPlan { revision: u64 },
    /// Validate the plan in a scratch copy. `attempt` identifies this run, so a
    /// result from an earlier run cannot overwrite a later one.
    Validate { revision: u64, attempt: u64 },
    /// Write the reviewed plan through `EditEngine::commit`.
    Apply { revision: u64 },
    /// Read generations and past snapshots (read-only).
    LoadHistory,
}

/// What came back from the state machine.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Response {
    pub intents: Vec<Intent>,
    pub refusal: Option<Refusal>,
}

impl Response {
    fn none() -> Self {
        Self::default()
    }

    fn intent(intent: Intent) -> Self {
        Self {
            intents: vec![intent],
            refusal: None,
        }
    }

    fn refuse(code: RefusalCode) -> Self {
        Self {
            intents: Vec::new(),
            refusal: Some(Refusal::new(code)),
        }
    }
}

/// One catalogue row, as the list needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageHit {
    pub attr: String,
    pub name: String,
    /// `None` for curated entries, which carry no version.
    pub version: Option<String>,
    pub description: String,
    pub category: Option<String>,
    /// `None` means unknown; it is shown as unknown, never hidden.
    pub license: Option<String>,
    /// The catalogue's tags; empty means unknown support.
    pub platforms: Vec<String>,
}

/// The catalogue screen's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogView {
    pub query: String,
    pub filters: Filters,
    pub page: usize,
    pub per_page: usize,
    pub total: usize,
    pub pages: usize,
    pub hits: Vec<PackageHit>,
    /// True once a search has returned, so "no results" is distinguishable from
    /// "not searched yet".
    pub searched: bool,
}

impl Default for CatalogView {
    fn default() -> Self {
        Self {
            query: String::new(),
            filters: Filters::default(),
            page: 1,
            per_page: 20,
            total: 0,
            pages: 0,
            hits: Vec::new(),
            searched: false,
        }
    }
}

/// Where the plan for the current revision stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanState {
    /// Nothing requested.
    None,
    /// Waiting for the adapter.
    Pending,
    /// Ready and bound to the current revision.
    Ready,
    /// The adapter could not build it (a refusal from `nhs-core`, e.g. a
    /// hand-modified managed block).
    Failed(String),
}

/// A finished validation, bound to the revision it was produced from.
#[derive(Debug, Clone)]
pub struct ValidationView {
    pub revision: u64,
    pub report: ValidationReport,
}

/// What the user said about the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewState {
    NotReviewed,
    Acknowledged { revision: u64 },
}

/// One Home Manager generation, read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationSummary {
    pub id: Option<u32>,
    pub link: PathBuf,
    pub target: Option<PathBuf>,
    /// The generation the `home-manager` profile currently points at.
    pub current: bool,
}

/// One past edit snapshot, read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSummary {
    pub id: String,
    pub created_unix_secs: u64,
    pub outcome: String,
    pub files: usize,
}

/// The history screen: two lists that must not be mixed up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryView {
    pub generations: Vec<GenerationSummary>,
    pub snapshots: Vec<SnapshotSummary>,
}

impl HistoryView {
    /// Always true: this version has no rollback, so nothing here is an action.
    #[must_use]
    pub fn read_only(&self) -> bool {
        true
    }

    /// The line the UI shows above both lists.
    #[must_use]
    pub fn read_only_reason(&self) -> &'static str {
        strings::HISTORY_READ_ONLY
    }

    /// Stable key for [`Self::read_only_reason`], for a translated UI.
    #[must_use]
    pub fn read_only_code(&self) -> &'static str {
        "history_read_only"
    }
}

/// Which pipe a log line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStream {
    Stdout,
    Stderr,
    /// A status line produced by this application, not by the build.
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub stream: LogStream,
    pub text: String,
}

/// The build log, bounded so a chatty build cannot grow the view without limit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildLogView {
    pub lines: Vec<LogLine>,
    /// How many lines were dropped from the front.
    pub dropped: usize,
    pub status: Option<String>,
}

/// How the write ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyState {
    Idle,
    Running,
    Applied {
        revision: u64,
        snapshot_id: String,
        written: Vec<PathBuf>,
    },
    Failed {
        revision: u64,
        message: String,
    },
}

/// The result the adapter reports after `EditEngine::commit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyResult {
    Applied {
        snapshot_id: String,
        written: Vec<PathBuf>,
    },
    Failed {
        message: String,
    },
}

/// Everything the window knows.
#[derive(Debug)]
pub struct App {
    step: Step,
    discovery: Option<Discovery>,
    discovery_error: Option<String>,
    catalog: CatalogView,
    selected: Vec<String>,
    settings: Vec<Setting>,
    /// Identity of the current plan attempt.
    ///
    /// Bumped when the selection changes **and** on every plan request, because
    /// two plans built from the same selection are still two different plans:
    /// a result from the first must not be mistaken for the second.
    revision: u64,
    /// Identity of the current validation attempt, so an older validation that
    /// finishes later cannot speak for a newer one.
    validation_attempt: u64,
    /// The attempt number whose final result was already accepted. A request has
    /// exactly one outcome; anything after it is a duplicate, not an update.
    validation_done: Option<u64>,
    plan: Option<Plan>,
    plan_state: PlanState,
    validation: Option<ValidationView>,
    review: ReviewState,
    confirmed_for: Option<u64>,
    log: BuildLogView,
    apply: ApplyState,
    history: Option<HistoryView>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self {
            step: Step::Discover,
            discovery: None,
            discovery_error: None,
            catalog: CatalogView::default(),
            selected: Vec::new(),
            settings: Vec::new(),
            revision: 0,
            validation_attempt: 0,
            validation_done: None,
            plan: None,
            plan_state: PlanState::None,
            validation: None,
            review: ReviewState::NotReviewed,
            confirmed_for: None,
            log: BuildLogView::default(),
            apply: ApplyState::Idle,
            history: None,
        }
    }

    // -- reads -------------------------------------------------------------

    #[must_use]
    pub fn step(&self) -> Step {
        self.step
    }

    #[must_use]
    pub fn discovery(&self) -> Option<&Discovery> {
        self.discovery.as_ref()
    }

    #[must_use]
    pub fn discovery_error(&self) -> Option<&str> {
        self.discovery_error.as_deref()
    }

    #[must_use]
    pub fn catalog(&self) -> &CatalogView {
        &self.catalog
    }

    #[must_use]
    pub fn selected(&self) -> &[String] {
        &self.selected
    }

    #[must_use]
    pub fn settings(&self) -> &[Setting] {
        &self.settings
    }

    #[must_use]
    pub fn is_selected(&self, attr: &str) -> bool {
        self.selected.iter().any(|selected| selected == attr)
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn plan(&self) -> Option<&Plan> {
        self.plan.as_ref()
    }

    #[must_use]
    pub fn plan_state(&self) -> &PlanState {
        &self.plan_state
    }

    #[must_use]
    pub fn validation(&self) -> Option<&ValidationView> {
        self.validation.as_ref()
    }

    /// The number of the validation run currently in flight (or last requested).
    #[must_use]
    pub fn validation_attempt(&self) -> u64 {
        self.validation_attempt
    }

    #[must_use]
    pub fn review(&self) -> ReviewState {
        self.review
    }

    #[must_use]
    pub fn build_log(&self) -> &BuildLogView {
        &self.log
    }

    #[must_use]
    pub fn apply(&self) -> &ApplyState {
        &self.apply
    }

    #[must_use]
    pub fn history(&self) -> Option<&HistoryView> {
        self.history.as_ref()
    }

    /// Activation is not part of this version. There is no intent for it either.
    #[must_use]
    pub fn switch_available(&self) -> bool {
        false
    }

    /// Why activation is unavailable; the UI must show this where the control is.
    #[must_use]
    pub fn switch_reason(&self) -> &'static str {
        strings::SWITCH_DISABLED
    }

    /// A stable key for [`Self::switch_reason`], so a translated UI does not have
    /// to match on English prose.
    #[must_use]
    pub fn switch_reason_code(&self) -> &'static str {
        "switch_disabled"
    }

    /// True when the detected layout has a configuration this application may edit.
    #[must_use]
    pub fn editable(&self) -> bool {
        self.discovery.as_ref().is_some_and(|discovery| {
            matches!(
                discovery.layout,
                Layout::FlakeHome { .. } | Layout::ClassicHome { .. }
            )
        })
    }

    /// The configuration root, when the layout names one.
    #[must_use]
    pub fn config_root(&self) -> Option<PathBuf> {
        match self.discovery.as_ref().map(|discovery| &discovery.layout) {
            Some(Layout::FlakeHome { root, .. }) => Some(root.clone()),
            Some(Layout::ClassicHome { main_file }) => {
                main_file.parent().map(std::path::Path::to_path_buf)
            }
            _ => None,
        }
    }

    /// True when the user has acknowledged the diff of the *current* plan.
    #[must_use]
    pub fn reviewed_for_current(&self) -> bool {
        matches!(
            self.review,
            ReviewState::Acknowledged { revision } if revision == self.revision
        )
    }

    /// Whether the "review and apply" button may be pressed.
    ///
    /// # Errors
    ///
    /// Returns the reason the button must stay disabled: no editable layout, no
    /// plan, an unfinished plan, no validation, a failed validation, or an
    /// unacknowledged diff.
    pub fn apply_readiness(&self) -> Result<(), RefusalCode> {
        self.ticket().map(|_| ())
    }

    /// The single gate every path to `Apply` goes through.
    ///
    /// Returns a token so the caller cannot "almost" satisfy it: the checks are
    /// performed once, here, and the token is not constructible anywhere else.
    fn ticket(&self) -> Result<ApplyTicket, RefusalCode> {
        if !self.editable() {
            return Err(RefusalCode::LayoutNotEditable);
        }
        if self.plan.is_none() {
            return Err(RefusalCode::NoPlan);
        }
        if self.plan_state != PlanState::Ready {
            return Err(RefusalCode::PlanNotReady);
        }
        let Some(validation) = &self.validation else {
            return Err(RefusalCode::NotValidated);
        };
        if validation.revision != self.revision {
            return Err(RefusalCode::NotValidated);
        }
        if !validation.report.success {
            return Err(RefusalCode::ValidationFailed);
        }
        match self.review {
            ReviewState::Acknowledged { revision } if revision == self.revision => {}
            _ => return Err(RefusalCode::DiffNotReviewed),
        }
        // The same plan must not be written twice: returning to the
        // confirmation screen after a successful apply would otherwise pass
        // every other check, because plan, validation and review still match.
        if matches!(self.apply, ApplyState::Applied { revision, .. } if revision == self.revision) {
            return Err(RefusalCode::AlreadyApplied);
        }
        Ok(ApplyTicket(()))
    }

    // -- the state machine -------------------------------------------------

    /// Feed one event in, get the work (or the refusal) out.
    #[allow(clippy::too_many_lines)]
    pub fn handle(&mut self, event: Event) -> Response {
        match event {
            Event::DiscoverRequested => Response::intent(Intent::Discover),
            Event::Discovered(discovery) => {
                let changed = self.discovery.as_ref().is_some_and(|old| {
                    layout_identity(&old.layout) != layout_identity(&discovery.layout)
                });
                self.discovery_error = None;
                self.discovery = Some(discovery);
                // Unconditional on purpose: a plan that is still being built
                // has no `plan` value yet, so "invalidate only if something
                // exists" would let the old layout's result land after the
                // switch.
                if changed {
                    self.invalidate_selection();
                }
                Response::none()
            }
            Event::DiscoveryFailed(message) => {
                self.discovery_error = Some(message);
                Response::none()
            }
            Event::SearchSubmitted {
                query,
                filters,
                page,
                per_page,
            } => {
                self.catalog.query.clone_from(&query);
                self.catalog.filters.clone_from(&filters);
                self.catalog.page = page;
                self.catalog.per_page = per_page;
                self.step = Step::Catalog;
                Response::intent(Intent::Search {
                    query,
                    filters,
                    page,
                    per_page,
                })
            }
            Event::SearchResults {
                hits,
                total,
                page,
                pages,
            } => {
                self.catalog.hits = hits;
                self.catalog.total = total;
                self.catalog.page = page;
                self.catalog.pages = pages;
                self.catalog.searched = true;
                self.step = Step::Catalog;
                Response::none()
            }
            Event::PackageToggled(attr) => {
                if let Some(index) = self.selected.iter().position(|name| name == &attr) {
                    self.selected.remove(index);
                } else {
                    self.selected.push(attr);
                }
                self.invalidate_selection();
                Response::none()
            }
            Event::SettingAdded(setting) => {
                self.settings
                    .retain(|existing| existing.path != setting.path);
                self.settings.push(setting);
                self.invalidate_selection();
                Response::none()
            }
            Event::SettingRemoved(path) => {
                self.settings.retain(|existing| existing.path != path);
                self.invalidate_selection();
                Response::none()
            }
            Event::PlanRequested => {
                if !self.editable() {
                    return Response::refuse(RefusalCode::LayoutNotEditable);
                }
                if self.selected.is_empty() && self.settings.is_empty() {
                    return Response::refuse(RefusalCode::EmptySelection);
                }
                // A new request is a new identity even when the selection is
                // unchanged: the previous plan may still be in flight.
                self.revision = self.revision.saturating_add(1);
                self.plan = None;
                self.plan_state = PlanState::Pending;
                self.validation = None;
                self.validation_attempt = 0;
                self.validation_done = None;
                self.review = ReviewState::NotReviewed;
                self.confirmed_for = None;
                self.step = Step::Plan;
                Response::intent(Intent::BuildPlan {
                    revision: self.revision,
                })
            }
            Event::PlanBuilt { revision, plan } => {
                // Both conditions matter: the revision rules out a plan for an old
                // selection, and `Pending` rules out a second result for a request
                // that already produced one.
                if revision != self.revision || self.plan_state != PlanState::Pending {
                    return Response::none();
                }
                self.plan = Some(plan);
                self.plan_state = PlanState::Ready;
                self.step = Step::Plan;
                Response::none()
            }
            Event::PlanFailed { revision, message } => {
                if revision != self.revision || self.plan_state != PlanState::Pending {
                    return Response::none();
                }
                self.plan = None;
                self.plan_state = PlanState::Failed(message);
                self.step = Step::Plan;
                Response::none()
            }
            Event::ValidationRequested => {
                match self.plan_state {
                    PlanState::Pending => return Response::refuse(RefusalCode::PlanNotReady),
                    PlanState::Ready if self.plan.is_some() => {}
                    _ => return Response::refuse(RefusalCode::NoPlan),
                }
                self.validation_attempt = self.validation_attempt.saturating_add(1);
                // The previous result speaks for the previous run; a late copy of
                // it must not be able to overwrite this one. The *diff* has not
                // changed, though, so the user's acknowledgement of it stands:
                // clearing it here made the natural order (review, then validate)
                // impossible to complete.
                self.validation = None;
                self.validation_done = None;
                self.confirmed_for = None;
                self.step = Step::Validation;
                Response::intent(Intent::Validate {
                    revision: self.revision,
                    attempt: self.validation_attempt,
                })
            }
            Event::ValidationFinished {
                revision,
                attempt,
                report,
            } => {
                // A result is only accepted for the current plan and the current
                // validation run: a newer run, or a newer plan, makes it stale.
                if revision != self.revision
                    || attempt == 0
                    || attempt != self.validation_attempt
                    || self.plan_state != PlanState::Ready
                {
                    return Response::none();
                }
                if self.validation_done == Some(attempt) {
                    // This request already produced its one outcome. A second
                    // result would let a late success overwrite a failure.
                    return Response::none();
                }
                self.validation_done = Some(attempt);
                self.validation = Some(ValidationView { revision, report });
                self.step = Step::Validation;
                Response::none()
            }
            Event::DiffReviewed => {
                if self.plan.is_none() || self.plan_state != PlanState::Ready {
                    return Response::refuse(RefusalCode::NoPlan);
                }
                self.review = ReviewState::Acknowledged {
                    revision: self.revision,
                };
                Response::none()
            }
            Event::ConfirmationRequested => {
                if let Err(code) = self.ticket() {
                    return Response::refuse(code);
                }
                self.step = Step::Confirm;
                Response::none()
            }
            Event::ApplyConfirmed => {
                // Re-checked here, not only on the way in: a validation that
                // finished while the confirmation was open may have failed. The
                // ticket runs first so the message names the real problem rather
                // than "you are on the wrong screen".
                if let Err(code) = self.ticket() {
                    return Response::refuse(code);
                }
                if self.step != Step::Confirm {
                    return Response::refuse(RefusalCode::NotConfirmed);
                }
                self.confirmed_for = Some(self.revision);
                self.apply = ApplyState::Running;
                self.log = BuildLogView::default();
                self.log.status = Some(strings::APPLY_RUNNING.to_string());
                self.step = Step::BuildLog;
                Response::intent(Intent::Apply {
                    revision: self.revision,
                })
            }
            Event::ConfirmationCancelled => {
                self.confirmed_for = None;
                self.step = Step::Validation;
                Response::none()
            }
            Event::ApplyFinished { revision, result } => {
                // Recorded whatever the revision: the write happened, and hiding
                // that would be a lie about the disk.
                match result {
                    ApplyResult::Applied {
                        snapshot_id,
                        written,
                    } => {
                        self.apply = ApplyState::Applied {
                            revision,
                            snapshot_id,
                            written,
                        };
                        self.log.status = Some(strings::APPLY_SUCCEEDED.to_string());
                    }
                    ApplyResult::Failed { message } => {
                        self.log.status = Some(strings::APPLY_FAILED.to_string());
                        self.apply = ApplyState::Failed { revision, message };
                    }
                }
                Response::none()
            }
            Event::LogLine { stream, text } => {
                self.log.lines.push(LogLine { stream, text });
                while self.log.lines.len() > MAX_LOG_LINES {
                    self.log.lines.remove(0);
                    self.log.dropped += 1;
                }
                Response::none()
            }
            Event::Progress(message) => {
                self.log.status = Some(message);
                Response::none()
            }
            Event::HistoryRequested => {
                self.step = Step::History;
                Response::intent(Intent::LoadHistory)
            }
            Event::HistoryLoaded {
                generations,
                snapshots,
            } => {
                self.history = Some(HistoryView {
                    generations,
                    snapshots,
                });
                self.step = Step::History;
                Response::none()
            }
            Event::StepRequested(step) => {
                if step == Step::Confirm {
                    // Navigation must not be a way around the review gate.
                    return self.handle(Event::ConfirmationRequested);
                }
                self.step = step;
                Response::none()
            }
        }
    }

    /// A selection change makes every derived fact stale.
    fn invalidate_selection(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.plan = None;
        self.plan_state = PlanState::None;
        self.validation = None;
        self.validation_attempt = 0;
        self.validation_done = None;
        self.review = ReviewState::NotReviewed;
        self.confirmed_for = None;
    }

    /// True when the user has confirmed this exact revision (used by the adapter
    /// to double-check before writing).
    #[must_use]
    pub fn confirmed_for(&self) -> Option<u64> {
        self.confirmed_for
    }
}

/// What makes two layouts "the same configuration" for plan validity.
///
/// Every field that decides *where* a plan is written or *what* it is validated
/// against belongs here: the kind, the main file, the flake attribute (the target
/// `homeConfigurations.<attr>`), the NixOS host/user pair, and the profile
/// directory. Missing one would let a re-discovery that changes the target keep a
/// plan built for the old one.
#[derive(Debug, PartialEq, Eq)]
struct LayoutIdentity {
    kind: &'static str,
    main_file: Option<PathBuf>,
    attr: Option<String>,
    host: Option<String>,
    user: Option<String>,
    profile_dir: Option<PathBuf>,
}

fn layout_identity(layout: &Layout) -> LayoutIdentity {
    let mut identity = LayoutIdentity {
        kind: layout.kind(),
        main_file: layout.main_file().map(std::path::Path::to_path_buf),
        attr: None,
        host: None,
        user: None,
        profile_dir: None,
    };
    match layout {
        Layout::FlakeHome { attr, .. } => identity.attr = Some(attr.clone()),
        Layout::FlakeNixos { host, user, .. } => {
            identity.host = Some(host.clone());
            identity.user = Some(user.clone());
        }
        Layout::ProfileOnly { profile_dir, .. } => {
            identity.profile_dir = Some(profile_dir.clone());
        }
        Layout::ClassicHome { .. } | Layout::Unknown { .. } => {}
    }
    identity
}

/// Proof that the review gate was satisfied. Not constructible outside this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ApplyTicket(());

/// One thing the user did, or one thing that came back from the world.
#[derive(Debug, Clone)]
pub enum Event {
    /// The user (or the window at start-up) asks for detection to run.
    DiscoverRequested,
    Discovered(Discovery),
    DiscoveryFailed(String),
    SearchSubmitted {
        query: String,
        filters: Filters,
        page: usize,
        per_page: usize,
    },
    SearchResults {
        hits: Vec<PackageHit>,
        total: usize,
        page: usize,
        pages: usize,
    },
    PackageToggled(String),
    SettingAdded(Setting),
    SettingRemoved(String),
    PlanRequested,
    PlanBuilt {
        revision: u64,
        plan: Plan,
    },
    PlanFailed {
        revision: u64,
        message: String,
    },
    ValidationRequested,
    ValidationFinished {
        revision: u64,
        attempt: u64,
        report: ValidationReport,
    },
    DiffReviewed,
    ConfirmationRequested,
    ApplyConfirmed,
    ConfirmationCancelled,
    ApplyFinished {
        revision: u64,
        result: ApplyResult,
    },
    LogLine {
        stream: LogStream,
        text: String,
    },
    Progress(String),
    HistoryRequested,
    HistoryLoaded {
        generations: Vec<GenerationSummary>,
        snapshots: Vec<SnapshotSummary>,
    },
    StepRequested(Step),
}
