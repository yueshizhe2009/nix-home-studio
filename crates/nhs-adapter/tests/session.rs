//! M6.2: the adapter the two prototype UIs share.
//!
//! These tests use a real temporary tree (the adapter is deliberately bound to
//! `RealFs` — a front end talks to the machine), but never a real process: the
//! `validate` step is the only one that spawns, and it is exercised by the core's
//! own tests. What is proved here is that the adapter wires the flows to core
//! correctly and reports results as the events the app expects.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::fs;
use std::path::{Path, PathBuf};

use nhs_adapter::Session;
use nhs_app::{App, ApplyResult, ApplyState, Event, Intent, RefusalCode, Step};
use nhs_core::catalog::Filters;
use nhs_core::flow::Selection;
use nhs_core::SettingValue;

struct Fixture {
    home: PathBuf,
    state: PathBuf,
    #[allow(dead_code)]
    dir: tempfile::TempDir,
}

impl Fixture {
    fn classic() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let config = home.join(".config").join("home-manager");
        fs::create_dir_all(&config).expect("config dir");
        fs::write(
            config.join("home.nix"),
            "{ pkgs, ... }:\n{\n  imports = [ ];\n}\n",
        )
        .expect("home.nix");
        let state = dir.path().join("state");
        fs::create_dir_all(&state).expect("state");
        Self { home, state, dir }
    }

    fn profile_only() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let profiles = home.join(".local/state/nix/profiles");
        fs::create_dir_all(&profiles).expect("profiles");
        // A generation link is what makes the layout `ProfileOnly` rather than
        // `Unknown`.
        let generation = dir.path().join("store/hm-gen3-home-manager-generation");
        fs::create_dir_all(&generation).expect("generation");
        fs::write(generation.join("activate"), "#!/bin/sh\n").expect("activate");
        std::os::unix::fs::symlink(&generation, profiles.join("home-manager"))
            .expect("profile link");
        std::os::unix::fs::symlink(&generation, profiles.join("home-manager-3-link"))
            .expect("numbered link");
        let state = dir.path().join("state");
        fs::create_dir_all(&state).expect("state");
        Self { home, state, dir }
    }

    fn session(&self) -> Session {
        // Build the environment explicitly. `Session::new` reads the process
        // environment, and a runner (or a developer) with `XDG_CONFIG_HOME` set
        // would make discovery look outside the fixture — the failure CI hit.
        let mut env = nhs_core::EnvView::from_process();
        env.home = self.home.clone();
        env.xdg_config_home = None;
        env.xdg_state_home = None;
        env.store_root = self.dir.path().join("store");
        Session::with_env(env, self.state.clone())
    }

    fn config(&self) -> PathBuf {
        self.home.join(".config").join("home-manager")
    }
}

/// A fake `home-manager` that succeeds, on a private `PATH`.
fn fake_toolchain(directory: &Path) -> PathBuf {
    let bin = directory.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let path = bin.join("home-manager");
    fs::write(&path, "#!/bin/sh\nprintf 'build ok\\n'\nexit 0\n").expect("fake");
    let mut permissions = fs::metadata(&path).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    fs::set_permissions(&path, permissions).expect("chmod");
    bin
}

/// A session whose `PATH` holds only the fake toolchain.
fn session_with_fake(fixture: &Fixture) -> Session {
    let bin = fake_toolchain(fixture.dir.path());
    let env = nhs_core::EnvView {
        home: fixture.home.clone(),
        user: Some("tester".to_string()),
        path_entries: vec![bin],
        xdg_config_home: None,
        xdg_state_home: None,
        store_root: fixture.dir.path().join("store"),
    };
    Session::with_env(env, fixture.state.clone())
}

/// Run the validate step the way a front end would: intent → adapter → event.
fn validate(app: &mut App, session: &Session) -> nhs_app::ApplyState {
    let response = app.handle(Event::ValidationRequested);
    let (revision, attempt) = match response.intents.first() {
        Some(Intent::Validate { revision, attempt }) => (*revision, *attempt),
        other => panic!("expected Validate, got {other:?}"),
    };
    let mut sink = |_: nhs_core::proc::Stream, _: &str| {};
    app.handle(session.validate(revision, attempt, &mut sink));
    app.apply().clone()
}

fn selection(packages: &[&str]) -> Selection {
    Selection {
        packages: packages.iter().map(|name| (*name).to_string()).collect(),
        settings: Vec::new(),
    }
}

#[test]
fn discover_reports_the_layout_and_the_state_directory() {
    let fixture = Fixture::classic();
    let session = fixture.session();
    assert_eq!(session.state_dir(), fixture.state);

    match session.discover() {
        Event::Discovered(discovery) => {
            assert_eq!(discovery.layout.kind(), "classic-home");
            assert_eq!(
                discovery.layout.main_file(),
                Some(fixture.config().join("home.nix").as_path())
            );
        }
        other => panic!("expected Discovered, got {other:?}"),
    }

    // History is empty on a fresh state directory, and says so rather than failing.
    match session.history() {
        Event::HistoryLoaded {
            generations,
            snapshots,
        } => {
            assert!(
                generations.is_empty(),
                "this fixture has no profile, so there is no generation to list"
            );
            assert!(snapshots.is_empty(), "nothing was applied yet");
        }
        other => panic!("expected HistoryLoaded, got {other:?}"),
    }
}

#[test]
fn search_returns_curated_hits_without_touching_the_disk() {
    let fixture = Fixture::classic();
    let session = fixture.session();
    match session.search("firefox", &Filters::default(), 1, 10) {
        Event::SearchResults { hits, total, .. } => {
            assert!(total >= 1);
            assert_eq!(hits[0].attr, "firefox");
            assert!(hits[0].license.is_some(), "curated entries carry metadata");
        }
        other => panic!("expected SearchResults, got {other:?}"),
    }
}

/// plan → apply, with the events flowing through `nhs-app` exactly as a UI would.
#[test]
fn plan_then_apply_writes_the_two_files_and_records_a_snapshot() {
    let fixture = Fixture::classic();
    let session = session_with_fake(&fixture);
    let mut app = App::new();

    app.handle(session.discover());
    app.handle(Event::PackageToggled("firefox".to_string()));

    // The app decides to plan and hands back an intent.
    let response = app.handle(Event::PlanRequested);
    let revision = match response.intents.first() {
        Some(Intent::BuildPlan { revision }) => *revision,
        other => panic!("expected BuildPlan, got {other:?}"),
    };
    let event = session.plan(&selection(&["firefox"]), revision);
    match &event {
        Event::PlanBuilt { plan, .. } => assert_eq!(plan.changes.len(), 2),
        other => panic!("expected PlanBuilt, got {other:?}"),
    }
    app.handle(event);
    validate(&mut app, &session);
    assert!(
        app.validation().expect("validation").report.success,
        "the fake build succeeds"
    );

    // Review, confirm, apply.
    app.handle(Event::DiffReviewed);
    app.handle(Event::ConfirmationRequested);
    assert_eq!(app.step(), Step::Confirm);
    let response = app.handle(Event::ApplyConfirmed);
    let revision = match response.intents.first() {
        Some(Intent::Apply { revision }) => *revision,
        other => panic!("expected Apply, got {other:?}"),
    };
    let event = session.apply(revision);
    match &event {
        Event::ApplyFinished {
            result: ApplyResult::Applied { written, .. },
            ..
        } => assert_eq!(written.len(), 2),
        other => panic!("expected Applied, got {other:?}"),
    }
    app.handle(event);
    assert!(matches!(app.apply(), ApplyState::Applied { .. }));

    // The files really are on disk, in the temporary tree.
    let config = fixture.config();
    let module = fs::read_to_string(config.join("modules/nhs-packages.nix")).expect("module");
    assert!(module.contains("pkgs.firefox"), "{module}");
    let main = fs::read_to_string(config.join("home.nix")).expect("home.nix");
    assert!(
        main.contains("imports = [ ./modules/nhs-packages.nix ]"),
        "{main}"
    );

    // And the snapshot shows up in history, applied.
    match session.history() {
        Event::HistoryLoaded { snapshots, .. } => {
            assert_eq!(snapshots.len(), 1);
            assert_eq!(snapshots[0].outcome, "applied");
            assert_eq!(snapshots[0].files, 2);
        }
        other => panic!("expected HistoryLoaded, got {other:?}"),
    }
}

/// A second plan for the same selection is a no-op: the files already match.
#[test]
fn a_second_plan_after_apply_has_nothing_to_do() {
    let fixture = Fixture::classic();
    let session = session_with_fake(&fixture);
    let mut app = App::new();
    app.handle(session.discover());
    app.handle(Event::PackageToggled("firefox".to_string()));

    let revision = match app.handle(Event::PlanRequested).intents.first() {
        Some(Intent::BuildPlan { revision }) => *revision,
        other => panic!("expected BuildPlan, got {other:?}"),
    };
    app.handle(session.plan(&selection(&["firefox"]), revision));
    validate(&mut app, &session);
    app.handle(Event::DiffReviewed);
    app.handle(Event::ConfirmationRequested);
    let revision = match app.handle(Event::ApplyConfirmed).intents.first() {
        Some(Intent::Apply { revision }) => *revision,
        other => panic!("expected Apply, got {other:?}"),
    };
    app.handle(session.apply(revision));

    // Same selection again.
    app.handle(Event::PackageToggled("firefox".to_string()));
    app.handle(Event::PackageToggled("firefox".to_string()));
    let revision = match app.handle(Event::PlanRequested).intents.first() {
        Some(Intent::BuildPlan { revision }) => *revision,
        other => panic!("expected BuildPlan, got {other:?}"),
    };
    let event = session.plan(&selection(&["firefox"]), revision);
    match &event {
        Event::PlanBuilt { plan, .. } => assert!(plan.is_empty(), "nothing would change"),
        other => panic!("expected PlanBuilt, got {other:?}"),
    }
}

#[test]
fn a_layout_with_nothing_to_edit_fails_with_a_reason() {
    let fixture = Fixture::profile_only();
    let session = fixture.session();
    match session.plan(&selection(&["firefox"]), 1) {
        Event::PlanFailed { revision, message } => {
            assert_eq!(revision, 1);
            assert!(message.contains("profile-only"), "{message}");
        }
        other => panic!("expected PlanFailed, got {other:?}"),
    }

    // The app therefore never reaches a state where apply is possible.
    let mut app = App::new();
    app.handle(session.discover());
    app.handle(Event::PackageToggled("firefox".to_string()));
    assert_eq!(app.apply_readiness(), Err(RefusalCode::LayoutNotEditable));
}

/// Settings flow through the same path as packages.
#[test]
fn settings_reach_the_plan() {
    let fixture = Fixture::classic();
    let session = fixture.session();
    let selection = Selection {
        packages: Vec::new(),
        settings: vec![nhs_core::Setting {
            path: "programs.git.enable".to_string(),
            value: SettingValue::Bool(true),
            scope: nhs_core::Scope::Home,
        }],
    };
    match session.plan(&selection, 7) {
        Event::PlanBuilt { plan, .. } => {
            let module = plan
                .changes
                .iter()
                .find(|change| change.path.ends_with("nhs-packages.nix"))
                .expect("module change");
            assert!(
                module.new_text.contains("programs.git.enable = true;"),
                "{}",
                module.new_text
            );
        }
        other => panic!("expected PlanBuilt, got {other:?}"),
    }
}

/// The audit host resolves `home-manager` from the store and has none on PATH.
/// Validation must use the resolved binary, not re-resolve it through PATH.
#[test]
fn validation_uses_the_store_toolchain_when_path_is_empty() {
    let fixture = Fixture::classic();
    let bin = fixture.dir.path().join("store/abc-home-manager-1.0/bin");
    fs::create_dir_all(&bin).expect("store bin");
    let tool = bin.join("home-manager");
    fs::write(&tool, "#!/bin/sh\nexit 0\n").expect("fake store tool");
    let mut permissions = fs::metadata(&tool).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    fs::set_permissions(&tool, permissions).expect("chmod");

    let env = nhs_core::EnvView {
        home: fixture.home.clone(),
        user: Some("tester".to_string()),
        path_entries: Vec::new(),
        xdg_config_home: None,
        xdg_state_home: None,
        store_root: fixture.dir.path().join("store"),
    };
    let session = Session::with_env(env, fixture.state.clone());

    let mut app = App::new();
    app.handle(session.discover());
    app.handle(Event::PackageToggled("firefox".to_string()));
    let plan_request = app.handle(Event::PlanRequested);
    let revision = match plan_request.intents.first() {
        Some(Intent::BuildPlan { revision }) => *revision,
        other => panic!("expected BuildPlan, got {other:?}"),
    };
    app.handle(session.plan(&selection(&["firefox"]), revision));
    app.handle(Event::DiffReviewed);
    let validate_request = app.handle(Event::ValidationRequested);
    let (revision, attempt) = match validate_request.intents.first() {
        Some(Intent::Validate { revision, attempt }) => (*revision, *attempt),
        other => panic!("expected Validate, got {other:?}"),
    };

    let mut sink = |_: nhs_core::proc::Stream, _: &str| {};
    match session.validate(revision, attempt, &mut sink) {
        Event::ValidationFinished { report, .. } => {
            assert!(report.success, "the store toolchain must be used");
        }
        other => panic!("validation did not run: {other:?}"),
    }
}

/// Generations belong to the user's profile, not to the layout: a classic
/// configuration still lists them, with the profile target marked current.
#[test]
fn history_lists_generations_for_a_classic_layout() {
    let fixture = Fixture::profile_only();
    let mut home = fixture.home.clone();
    // Give the same fixture a classic configuration next to the profile.
    let config = home.join(".config/home-manager");
    fs::create_dir_all(&config).expect("config");
    fs::write(config.join("home.nix"), "{ pkgs, ... }:\n{ }\n").expect("home.nix");
    home = fixture.home.clone();
    let _ = home;

    let session = fixture.session();
    match session.history() {
        Event::HistoryLoaded { generations, .. } => {
            assert_eq!(generations.len(), 1, "the generation is listed");
            assert_eq!(generations.first().and_then(|g| g.id), Some(3));
            assert!(
                generations.first().is_some_and(|g| g.current),
                "the profile target is marked current"
            );
        }
        other => panic!("expected HistoryLoaded, got {other:?}"),
    }
}

/// A flake layout still shares the user's profile, so its generations are listed
/// the same way a classic layout's are.
#[test]
fn history_lists_generations_for_a_flake_layout() {
    let fixture = Fixture::profile_only();
    let config = fixture.home.join(".config/home-manager");
    fs::create_dir_all(&config).expect("config");
    fs::write(
        config.join("flake.nix"),
        "{\n  outputs = { self, nixpkgs, home-manager }: {\n    homeConfigurations.tester = home-manager.lib.homeManagerConfiguration { };\n  };\n}\n",
    )
    .expect("flake");
    let env = nhs_core::EnvView {
        home: fixture.home.clone(),
        user: Some("tester".to_string()),
        path_entries: Vec::new(),
        xdg_config_home: None,
        xdg_state_home: None,
        store_root: fixture.dir.path().join("store"),
    };
    let session = Session::with_env(env, fixture.state.clone());

    match session.discover() {
        Event::Discovered(discovery) => {
            assert_eq!(discovery.layout.kind(), "flake-home");
        }
        other => panic!("expected Discovered, got {other:?}"),
    }

    match session.history() {
        Event::HistoryLoaded { generations, .. } => {
            assert_eq!(generations.len(), 1);
            assert!(generations.first().is_some_and(|g| g.current));
        }
        other => panic!("expected HistoryLoaded, got {other:?}"),
    }
}
