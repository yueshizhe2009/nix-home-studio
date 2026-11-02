//! iced prototype for M6.2 — the same seven steps as the Tauri spike, over the
//! same `nhs-app` + `nhs-adapter`.
//!
//! Everything the two prototypes share is deliberately identical: the state
//! machine, the adapter, the scratch HOME convention, and the `--selftest`
//! sequence. Only the rendering and the event plumbing differ, which is what the
//! comparison is supposed to measure.
//!
//! Layout rule shared with the Tauri prototype: the scrolling area holds the
//! content, the action row sits *outside* it so the buttons cannot fall below the
//! fold, and the current step is visually marked.

mod i18n;

use iced::widget::{button, checkbox, column, container, row, scrollable, text, text_input};
use iced::{Element, Font, Length, Task};

use nhs_adapter::Session;
use nhs_app::{App, ApplyState, Event, Intent, LogStream, PlanState, ReviewState, Step};
use nhs_core::catalog::Filters;
use nhs_core::flow::Selection;

use i18n::{strings, Lang, Strings};

#[derive(Debug, Clone)]
enum Message {
    /// One state-machine event.
    Event(Event),
    /// The search box changed.
    Query(String),
    /// Switch the interface language; the flow state is untouched.
    ToggleLanguage,
}

struct Ui {
    app: App,
    session: Session,
    query: String,
    lang: Lang,
}

/// A screen is content plus the actions that belong to it. The actions are laid
/// out outside the scrollable, exactly like the Tauri prototype's sticky row.
type Screen<'a> = (Element<'a, Message>, Element<'a, Message>);

impl Ui {
    fn boot() -> (Self, Task<Message>) {
        let mut ui = Self {
            app: App::new(),
            session: Session::new(None, None),
            query: String::new(),
            lang: Lang::En,
        };
        // The same capture hook as the Tauri spike: drive to a screen before the
        // window opens, so a screenshot shows a populated one.
        if let Ok(screen) = std::env::var("NHS_SPIKE_SCREEN") {
            drive(&mut ui, Event::DiscoverRequested);
            let stages = ["catalog", "plan", "validation", "confirm", "history"];
            if let Some(position) = stages.iter().position(|stage| *stage == screen) {
                ui.query = "firefox".to_string();
                drive(
                    &mut ui,
                    Event::SearchSubmitted {
                        query: "firefox".to_string(),
                        filters: Filters::default(),
                        page: 1,
                        per_page: 20,
                    },
                );
                if position >= 1 {
                    drive(&mut ui, Event::PackageToggled("firefox".to_string()));
                    drive(&mut ui, Event::PlanRequested);
                    drive(&mut ui, Event::DiffReviewed);
                }
                if position >= 2 {
                    drive(&mut ui, Event::ValidationRequested);
                }
                if position >= 3 {
                    drive(&mut ui, Event::ConfirmationRequested);
                }
                if position >= 4 {
                    drive(&mut ui, Event::HistoryRequested);
                }
            }
        }
        if let Ok(lang) = std::env::var("NHS_SPIKE_LANG") {
            ui.lang = if lang == "zh" { Lang::Zh } else { Lang::En };
        }
        (ui, Task::none())
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Query(query) => {
                self.query = query;
                Task::none()
            }
            Message::Event(event) => {
                drive(self, event);
                Task::none()
            }
            Message::ToggleLanguage => {
                self.lang = self.lang.toggled();
                Task::none()
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let s = strings(self.lang);

        let header = row![
            text(format!("{} {}", s.app_title, s.prototype)).size(20),
            container(button(text(s.language)).on_press(Message::ToggleLanguage))
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Right),
        ]
        .align_y(iced::Alignment::Center);

        let nav = column(STEP_ORDER.iter().enumerate().map(|(index, step)| {
            let target = *step;
            let active = *step == self.app.step();
            let mut control = button(text(s.nav[index]).size(14))
                .width(Length::Fill)
                .padding(8);
            let allowed = *step != Step::Confirm
                || (self.app.reviewed_for_current()
                    && self.app.validation().is_some_and(|v| v.report.success));
            if allowed {
                control = control.on_press(Message::Event(Event::StepRequested(target)));
            }
            // The current step is drawn with the primary style; the rest stay
            // secondary. This is the "visual state" the Tauri side gets from
            // `aria-current` plus its own `.active` class.
            control
                .style(if active {
                    button::primary
                } else {
                    button::secondary
                })
                .into()
        }))
        .spacing(4)
        .width(Length::Fixed(190.0));

        let (body, actions) = self.screen(s);

        let footer = text(fill(
            s.footer,
            &[
                step_name(self.app.step()).to_string(),
                self.app.selected().len().to_string(),
                match self.app.apply_readiness() {
                    Ok(()) => s.ready.to_string(),
                    Err(code) => code.message().to_string(),
                },
            ],
        ))
        .size(12);

        container(
            column![
                header,
                row![
                    nav,
                    scrollable(container(body).padding(16)).height(Length::Fill)
                ]
                .spacing(16)
                .height(Length::Fill),
                container(actions).padding(8),
                footer,
            ]
            .spacing(12),
        )
        .padding(16)
        .into()
    }

    fn screen(&self, s: &'static Strings) -> Screen<'_> {
        match self.app.step() {
            Step::Discover => self.view_discover(s),
            Step::Catalog => self.view_catalog(s),
            Step::Plan => self.view_plan(s),
            Step::Validation => self.view_validation(s),
            Step::Confirm => self.view_confirm(s),
            Step::BuildLog => self.view_log(s),
            Step::History => self.view_history(s),
        }
    }

    fn view_discover(&self, s: &'static Strings) -> Screen<'_> {
        let mut body = column![text(s.discover_heading).size(18)].spacing(8);
        let actions;
        match self.app.discovery() {
            None => {
                body = body.push(text(s.nothing_detected));
                actions =
                    row![button(text(s.detect)).on_press(Message::Event(Event::DiscoverRequested))];
            }
            Some(discovery) => {
                let layout = discovery.layout.clone();
                body = body
                    .push(text(format!("{}: {}", s.layout, layout.kind())))
                    .push(text(format!(
                        "{}: {}",
                        s.editable,
                        if self.app.editable() {
                            s.yes
                        } else {
                            s.no_read_only
                        }
                    )))
                    .push(text(format!(
                        "{}: {}",
                        s.root,
                        self.app
                            .config_root()
                            .map_or_else(|| "—".to_string(), |path| path.display().to_string())
                    )))
                    .push(text(format!(
                        "{}: {}",
                        s.main_file,
                        layout
                            .main_file()
                            .map_or_else(|| "—".to_string(), |path| path.display().to_string())
                    )))
                    .push(text(format!(
                        "{}: {}",
                        s.generations,
                        discovery.layout.generations().len()
                    )));
                actions = row![
                    button(text(s.detect_again)).on_press(Message::Event(Event::DiscoverRequested)),
                    button(text(s.choose_packages))
                        .on_press(Message::Event(Event::StepRequested(Step::Catalog))),
                ]
                .spacing(8);
            }
        }
        (body.into(), actions.into())
    }

    fn view_catalog(&self, s: &'static Strings) -> Screen<'_> {
        let catalog = self.app.catalog();
        let mut list = column![].spacing(6);
        for hit in &catalog.hits {
            let selected = self.app.is_selected(&hit.attr);
            list = list.push(
                checkbox(selected)
                    .label(hit.attr.clone())
                    .on_toggle({
                        let attr = hit.attr.clone();
                        move |_| Message::Event(Event::PackageToggled(attr.clone()))
                    })
                    .text_size(14),
            );
            list = list.push(
                text(format!(
                    "    {} · {} {}",
                    hit.category
                        .clone()
                        .unwrap_or_else(|| s.uncategorised.to_string()),
                    s.licence,
                    hit.license
                        .clone()
                        .unwrap_or_else(|| s.licence_unknown.to_string())
                ))
                .size(12),
            );
            list = list.push(text(format!("    {}", hit.description)).size(12));
        }
        if catalog.searched && catalog.hits.is_empty() {
            list = list.push(text(s.no_matches));
        }

        let mut plan_button = button(text(fill(
            s.build_plan_for,
            &[self.app.selected().len().to_string()],
        )));
        if !self.app.selected().is_empty() {
            plan_button = plan_button.on_press(Message::Event(Event::PlanRequested));
        }

        let body = column![
            text(s.catalog_heading).size(18),
            row![
                text_input(s.search_placeholder, &self.query)
                    .on_input(Message::Query)
                    .on_submit(Message::Event(Event::SearchSubmitted {
                        query: self.query.clone(),
                        filters: Filters::default(),
                        page: 1,
                        per_page: 20,
                    }))
                    .width(Length::Fixed(320.0)),
                button(text(s.search)).on_press(Message::Event(Event::SearchSubmitted {
                    query: self.query.clone(),
                    filters: Filters::default(),
                    page: 1,
                    per_page: 20,
                })),
            ]
            .spacing(8),
            text(s.offline_note).size(12),
            list,
        ]
        .spacing(10);

        (body.into(), row![plan_button].into())
    }

    fn view_plan(&self, s: &'static Strings) -> Screen<'_> {
        if self.app.plan().is_none() {
            return (
                column![text(s.plan_heading).size(18), text(s.no_plan),]
                    .spacing(8)
                    .into(),
                row![].into(),
            );
        }
        if *self.app.plan_state() == PlanState::Pending {
            return (text(s.plan_pending).into(), row![].into());
        }

        let plan = self.app.plan().expect("plan");
        let mut body = column![text(s.plan_heading).size(18)].spacing(8);
        for change in &plan.changes {
            if change.is_noop() {
                continue;
            }
            body = body
                .push(text(format!(
                    "{} (+{} −{})",
                    change.path.display(),
                    change.added_lines,
                    change.removed_lines
                )))
                .push(
                    text(change.unified_diff.clone())
                        .font(Font::MONOSPACE)
                        .size(12),
                );
        }
        for note in &plan.notes {
            body = body.push(text(format!("{}: {note}", s.note)).size(12));
        }

        let reviewed = self.app.reviewed_for_current();
        let mut validate = button(text(s.validate_scratch));
        if reviewed {
            validate = validate.on_press(Message::Event(Event::ValidationRequested));
        }
        let actions = row![
            checkbox(reviewed)
                .label(s.reviewed)
                .on_toggle(|_| Message::Event(Event::DiffReviewed)),
            validate,
        ]
        .spacing(8);
        (body.into(), actions.into())
    }

    fn view_validation(&self, s: &'static Strings) -> Screen<'_> {
        let Some(validation) = self.app.validation() else {
            return (
                column![text(s.validation_heading).size(18), text(s.not_validated)]
                    .spacing(8)
                    .into(),
                row![].into(),
            );
        };
        let report = &validation.report;
        let mut body = column![
            text(s.validation_heading).size(18),
            text(format!(
                "{} — {} ms",
                report.command,
                report.elapsed.as_millis()
            )),
        ]
        .spacing(8);
        if report.success {
            body = body.push(text(s.build_succeeds));
        } else if let Some(diagnosis) = &report.diagnosis {
            body = body
                .push(text(format!(
                    "{}: {}",
                    diagnosis.kind.label(),
                    diagnosis.summary
                )))
                .push(text(format!("{}: {}", s.what_to_do, diagnosis.hint)))
                .push(text(diagnosis.location.clone().unwrap_or_default()).size(12));
            if let Some(scratch) = &report.kept_scratch {
                body =
                    body.push(text(fill(s.copy_kept, &[scratch.display().to_string()])).size(12));
            }
        }
        let mut next = button(text(s.review_and_apply));
        if report.success && self.app.reviewed_for_current() {
            next = next.on_press(Message::Event(Event::ConfirmationRequested));
        }
        (body.into(), row![next].into())
    }

    fn view_confirm(&self, s: &'static Strings) -> Screen<'_> {
        let mut body =
            column![text(s.confirm_heading).size(18), text(s.confirm_warning),].spacing(8);
        if let Some(plan) = self.app.plan() {
            for change in &plan.changes {
                if change.is_noop() {
                    continue;
                }
                body = body.push(text(format!("· {}", change.path.display())).size(13));
            }
        }
        let actions = row![
            button(text(s.apply_build_only)).on_press(Message::Event(Event::ApplyConfirmed)),
            button(text(s.cancel)).on_press(Message::Event(Event::ConfirmationCancelled)),
        ]
        .spacing(8);
        (body.into(), actions.into())
    }

    fn view_log(&self, s: &'static Strings) -> Screen<'_> {
        let log = self.app.build_log();
        let mut body = column![
            text(s.log_heading).size(18),
            text(log.status.clone().unwrap_or_default()),
        ]
        .spacing(8);
        let mut lines = column![].spacing(2);
        for line in &log.lines {
            lines =
                lines.push(text(format!("[{}] {}", stream_name(line.stream), line.text)).size(11));
        }
        if log.lines.is_empty() {
            lines = lines.push(text(s.empty).size(11));
        }
        body = body.push(scrollable(lines).height(Length::Fixed(260.0)));
        match self.app.apply() {
            ApplyState::Applied {
                snapshot_id,
                written,
                ..
            } => {
                body = body.push(text(fill(
                    s.applied,
                    &[snapshot_id.to_string(), written.len().to_string()],
                )));
            }
            ApplyState::Failed { message, .. } => body = body.push(text(message.clone())),
            ApplyState::Idle | ApplyState::Running => {}
        }
        let actions =
            row![button(text(s.history_button)).on_press(Message::Event(Event::HistoryRequested))];
        (body.into(), actions.into())
    }

    fn view_history(&self, s: &'static Strings) -> Screen<'_> {
        let Some(history) = self.app.history() else {
            return (text(s.loading).into(), row![].into());
        };
        let mut body = column![
            text(s.history_heading).size(18),
            text(history.read_only_reason()).size(12),
            text(fill(
                s.generations_heading,
                &[history.generations.len().to_string()],
            )),
        ]
        .spacing(6);
        if history.generations.is_empty() {
            body = body.push(text(s.no_generations).size(12));
        }
        for generation in &history.generations {
            body = body.push(text(format!(
                "· {:?} {} → {}",
                generation.id,
                generation.link.display(),
                generation
                    .target
                    .as_ref()
                    .map_or_else(|| "—".to_string(), |path| path.display().to_string())
            )));
        }
        body = body.push(text(fill(
            s.snapshots_heading,
            &[history.snapshots.len().to_string()],
        )));
        if history.snapshots.is_empty() {
            body = body.push(text(s.no_snapshots).size(12));
        }
        for snapshot in &history.snapshots {
            body = body.push(text(format!(
                "· {} · {} · {} file(s)",
                snapshot.id, snapshot.outcome, snapshot.files
            )));
        }
        (body.into(), row![].into())
    }
}

/// Fill `{}` placeholders in order. Positional rather than named, so the same
/// templates work for both languages and stay parallel with `i18n.js`.
fn fill(template: &str, values: &[String]) -> String {
    let mut out = String::new();
    let mut rest = template;
    for value in values {
        match rest.split_once("{}") {
            Some((head, tail)) => {
                out.push_str(head);
                out.push_str(value);
                rest = tail;
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// The steps in navigation order; the index matches `Strings::nav`.
const STEP_ORDER: [Step; 7] = [
    Step::Discover,
    Step::Catalog,
    Step::Plan,
    Step::Validation,
    Step::Confirm,
    Step::BuildLog,
    Step::History,
];

/// Run one event and whatever it asks for, feeding results back in.
fn drive(ui: &mut Ui, event: Event) {
    let mut response = ui.app.handle(event);
    for _ in 0..6 {
        let Some(intent) = response.intents.first().cloned() else {
            return;
        };
        let result = match intent {
            Intent::Discover => ui.session.discover(),
            Intent::Search {
                query,
                filters,
                page,
                per_page,
            } => ui.session.search(&query, &filters, page, per_page),
            Intent::BuildPlan { revision } => {
                let selection = Selection {
                    packages: ui.app.selected().to_vec(),
                    settings: ui.app.settings().to_vec(),
                };
                ui.session.plan(&selection, revision)
            }
            Intent::Validate { revision, attempt } => {
                let mut lines: Vec<(nhs_core::proc::Stream, String)> = Vec::new();
                let mut sink = |stream: nhs_core::proc::Stream, line: &str| {
                    lines.push((stream, line.to_string()));
                };
                let event = ui.session.validate(revision, attempt, &mut sink);
                for (stream, line) in lines {
                    let stream = match stream {
                        nhs_core::proc::Stream::Stdout => LogStream::Stdout,
                        nhs_core::proc::Stream::Stderr => LogStream::Stderr,
                    };
                    ui.app.handle(Event::LogLine { stream, text: line });
                }
                event
            }
            Intent::Apply { revision } => ui.session.apply(revision),
            Intent::LoadHistory => ui.session.history(),
        };
        response = ui.app.handle(result);
    }
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

fn stream_name(stream: LogStream) -> &'static str {
    match stream {
        LogStream::Stdout => "stdout",
        LogStream::Stderr => "stderr",
        LogStream::Status => "status",
    }
}

/// The same headless walk as the Tauri spike, for a like-for-like comparison.
fn run_selftest() {
    let (mut ui, _) = Ui::boot();
    let sequence: Vec<Event> = vec![
        Event::DiscoverRequested,
        Event::SearchSubmitted {
            query: "firefox".to_string(),
            filters: Filters::default(),
            page: 1,
            per_page: 20,
        },
        Event::PackageToggled("firefox".to_string()),
        Event::PlanRequested,
        Event::DiffReviewed,
        Event::ValidationRequested,
        Event::ConfirmationRequested,
        Event::ApplyConfirmed,
        Event::HistoryRequested,
    ];
    for event in sequence {
        drive(&mut ui, event.clone());
        println!(
            "{:<22} step={:<11} selected={} plan={} validation={} apply={}",
            format!("{event:?}").split('(').next().unwrap_or("?"),
            step_name(ui.app.step()),
            ui.app.selected().len(),
            ui.app.plan().map_or(0, |plan| plan.changes.len()),
            ui.app.validation().map_or_else(
                || "none".to_string(),
                |v| if v.report.success {
                    "ok".into()
                } else {
                    "failed".into()
                }
            ),
            match ui.app.apply() {
                ApplyState::Idle => "idle",
                ApplyState::Running => "running",
                ApplyState::Applied { .. } => "applied",
                ApplyState::Failed { .. } => "failed",
            },
        );
    }
    let reviewed = matches!(ui.app.review(), ReviewState::Acknowledged { .. });
    println!("reviewed at end: {reviewed}");
    if let Some(status) = ui.app.build_log().status.clone() {
        println!("last status: {status}");
    }
    // The language toggle must not disturb the flow: check it here, where the
    // headless walk can assert it without a window.
    let before = step_name(ui.app.step());
    ui.lang = ui.lang.toggled();
    assert_eq!(before, step_name(ui.app.step()));
    println!(
        "i18n: {} / {}",
        strings(Lang::En).app_title,
        strings(Lang::Zh).app_title
    );
}

fn main() -> iced::Result {
    if std::env::args().any(|arg| arg == "--selftest") {
        run_selftest();
        return Ok(());
    }
    iced::application(Ui::boot, Ui::update, Ui::view)
        .title("Nix Home Studio — prototype (iced)")
        .window_size((1180.0, 760.0))
        .run()
}
