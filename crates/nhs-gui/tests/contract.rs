//! The JSON contract and the read-only gate, tested without a window.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use nhs_app::{App, Event, Step};
use nhs_core::EnvView;
use nhs_gui::bridge::{event_from_json, state_of, STATE_VERSION};
use nhs_gui::service::Service;
use serde_json::{json, Value};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Result<Self, Box<dyn Error>> {
        let unique = NEXT_DIR.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "nhs-gui-test-{name}-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A scratch HOME with a classic Home Manager layout and an empty store.
fn scratch() -> Result<(TempDir, Service, PathBuf), Box<dyn Error>> {
    let dir = TempDir::new("service")?;
    let home = dir.path().join("home");
    let config = home.join(".config/home-manager");
    fs::create_dir_all(&config)?;
    fs::write(
        config.join("home.nix"),
        "{ pkgs, ... }:\n{\n  imports = [ ];\n  home.stateVersion = \"24.05\";\n}\n",
    )?;
    let state = dir.path().join("state");
    let env = EnvView {
        home,
        user: Some("tester".to_string()),
        path_entries: Vec::new(),
        xdg_config_home: None,
        xdg_state_home: None,
        store_root: dir.path().join("store"),
    };
    Ok((
        dir,
        Service::new(nhs_adapter::Session::with_env(env, state.clone())),
        state,
    ))
}

#[test]
fn state_has_a_version_and_no_capabilities_yet() -> Result<(), Box<dyn Error>> {
    let state = state_of(&App::new());
    let value = serde_json::to_value(&state)?;
    let field = |path: &[&str]| -> Value {
        let mut current = &value;
        for key in path {
            current = &current[key];
        }
        current.clone()
    };
    assert_eq!(field(&["version"]), json!(STATE_VERSION));
    assert_eq!(field(&["app", "name"]), json!("Nix Home Studio"));
    assert_eq!(field(&["app", "identifier"]), json!("dev.local.nhs.studio"));
    assert_eq!(field(&["step"]), json!("discover"));
    assert_eq!(field(&["discovery"]), Value::Null);
    for capability in ["catalog", "plan", "validate", "apply"] {
        assert_eq!(
            field(&["capabilities", capability]),
            json!(true),
            "{capability} is implemented by the backend and must be reported"
        );
    }
    assert_eq!(field(&["switch", "available"]), json!(false));
    assert_eq!(field(&["busy"]), Value::Null);
    assert_eq!(field(&["apply", "state"]), json!("idle"));
    assert_eq!(field(&["log", "lines"]), json!([]));
    assert_eq!(
        field(&["switch", "reason_code"]),
        json!("switch_disabled"),
        "the UI translates by code, so the code must be stable"
    );
    Ok(())
}

#[test]
fn known_events_parse_and_unknown_ones_do_not() -> Result<(), Box<dyn Error>> {
    assert!(matches!(
        event_from_json(&json!({ "kind": "discover" })),
        Ok(Event::DiscoverRequested)
    ));
    assert!(matches!(
        event_from_json(&json!({ "kind": "step", "step": "discover" })),
        Ok(Event::StepRequested(Step::Discover))
    ));
    assert!(event_from_json(&json!({ "kind": "nope" })).is_err());
    assert!(event_from_json(&json!({})).is_err());
    assert!(event_from_json(&json!({ "kind": "toggle" })).is_err());

    let error = event_from_json(&json!({ "kind": "nope" }))
        .err()
        .ok_or("an unknown kind must be rejected")?;
    assert_eq!(error.code, "bad_event");
    Ok(())
}

#[test]
fn discovery_reads_the_layout_and_writes_nothing() -> Result<(), Box<dyn Error>> {
    let (_dir, service, state_dir) = scratch()?;
    let state = service.dispatch(Event::DiscoverRequested)?;
    let found = state
        .discovery
        .as_ref()
        .ok_or("discovery must report the layout")?;
    assert_eq!(found.kind, "classic-home");
    assert!(found.editable);
    assert_eq!(found.generations, 0);
    assert!(found.main_file.is_some());
    assert!(
        !state_dir.exists(),
        "discovery must not create the state directory"
    );
    Ok(())
}

#[test]
fn out_of_order_actions_do_nothing() -> Result<(), Box<dyn Error>> {
    let (_dir, service, state_dir) = scratch()?;
    // Applying before any plan, validation or review must not reach an intent.
    let error = service
        .dispatch(Event::ApplyConfirmed)
        .err()
        .ok_or("an out-of-order apply must be refused")?;
    assert_eq!(error.code, "refused");
    assert_eq!(
        error.detail.as_deref(),
        Some("layout_not_editable"),
        "without discovery the layout is not editable, and that is the first gate"
    );
    let state = service.snapshot()?;
    assert_eq!(state.apply.state, "idle");
    assert!(state.busy.is_none());
    assert!(state.plan.is_none());
    assert!(state.validation.is_none());
    assert!(!state_dir.exists(), "nothing may be written");
    Ok(())
}

#[test]
fn apply_is_not_advertised_as_cancellable() {
    assert!(nhs_gui::service::job_is_cancellable("validation"));
    assert!(
        !nhs_gui::service::job_is_cancellable("apply"),
        "apply does not read the token, so it must not claim to be cancellable"
    );
}

#[test]
fn cancel_does_not_pretend_to_succeed() -> Result<(), Box<dyn Error>> {
    let (_dir, service, _state) = scratch()?;
    let error = service.cancel().err().ok_or("cancel is not implemented")?;
    assert_eq!(error.code, "cancel_unavailable");
    assert!(
        error.detail.is_none(),
        "nothing was cancelled, so there is no detail"
    );
    Ok(())
}

#[test]
fn search_select_plan_and_review() -> Result<(), Box<dyn Error>> {
    let (_dir, service, _state) = scratch()?;
    service.dispatch(Event::DiscoverRequested)?;

    let state = service.dispatch(Event::SearchSubmitted {
        query: "firefox".to_string(),
        filters: nhs_core::catalog::Filters::default(),
        page: 1,
        per_page: 20,
    })?;
    assert!(state.catalog.searched);
    assert!(
        state.catalog.hits.iter().any(|hit| hit.attr == "firefox"),
        "the curated catalogue must contain firefox"
    );
    assert_eq!(state.selected, 0);

    let state = service.dispatch(Event::PackageToggled("firefox".to_string()))?;
    assert_eq!(state.selected, 1);
    assert!(state
        .catalog
        .hits
        .iter()
        .any(|hit| hit.attr == "firefox" && hit.selected));

    let state = service.dispatch(Event::PlanRequested)?;
    let plan = state.plan.ok_or("plan must be built")?;
    assert_eq!(plan.state, "ready");
    assert!(!plan.files.is_empty(), "the plan must list files");
    assert!(
        plan.files.iter().any(|file| !file.diff.is_empty()),
        "each change carries its unified diff"
    );
    assert!(!state.reviewed);

    let state = service.dispatch(Event::DiffReviewed)?;
    assert!(state.reviewed, "review is recorded");
    Ok(())
}

#[test]
fn a_search_without_matches_is_not_an_error() -> Result<(), Box<dyn Error>> {
    let (_dir, service, _state) = scratch()?;
    service.dispatch(Event::DiscoverRequested)?;
    let state = service.dispatch(Event::SearchSubmitted {
        query: "definitely-not-a-package".to_string(),
        filters: nhs_core::catalog::Filters::default(),
        page: 1,
        per_page: 20,
    })?;
    assert!(state.catalog.searched);
    assert!(state.catalog.hits.is_empty());
    Ok(())
}
