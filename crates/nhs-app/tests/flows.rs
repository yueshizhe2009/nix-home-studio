//! M6.1 acceptance tests: the seven flows, as state transitions.
//!
//! Every test drives the real state machine and asserts two things: the state it
//! ended in, and the work it did (or refused to) hand to the adapter. Nothing
//! here builds a plan, runs a build or writes a file — that is the adapter's job,
//! and the last test proves this crate cannot do it even by accident.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::time::Duration;

use nhs_app::{
    strings, App, ApplyResult, ApplyState, Event, Intent, LogStream, PackageHit, PlanState,
    RefusalCode, ReviewState, SnapshotSummary, Step, MAX_LOG_LINES,
};
use nhs_core::catalog::Filters;
use nhs_core::classify::{Diagnosis, ErrorKind};
use nhs_core::discovery::{Discovery, GenerationLink, GenerationState, Layout};
use nhs_core::edit::{ChangeKind, FileChange, Plan};
use nhs_core::validation::ValidationReport;
use nhs_core::SettingValue;
use nhs_core::Sha256;

// -- fixtures ---------------------------------------------------------------

fn discovery(layout: Layout) -> Discovery {
    Discovery {
        layout,
        attempts: Vec::new(),
        generations: GenerationState {
            generations: Vec::new(),
            current: None,
        },
    }
}

fn flake_home() -> Discovery {
    discovery(Layout::FlakeHome {
        root: PathBuf::from("/home/tester/dotfiles"),
        main_file: PathBuf::from("/home/tester/dotfiles/home.nix"),
        attr: "tester".to_string(),
    })
}

fn classic_home() -> Discovery {
    discovery(Layout::ClassicHome {
        main_file: PathBuf::from("/home/tester/.config/home-manager/home.nix"),
    })
}

fn profile_only() -> Discovery {
    discovery(Layout::ProfileOnly {
        profile_dir: PathBuf::from("/home/tester/.local/state/nix/profiles"),
        generations: vec![GenerationLink {
            id: Some(3),
            link: PathBuf::from("/home/tester/.local/state/nix/profiles/home-manager-3-link"),
            target: Some(PathBuf::from("/nix/store/abc-home-manager-generation")),
        }],
    })
}

fn nixos() -> Discovery {
    discovery(Layout::FlakeNixos {
        root: PathBuf::from("/etc/nixos"),
        main_file: PathBuf::from("/etc/nixos/flake.nix"),
        host: "box".to_string(),
        user: "tester".to_string(),
    })
}

fn change(path: &str) -> FileChange {
    FileChange {
        path: PathBuf::from(path),
        kind: ChangeKind::Replace,
        expected_sha256: Some(Sha256::of_bytes(b"old")),
        old_text: "old".to_string(),
        new_text: "new".to_string(),
        unified_diff: "--- a\n+++ b\n-old\n+new\n".to_string(),
        added_lines: 1,
        removed_lines: 1,
        resolved_from: None,
    }
}

fn plan() -> Plan {
    Plan {
        changes: vec![
            change("/home/tester/.config/home-manager/home.nix"),
            change("/home/tester/.config/home-manager/modules/nhs-packages.nix"),
        ],
        notes: vec!["`boot.kernelPackages` is a NixOS-only option".to_string()],
    }
}

fn validation(success: bool) -> ValidationReport {
    ValidationReport {
        success,
        command: "home-manager build -f home.nix".to_string(),
        diagnosis: (!success).then(|| Diagnosis {
            kind: ErrorKind::MissingAttribute,
            summary: "attribute 'nope' missing".to_string(),
            hint: "check the name",
            location: Some("/home/tester/home.nix:3:1".to_string()),
        }),
        kept_scratch: (!success).then(|| PathBuf::from("/state/scratch/v1/root")),
        record: None,
        elapsed: Duration::from_millis(42),
    }
}

fn hit(attr: &str) -> PackageHit {
    PackageHit {
        attr: attr.to_string(),
        name: attr.to_string(),
        version: None,
        description: "a package".to_string(),
        category: Some("development".to_string()),
        license: Some("MIT".to_string()),
        platforms: vec!["linux".to_string()],
    }
}

/// Ask for a validation and return the attempt number the app assigned.
fn request_validation(app: &mut App) -> u64 {
    let response = app.handle(Event::ValidationRequested);
    match response.intents.first() {
        Some(Intent::Validate { attempt, .. }) => *attempt,
        other => panic!("expected a Validate intent, got {other:?}"),
    }
}

/// Report a validation result for the attempt the app asked for.
fn finish_validation(app: &mut App, attempt: u64, success: bool) {
    app.handle(Event::ValidationFinished {
        revision: app.revision(),
        attempt,
        report: validation(success),
    });
}

/// Drive the app to a plan that is built and validated successfully.
fn app_with_plan(success: bool) -> App {
    let mut app = App::new();
    app.handle(Event::Discovered(classic_home()));
    app.handle(Event::PackageToggled("firefox".to_string()));
    app.handle(Event::PlanRequested);
    app.handle(Event::PlanBuilt {
        revision: app.revision(),
        plan: plan(),
    });
    let attempt = request_validation(&mut app);
    finish_validation(&mut app, attempt, success);
    app
}

// -- 1. discover ------------------------------------------------------------

#[test]
fn the_initial_state_is_discover_and_nothing_is_editable_yet() {
    let mut app = App::new();
    assert_eq!(app.step(), Step::Discover);
    assert!(app.discovery().is_none());
    assert!(!app.editable(), "no discovery, so no editable layout");
    assert_eq!(app.apply_readiness(), Err(RefusalCode::LayoutNotEditable));

    let response = app.handle(Event::Discovered(classic_home()));
    assert!(
        response.intents.is_empty(),
        "discovery is a report, not work"
    );
    assert!(response.refusal.is_none());
    assert_eq!(app.step(), Step::Discover);
    assert!(app.editable());
    assert_eq!(
        app.config_root(),
        Some(PathBuf::from("/home/tester/.config/home-manager"))
    );

    // A flake layout is editable, and its root is the flake root.
    let mut app = App::new();
    app.handle(Event::Discovered(flake_home()));
    assert!(app.editable());
    assert_eq!(
        app.config_root(),
        Some(PathBuf::from("/home/tester/dotfiles"))
    );

    // A layout this application must not write is not editable.
    let mut app = App::new();
    app.handle(Event::Discovered(nixos()));
    assert!(!app.editable());
    assert_eq!(app.apply_readiness(), Err(RefusalCode::LayoutNotEditable));

    let mut app = App::new();
    app.handle(Event::Discovered(profile_only()));
    assert!(!app.editable());
    assert_eq!(
        app.discovery()
            .expect("discovery")
            .layout
            .generations()
            .len(),
        1,
        "generations are available even when nothing is editable"
    );
}

// -- 2. catalog -------------------------------------------------------------

#[test]
fn searching_only_produces_an_intent() {
    let mut app = App::new();
    app.handle(Event::Discovered(classic_home()));
    let response = app.handle(Event::SearchSubmitted {
        query: "fire".to_string(),
        filters: Filters::default(),
        page: 1,
        per_page: 20,
    });

    assert_eq!(response.intents.len(), 1);
    match &response.intents[0] {
        Intent::Search {
            query,
            page,
            per_page,
            ..
        } => {
            assert_eq!(query, "fire");
            assert_eq!((*page, *per_page), (1, 20));
        }
        other => panic!("expected Search, got {other:?}"),
    }
    assert!(response.refusal.is_none());
    assert_eq!(app.step(), Step::Catalog);
    assert!(!app.catalog().searched, "no results yet");

    app.handle(Event::SearchResults {
        hits: vec![hit("firefox")],
        total: 1,
        page: 1,
        pages: 1,
    });
    assert!(app.catalog().searched);
    assert_eq!(app.catalog().hits[0].attr, "firefox");
    assert_eq!(app.catalog().total, 1);
}

// -- 3. empty selection -----------------------------------------------------

#[test]
fn an_empty_selection_cannot_build_a_plan() {
    let mut app = App::new();
    app.handle(Event::Discovered(classic_home()));

    let response = app.handle(Event::PlanRequested);
    assert!(response.intents.is_empty());
    assert_eq!(
        response.refusal.map(|refusal| refusal.code),
        Some(RefusalCode::EmptySelection)
    );
    assert_eq!(app.plan_state(), &PlanState::None);
}

// -- 4. selection changes invalidate ---------------------------------------

#[test]
fn changing_the_selection_invalidates_the_plan_and_the_validation() {
    let mut app = app_with_plan(true);
    assert_eq!(app.plan_state(), &PlanState::Ready);
    assert!(app.validation().is_some());
    let before = app.revision();

    app.handle(Event::PackageToggled("ripgrep".to_string()));

    assert!(app.revision() > before, "the revision must move");
    assert!(app.plan().is_none(), "the old plan is gone");
    assert_eq!(app.plan_state(), &PlanState::None);
    assert!(app.validation().is_none(), "the old result is gone");
    assert_eq!(app.review(), ReviewState::NotReviewed);
    assert_eq!(app.apply_readiness(), Err(RefusalCode::NoPlan));

    // A result for the old revision that arrives late is ignored.
    let response = app.handle(Event::PlanBuilt {
        revision: before,
        plan: plan(),
    });
    assert!(response.intents.is_empty());
    assert!(app.plan().is_none(), "a stale plan must not be adopted");
    app.handle(Event::ValidationFinished {
        revision: before,
        attempt: 1,
        report: validation(true),
    });
    assert!(
        app.validation().is_none(),
        "a stale validation must not be adopted"
    );
}

// -- 5. no plan, no validation ---------------------------------------------

#[test]
fn a_plan_must_exist_and_be_finished_before_validation() {
    let mut app = App::new();
    app.handle(Event::Discovered(classic_home()));

    let response = app.handle(Event::ValidationRequested);
    assert!(response.intents.is_empty());
    assert_eq!(
        response.refusal.map(|refusal| refusal.code),
        Some(RefusalCode::NoPlan)
    );

    // A plan that is still being built is not validatable either.
    app.handle(Event::PackageToggled("firefox".to_string()));
    app.handle(Event::PlanRequested);
    let response = app.handle(Event::ValidationRequested);
    assert_eq!(
        response.refusal.map(|refusal| refusal.code),
        Some(RefusalCode::PlanNotReady)
    );
    assert!(response.intents.is_empty());
}

// -- 6. failed validation blocks apply -------------------------------------

#[test]
fn a_failed_validation_blocks_apply() {
    let mut app = app_with_plan(false);
    assert!(!app.validation().expect("validation").report.success);

    assert_eq!(
        app.apply_readiness(),
        Err(RefusalCode::ValidationFailed),
        "the failed build is the reason, not the diff"
    );

    let response = app.handle(Event::ConfirmationRequested);
    assert!(response.intents.is_empty());
    assert_eq!(
        response.refusal.map(|refusal| refusal.code),
        Some(RefusalCode::ValidationFailed)
    );
    assert_ne!(app.step(), Step::Confirm);
}

// -- 7. unreviewed diff blocks apply ---------------------------------------

#[test]
fn an_unreviewed_diff_blocks_apply() {
    let mut app = app_with_plan(true);
    assert_eq!(app.apply_readiness(), Err(RefusalCode::DiffNotReviewed));

    let response = app.handle(Event::ConfirmationRequested);
    assert_eq!(
        response.refusal.map(|refusal| refusal.code),
        Some(RefusalCode::DiffNotReviewed)
    );
    assert!(response.intents.is_empty());
    assert_ne!(app.step(), Step::Confirm);

    // Reviewing the diff is not the same as confirming the write.
    app.handle(Event::DiffReviewed);
    assert_eq!(app.apply_readiness(), Ok(()));
    let response = app.handle(Event::ConfirmationRequested);
    assert!(
        response.intents.is_empty(),
        "showing the warning is not applying"
    );
    assert_eq!(app.step(), Step::Confirm);
    assert_eq!(app.apply(), &ApplyState::Idle, "nothing has been written");
}

// -- 8. explicit confirmation ----------------------------------------------

#[test]
fn apply_requires_the_confirmation_screen() {
    let mut app = app_with_plan(true);
    app.handle(Event::DiffReviewed);

    // Pressing "apply" from any other screen is refused.
    let response = app.handle(Event::ApplyConfirmed);
    assert!(response.intents.is_empty());
    assert_eq!(
        response.refusal.map(|refusal| refusal.code),
        Some(RefusalCode::NotConfirmed)
    );
    assert_eq!(app.apply(), &ApplyState::Idle);

    // Navigation cannot jump to the confirmation screen either.
    let mut app = app_with_plan(true);
    app.handle(Event::DiffReviewed);
    let response = app.handle(Event::StepRequested(Step::Confirm));
    assert!(response.intents.is_empty());
    assert_eq!(
        app.step(),
        Step::Confirm,
        "the gate opened for a reviewed diff"
    );

    // And cancelling returns without applying.
    app.handle(Event::ConfirmationCancelled);
    assert_eq!(app.step(), Step::Validation);
    assert_eq!(app.apply(), &ApplyState::Idle);

    // The real path: review, confirm, apply.
    let mut app = app_with_plan(true);
    app.handle(Event::DiffReviewed);
    app.handle(Event::ConfirmationRequested);
    let response = app.handle(Event::ApplyConfirmed);
    assert_eq!(response.intents.len(), 1);
    match &response.intents[0] {
        Intent::Apply { revision } => assert_eq!(*revision, app.revision()),
        other => panic!("expected Apply, got {other:?}"),
    }
    assert_eq!(app.apply(), &ApplyState::Running);
    assert_eq!(app.step(), Step::BuildLog);
    assert_eq!(app.confirmed_for(), Some(app.revision()));
}

/// A validation that fails before the write must stop it, even on the
/// confirmation screen.
///
/// One request has exactly one outcome, so a *late second result for the same
/// request* is impossible by design (see `a_duplicate_validation_result_is_ignored`).
/// What can still happen is a fresh validation: the user re-validates from the
/// confirmation screen, it fails, and apply must be refused.
#[test]
fn a_failed_revalidation_after_confirmation_blocks_the_apply() {
    let mut app = app_with_plan(true);
    app.handle(Event::DiffReviewed);
    app.handle(Event::ConfirmationRequested);
    assert_eq!(app.step(), Step::Confirm);

    let attempt = request_validation(&mut app);
    finish_validation(&mut app, attempt, false);

    let response = app.handle(Event::ApplyConfirmed);
    assert!(response.intents.is_empty());
    assert_eq!(
        response.refusal.map(|refusal| refusal.code),
        Some(RefusalCode::ValidationFailed),
        "the fresh failure is the reason, not the screen"
    );
    assert_eq!(app.apply(), &ApplyState::Idle);
}

// -- 9. switch -------------------------------------------------------------

#[test]
fn switch_is_always_disabled_and_has_no_intent() {
    let mut app = app_with_plan(true);
    app.handle(Event::DiffReviewed);
    app.handle(Event::ConfirmationRequested);

    assert!(!app.switch_available());
    assert!(
        app.switch_reason().contains("not available"),
        "{}",
        app.switch_reason()
    );
    assert!(
        app.switch_reason().contains("switch"),
        "the reason must name what is missing: {}",
        app.switch_reason()
    );

    // No event produces an activation intent: the only Apply path is the one
    // above, and it is documented as build-only.
    for event in [
        Event::StepRequested(Step::BuildLog),
        Event::HistoryRequested,
        Event::DiffReviewed,
    ] {
        let response = app.handle(event);
        assert!(
            !response
                .intents
                .iter()
                .any(|intent| matches!(intent, Intent::Apply { .. })),
            "only ApplyConfirmed may request a write"
        );
    }
    assert!(strings::APPLY_IS_BUILD_ONLY.contains("does not activate"));
}

// -- 10. history -----------------------------------------------------------

#[test]
fn history_keeps_generations_and_snapshots_apart_and_read_only() {
    let mut app = App::new();
    app.handle(Event::Discovered(profile_only()));
    let response = app.handle(Event::HistoryRequested);
    assert_eq!(response.intents, vec![Intent::LoadHistory]);

    app.handle(Event::HistoryLoaded {
        generations: vec![nhs_app::GenerationSummary {
            id: Some(3),
            link: PathBuf::from("/home/tester/.local/state/nix/profiles/home-manager-3-link"),
            target: Some(PathBuf::from("/nix/store/abc-home-manager-generation")),
            current: true,
        }],
        snapshots: vec![SnapshotSummary {
            id: "1700000000-1-000001".to_string(),
            created_unix_secs: 1_700_000_000,
            outcome: "applied".to_string(),
            files: 2,
        }],
    });

    let history = app.history().expect("history");
    assert_eq!(history.generations.len(), 1);
    assert_eq!(history.snapshots.len(), 1);
    assert_eq!(history.generations[0].id, Some(3));
    assert_eq!(history.snapshots[0].files, 2);
    assert!(history.read_only());
    assert!(history.read_only_reason().contains("not available"));
    assert_eq!(app.step(), Step::History);

    // Nothing in the history flow can be turned into a write.
    let response = app.handle(Event::StepRequested(Step::History));
    assert!(response.intents.is_empty());
}

// -- 11. apply outcome -----------------------------------------------------

#[test]
fn apply_success_and_failure_land_in_different_states() {
    let mut app = app_with_plan(true);
    app.handle(Event::DiffReviewed);
    app.handle(Event::ConfirmationRequested);
    app.handle(Event::ApplyConfirmed);
    app.handle(Event::ApplyFinished {
        revision: app.revision(),
        result: ApplyResult::Applied {
            snapshot_id: "snap-1".to_string(),
            written: vec![PathBuf::from("/home/tester/.config/home-manager/home.nix")],
        },
    });
    match app.apply() {
        ApplyState::Applied {
            snapshot_id,
            written,
            ..
        } => {
            assert_eq!(snapshot_id, "snap-1");
            assert_eq!(written.len(), 1);
        }
        other => panic!("expected Applied, got {other:?}"),
    }
    assert_eq!(
        app.build_log().status.as_deref(),
        Some(strings::APPLY_SUCCEEDED)
    );

    let mut app = app_with_plan(true);
    app.handle(Event::DiffReviewed);
    app.handle(Event::ConfirmationRequested);
    app.handle(Event::ApplyConfirmed);
    app.handle(Event::ApplyFinished {
        revision: app.revision(),
        result: ApplyResult::Failed {
            message: "disk full".to_string(),
        },
    });
    match app.apply() {
        ApplyState::Failed { message, .. } => assert_eq!(message, "disk full"),
        other => panic!("expected Failed, got {other:?}"),
    }
    assert_eq!(
        app.build_log().status.as_deref(),
        Some(strings::APPLY_FAILED)
    );
}

/// The same plan must not be written twice by walking back to the confirmation
/// screen: plan, validation and review all still match, so only an explicit
/// already-applied check stops it.
#[test]
fn the_same_plan_cannot_be_applied_twice() {
    let mut app = app_with_plan(true);
    app.handle(Event::DiffReviewed);
    app.handle(Event::ConfirmationRequested);
    app.handle(Event::ApplyConfirmed);
    app.handle(Event::ApplyFinished {
        revision: app.revision(),
        result: ApplyResult::Applied {
            snapshot_id: "snap-1".to_string(),
            written: vec![PathBuf::from("/home/tester/.config/home-manager/home.nix")],
        },
    });

    let reopen = app.handle(Event::ConfirmationRequested);
    assert!(
        reopen.intents.is_empty(),
        "returning to confirm must not authorise another write"
    );
    assert_eq!(
        reopen.refusal.map(|refusal| refusal.code),
        Some(RefusalCode::AlreadyApplied)
    );

    let direct = app.handle(Event::StepRequested(Step::Confirm));
    assert!(direct.intents.is_empty());
    assert_eq!(
        direct.refusal.map(|refusal| refusal.code),
        Some(RefusalCode::AlreadyApplied)
    );

    // A new selection is a new revision and may be applied once it is planned,
    // validated and reviewed again.
    app.handle(Event::PackageToggled("ripgrep".to_string()));
    app.handle(Event::PlanRequested);
    app.handle(Event::PlanBuilt {
        revision: app.revision(),
        plan: plan(),
    });
    let attempt = request_validation(&mut app);
    finish_validation(&mut app, attempt, true);
    app.handle(Event::DiffReviewed);
    let again = app.handle(Event::ConfirmationRequested);
    assert!(
        again.refusal.is_none(),
        "a changed selection must be applyable again: {:?}",
        again.refusal
    );
}

/// A result that arrives after the selection changed is still recorded: the write
/// happened, and pretending otherwise would misreport the disk.
#[test]
fn a_late_apply_result_is_recorded_rather_than_dropped() {
    let mut app = app_with_plan(true);
    app.handle(Event::DiffReviewed);
    app.handle(Event::ConfirmationRequested);
    app.handle(Event::ApplyConfirmed);
    let applied_for = app.revision();

    app.handle(Event::PackageToggled("ripgrep".to_string()));
    app.handle(Event::ApplyFinished {
        revision: applied_for,
        result: ApplyResult::Applied {
            snapshot_id: "snap-late".to_string(),
            written: Vec::new(),
        },
    });
    match app.apply() {
        ApplyState::Applied {
            revision,
            snapshot_id,
            ..
        } => {
            assert_eq!(*revision, applied_for);
            assert_eq!(snapshot_id, "snap-late");
        }
        other => panic!("expected the write to be recorded, got {other:?}"),
    }
}

// -- 12. log lines are not state -------------------------------------------

#[test]
fn log_and_progress_lines_never_change_the_flow_state() {
    let mut app = app_with_plan(true);
    let before = (app.step(), app.apply().clone(), app.plan_state().clone());

    for text in [
        "applied snapshot 123",
        "switching configuration",
        "error: attribute 'x' missing",
        "rolling back",
    ] {
        app.handle(Event::LogLine {
            stream: LogStream::Stdout,
            text: text.to_string(),
        });
    }
    app.handle(Event::Progress("validating…".to_string()));

    assert_eq!(app.step(), before.0, "a log line is not a transition");
    assert_eq!(app.apply(), &before.1);
    assert_eq!(app.plan_state(), &before.2);
    assert_eq!(app.build_log().lines.len(), 4);
    assert_eq!(app.build_log().status.as_deref(), Some("validating…"));
    assert!(
        app.build_log()
            .lines
            .iter()
            .all(|line| line.text != "switching configuration" || line.stream == LogStream::Stdout),
        "log lines keep their stream"
    );
}

#[test]
fn the_build_log_is_bounded() {
    let mut app = App::new();
    for index in 0..(MAX_LOG_LINES + 25) {
        app.handle(Event::LogLine {
            stream: LogStream::Stderr,
            text: format!("line {index}"),
        });
    }
    let log = app.build_log();
    assert_eq!(log.lines.len(), MAX_LOG_LINES);
    assert_eq!(log.dropped, 25);
    assert_eq!(
        log.lines.last().expect("last").text,
        format!("line {}", MAX_LOG_LINES + 24)
    );
}

// -- settings are part of the selection ------------------------------------

#[test]
fn settings_are_part_of_the_selection_and_invalidate_it_too() {
    let mut app = App::new();
    app.handle(Event::Discovered(classic_home()));
    app.handle(Event::SettingAdded(nhs_core::Setting {
        path: "programs.git.enable".to_string(),
        value: SettingValue::Bool(true),
        scope: nhs_core::Scope::Home,
    }));
    assert_eq!(app.selected().len(), 0);
    assert_eq!(app.settings().len(), 1);

    // A setting alone is enough to plan.
    let response = app.handle(Event::PlanRequested);
    assert_eq!(response.intents.len(), 1);

    app.handle(Event::PlanBuilt {
        revision: app.revision(),
        plan: plan(),
    });
    let attempt = request_validation(&mut app);
    finish_validation(&mut app, attempt, true);
    assert!(app.validation().is_some());

    app.handle(Event::SettingRemoved("programs.git.enable".to_string()));
    assert!(
        app.plan().is_none(),
        "removing a setting invalidates the plan"
    );
    assert!(app.validation().is_none());
}

// -- the boundary this crate must not cross --------------------------------

/// The app layer must not be able to touch the world. This is the same kind of
/// source scan gate G3 uses for the core, applied to the crate that the GUI will
/// call.
#[test]
fn the_app_layer_has_no_filesystem_process_or_toolkit_dependency() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for entry in std::fs::read_dir(&src).expect("read src") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read source");
        checked += 1;
        for needle in [
            "std::fs",
            "std::process",
            "Command::new",
            "std::io::stdin",
            "tauri",
            "iced",
            "slint",
            "eframe",
            "gtk",
        ] {
            assert!(
                !text.contains(needle),
                "{} must not contain `{needle}`: the app layer is pure state",
                path.display()
            );
        }
    }
    assert!(checked >= 2, "expected the app's sources, found {checked}");

    let manifest =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect("read manifest");
    for toolkit in ["tauri", "iced", "slint", "eframe", "gtk"] {
        assert!(
            !manifest.contains(toolkit),
            "no UI toolkit may be a dependency of the app layer: {toolkit}"
        );
    }
}

// -- same selection, two plans in flight -----------------------------------

/// Two plans for the same selection are still two different plans.
///
/// Sequence from the review: request A, request B, B finishes and is validated
/// and reviewed, then A arrives late. A must not replace B, and B's validation
/// must not be able to authorise A.
#[test]
fn a_late_plan_for_the_same_selection_cannot_replace_the_reviewed_one() {
    let mut app = App::new();
    app.handle(Event::Discovered(classic_home()));
    app.handle(Event::PackageToggled("firefox".to_string()));

    // Plan A.
    let response = app.handle(Event::PlanRequested);
    let plan_a = match response.intents.first() {
        Some(Intent::BuildPlan { revision }) => *revision,
        other => panic!("expected BuildPlan, got {other:?}"),
    };
    // Plan B, same selection.
    let response = app.handle(Event::PlanRequested);
    let plan_b = match response.intents.first() {
        Some(Intent::BuildPlan { revision }) => *revision,
        other => panic!("expected BuildPlan, got {other:?}"),
    };
    assert_ne!(plan_a, plan_b, "two requests must have two identities");

    // B arrives first and goes through validation and review.
    app.handle(Event::PlanBuilt {
        revision: plan_b,
        plan: plan(),
    });
    let attempt = request_validation(&mut app);
    finish_validation(&mut app, attempt, true);
    app.handle(Event::DiffReviewed);
    assert_eq!(app.apply_readiness(), Ok(()));

    // A arrives last.
    app.handle(Event::PlanBuilt {
        revision: plan_a,
        plan: plan(),
    });

    // The plan is still B's, and the readiness still refers to it.
    assert_eq!(app.revision(), plan_b, "B is still the current plan");
    assert_eq!(app.plan_state(), &PlanState::Ready);
    assert_eq!(app.apply_readiness(), Ok(()));

    // And the intent that would be applied carries B's identity, not A's.
    app.handle(Event::ConfirmationRequested);
    let response = app.handle(Event::ApplyConfirmed);
    match response.intents.first() {
        Some(Intent::Apply { revision }) => assert_eq!(*revision, plan_b),
        other => panic!("expected Apply, got {other:?}"),
    }
}

/// A failed plan for a superseded request must not mark the current one failed.
#[test]
fn a_late_plan_failure_for_an_old_request_is_ignored() {
    let mut app = App::new();
    app.handle(Event::Discovered(classic_home()));
    app.handle(Event::PackageToggled("firefox".to_string()));
    let first = match app.handle(Event::PlanRequested).intents.first() {
        Some(Intent::BuildPlan { revision }) => *revision,
        other => panic!("expected BuildPlan, got {other:?}"),
    };
    let second = match app.handle(Event::PlanRequested).intents.first() {
        Some(Intent::BuildPlan { revision }) => *revision,
        other => panic!("expected BuildPlan, got {other:?}"),
    };
    app.handle(Event::PlanBuilt {
        revision: second,
        plan: plan(),
    });

    app.handle(Event::PlanFailed {
        revision: first,
        message: "hand-modified block".to_string(),
    });
    assert_eq!(
        app.plan_state(),
        &PlanState::Ready,
        "the old failure must not touch the current plan"
    );
    assert!(app.plan().is_some());
}

/// A second result for a request that already produced a plan is ignored.
#[test]
fn a_duplicate_plan_result_is_ignored() {
    let mut app = App::new();
    app.handle(Event::Discovered(classic_home()));
    app.handle(Event::PackageToggled("firefox".to_string()));
    let request = match app.handle(Event::PlanRequested).intents.first() {
        Some(Intent::BuildPlan { revision }) => *revision,
        other => panic!("expected BuildPlan, got {other:?}"),
    };
    let mut first = plan();
    first.notes = vec!["first".to_string()];
    app.handle(Event::PlanBuilt {
        revision: request,
        plan: first,
    });
    let mut second = plan();
    second.notes = vec!["second".to_string()];
    app.handle(Event::PlanBuilt {
        revision: request,
        plan: second,
    });
    assert_eq!(
        app.plan().expect("plan").notes,
        vec!["first".to_string()],
        "the first result for a request wins; a duplicate is a bug, not a replacement"
    );
}

/// Two validation runs for the same plan: the older result cannot overwrite the
/// newer one, in either direction.
#[test]
fn validation_results_are_ordered_by_attempt() {
    let mut app = App::new();
    app.handle(Event::Discovered(classic_home()));
    app.handle(Event::PackageToggled("firefox".to_string()));
    app.handle(Event::PlanRequested);
    app.handle(Event::PlanBuilt {
        revision: app.revision(),
        plan: plan(),
    });

    let first = request_validation(&mut app);
    let second = request_validation(&mut app);
    assert_ne!(first, second, "each run has its own number");

    // The newer run reports failure, then the older run reports success late.
    finish_validation(&mut app, second, false);
    finish_validation(&mut app, first, true);

    let validation = app.validation().expect("a result");
    assert!(
        !validation.report.success,
        "the late success of an older run must not overwrite the newer failure"
    );
    app.handle(Event::DiffReviewed);
    assert_eq!(
        app.apply_readiness(),
        Err(RefusalCode::ValidationFailed),
        "the current result is the failure, so apply stays blocked"
    );
}

/// A validation result with no request behind it is refused.
#[test]
fn a_validation_result_without_a_request_is_ignored() {
    let mut app = App::new();
    app.handle(Event::Discovered(classic_home()));
    app.handle(Event::PackageToggled("firefox".to_string()));
    app.handle(Event::PlanRequested);
    app.handle(Event::PlanBuilt {
        revision: app.revision(),
        plan: plan(),
    });

    app.handle(Event::ValidationFinished {
        revision: app.revision(),
        attempt: 0,
        report: validation(true),
    });
    assert!(
        app.validation().is_none(),
        "attempt 0 means nothing was asked for"
    );
    assert_eq!(app.apply_readiness(), Err(RefusalCode::NotValidated));
}

/// Re-discovery that finds a different layout invalidates a plan staged against
/// the old one; a refresh of the same layout keeps it.
#[test]
fn rediscovery_invalidates_a_plan_only_when_the_layout_moved() {
    let mut app = app_with_plan(true);
    app.handle(Event::DiffReviewed);
    let revision = app.revision();

    // Same layout again: a refresh, nothing to invalidate.
    app.handle(Event::Discovered(classic_home()));
    assert_eq!(
        app.revision(),
        revision,
        "an identical layout is not a change"
    );
    assert!(app.plan().is_some());
    assert!(app.validation().is_some());

    // A different layout: the plan was staged against other paths.
    app.handle(Event::Discovered(flake_home()));
    assert!(app.plan().is_none(), "the plan belonged to the old tree");
    assert!(app.validation().is_none());
    assert_eq!(app.review(), ReviewState::NotReviewed);
    assert_eq!(app.apply_readiness(), Err(RefusalCode::NoPlan));
}

// -- layout identity and duplicate results (review round 3) ------------------

/// A plan that is still being built must not survive a layout change.
///
/// The failure this guards: `Planning` has no `plan` value yet, so an
/// "invalidate only if something exists" rule would let the old layout's result
/// arrive after the switch and be adopted.
#[test]
fn a_pending_plan_is_invalidated_by_a_layout_change() {
    let mut app = App::new();
    app.handle(Event::Discovered(classic_home()));
    app.handle(Event::PackageToggled("firefox".to_string()));
    let stale = match app.handle(Event::PlanRequested).intents.first() {
        Some(Intent::BuildPlan { revision }) => *revision,
        other => panic!("expected BuildPlan, got {other:?}"),
    };
    assert_eq!(app.plan_state(), &PlanState::Pending);

    // The user re-discovers and gets a different layout while the plan is in
    // flight.
    app.handle(Event::Discovered(flake_home()));
    assert_ne!(app.revision(), stale, "the pending request must be voided");

    app.handle(Event::PlanBuilt {
        revision: stale,
        plan: plan(),
    });
    assert!(
        app.plan().is_none(),
        "a plan built against the old layout must not be adopted"
    );
    assert_eq!(app.plan_state(), &PlanState::None);
    assert_eq!(app.apply_readiness(), Err(RefusalCode::NoPlan));
}

/// The flake attribute decides which `homeConfigurations.<attr>` a plan targets,
/// so a re-discovery that changes only the attribute invalidates the plan.
#[test]
fn a_different_flake_attribute_invalidates_the_plan() {
    let mut app = App::new();
    app.handle(Event::Discovered(flake_home()));
    app.handle(Event::PackageToggled("firefox".to_string()));
    app.handle(Event::PlanRequested);
    app.handle(Event::PlanBuilt {
        revision: app.revision(),
        plan: plan(),
    });
    let attempt = request_validation(&mut app);
    finish_validation(&mut app, attempt, true);
    app.handle(Event::DiffReviewed);
    assert_eq!(app.apply_readiness(), Ok(()));

    // Same root, same main file, different attribute: another target.
    app.handle(Event::Discovered(discovery(Layout::FlakeHome {
        root: PathBuf::from("/home/tester/dotfiles"),
        main_file: PathBuf::from("/home/tester/dotfiles/home.nix"),
        attr: "someone-else".to_string(),
    })));

    assert!(app.plan().is_none(), "the plan targeted another attribute");
    assert!(app.validation().is_none());
    assert_eq!(app.apply_readiness(), Err(RefusalCode::NoPlan));
}

/// One validation request produces exactly one outcome: a second result for the
/// same request is a duplicate and must not overwrite the first.
#[test]
fn a_duplicate_validation_result_is_ignored() {
    let mut app = App::new();
    app.handle(Event::Discovered(classic_home()));
    app.handle(Event::PackageToggled("firefox".to_string()));
    app.handle(Event::PlanRequested);
    app.handle(Event::PlanBuilt {
        revision: app.revision(),
        plan: plan(),
    });

    let attempt = request_validation(&mut app);
    finish_validation(&mut app, attempt, false);
    assert!(!app.validation().expect("validation").report.success);

    // The same request reports success afterwards. That must not unlock apply.
    finish_validation(&mut app, attempt, true);
    let validation = app.validation().expect("validation");
    assert!(
        !validation.report.success,
        "the first outcome of a request is its outcome"
    );
    app.handle(Event::DiffReviewed);
    assert_eq!(app.apply_readiness(), Err(RefusalCode::ValidationFailed));

    // A *new* request is allowed to report, and its result counts.
    let next = request_validation(&mut app);
    assert_ne!(next, attempt);
    finish_validation(&mut app, next, true);
    assert!(app.validation().expect("validation").report.success);
    app.handle(Event::DiffReviewed);
    assert_eq!(app.apply_readiness(), Ok(()));
}

/// Reviewing before validating must work: a new validation voids the old
/// *result*, not the user's acknowledgement of the diff.
#[test]
fn reviewing_before_validating_survives_the_validation() {
    let mut app = App::new();
    app.handle(Event::Discovered(classic_home()));
    app.handle(Event::PackageToggled("firefox".to_string()));
    app.handle(Event::PlanRequested);
    app.handle(Event::PlanBuilt {
        revision: app.revision(),
        plan: plan(),
    });

    // The order a person actually uses.
    app.handle(Event::DiffReviewed);
    let attempt = request_validation(&mut app);
    assert_eq!(
        app.review(),
        ReviewState::Acknowledged {
            revision: app.revision()
        },
        "validating must not discard the review"
    );
    finish_validation(&mut app, attempt, true);

    app.handle(Event::ConfirmationRequested);
    assert_eq!(app.step(), Step::Confirm);
    let response = app.handle(Event::ApplyConfirmed);
    assert_eq!(response.intents.len(), 1, "the write is requested");

    // A *new plan* still clears everything, review included.
    let mut app = app_with_plan(true);
    app.handle(Event::DiffReviewed);
    app.handle(Event::PackageToggled("ripgrep".to_string()));
    assert_eq!(app.review(), ReviewState::NotReviewed);
}

/// Asking for detection produces exactly one intent, and the answer lands.
#[test]
fn discovery_can_be_requested_and_reported() {
    let mut app = App::new();
    let response = app.handle(Event::DiscoverRequested);
    assert_eq!(response.intents, vec![Intent::Discover]);
    assert!(response.refusal.is_none());
    assert!(app.discovery().is_none(), "asking is not the answer");

    app.handle(Event::Discovered(classic_home()));
    assert!(app.editable());
    assert_eq!(app.step(), Step::Discover);

    // Asking again is allowed: re-detection after the user changed something.
    let response = app.handle(Event::DiscoverRequested);
    assert_eq!(response.intents, vec![Intent::Discover]);

    // A failed detection is reported, not swallowed.
    app.handle(Event::DiscoveryFailed("permission denied".to_string()));
    assert_eq!(app.discovery_error(), Some("permission denied"));
}

/// Re-detecting after a plan exists must go through the same invalidation gate.
#[test]
fn a_re_detection_that_moves_the_layout_voids_the_plan() {
    let mut app = app_with_plan(true);
    app.handle(Event::DiffReviewed);
    assert_eq!(app.apply_readiness(), Ok(()));

    // Same layout: the plan survives a refresh.
    let _ = app.handle(Event::DiscoverRequested);
    app.handle(Event::Discovered(classic_home()));
    assert!(app.plan().is_some());

    // Different layout: it does not.
    let _ = app.handle(Event::DiscoverRequested);
    app.handle(Event::Discovered(flake_home()));
    assert!(app.plan().is_none());
    assert_eq!(app.apply_readiness(), Err(RefusalCode::NoPlan));
}
