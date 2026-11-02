//! Headless full-flow check: discover → search → select → plan → review →
//! validate → confirm → apply → history, against a throw-away HOME and a fake
//! `home-manager`.
//!
//! It uses the same `Service` and the same adapter as the window, so it exercises
//! the real bridge, worker and safety gate — not a parallel implementation. It
//! never reads or writes the caller's configuration; `HOME`, `XDG_STATE_HOME` and
//! the store root all point inside one temporary directory that is removed on
//! success.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nhs_adapter::Session;
use nhs_app::Event;
use nhs_core::EnvView;
use serde_json::{json, Value};

use crate::bridge::{BridgeError, UiState};
use crate::service::Service;

const WAIT: Duration = Duration::from_secs(60);

pub struct Flow {
    scratch: Scratch,
    service: Service,
    steps: Vec<Value>,
}

/// Owns the temporary tree. Removal happens in `Drop`, so an early `?` on any
/// step still cleans up — a success-only cleanup would leak the tree whenever a
/// step refused or the fake build failed.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Knobs for the test harness. The binary always uses [`Setup::default`].
#[doc(hidden)]
#[derive(Default)]
pub struct Setup {
    /// Exit status of the fake `home-manager`, so a test can make the flow fail.
    pub tool_exit: u8,
    /// Where the scratch tree goes; a test passes a path it can inspect afterwards.
    pub scratch: Option<PathBuf>,
    /// Return an error right after the guard is in place, to prove that an
    /// initialisation failure still cleans up.
    pub fail_setup: bool,
}

impl Flow {
    fn new(setup: &Setup) -> Result<Self, BridgeError> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let dir = setup.scratch.clone().unwrap_or_else(|| {
            std::env::temp_dir().join(format!("nhs-flow-{}-{stamp}", std::process::id()))
        });
        // Exclusive: never adopt (or later delete) a directory that already exists.
        fs::create_dir(&dir).map_err(|error| {
            BridgeError::internal(format!("cannot create the scratch root: {error}"))
        })?;
        // From here on every early return is covered: the guard owns the tree.
        let scratch = Scratch::new(dir);
        let root = scratch.path().to_path_buf();

        if setup.fail_setup {
            return Err(BridgeError::internal("injected setup failure"));
        }

        let home = root.join("home");
        let config = home.join(".config/home-manager");
        fs::create_dir_all(&config).map_err(|error| {
            BridgeError::internal(format!("cannot prepare the scratch HOME: {error}"))
        })?;
        fs::write(
            config.join("home.nix"),
            "{ pkgs, ... }:\n{\n  imports = [ ];\n  home.stateVersion = \"24.05\";\n}\n",
        )
        .map_err(|error| {
            BridgeError::internal(format!("cannot write the scratch config: {error}"))
        })?;

        let bin = root.join("bin");
        fs::create_dir_all(&bin).map_err(|error| {
            BridgeError::internal(format!("cannot create the fake toolchain: {error}"))
        })?;
        let tool = bin.join("home-manager");
        fs::write(
            &tool,
            format!(
                "#!/bin/sh\necho \"fake home-manager: $*\"\nexit {}\n",
                setup.tool_exit
            ),
        )
        .map_err(|error| BridgeError::internal(format!("cannot write the fake tool: {error}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&tool)
                .map_err(|error| BridgeError::internal(format!("fake tool metadata: {error}")))?
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&tool, permissions).map_err(|error| {
                BridgeError::internal(format!("fake tool permissions: {error}"))
            })?;
        }

        let env = EnvView {
            home,
            user: Some("flow-check".to_string()),
            path_entries: vec![bin],
            xdg_config_home: None,
            xdg_state_home: None,
            store_root: root.join("store"),
        };
        let state = root.join("state");
        Ok(Self {
            scratch,
            service: Service::new(Session::with_env(env, state)),
            steps: Vec::new(),
        })
    }

    fn record(&mut self, label: &str, state: &UiState) {
        self.steps.push(json!({
            "step": label,
            "screen": state.step,
            "selected": state.selected,
            "plan_files": state.plan.as_ref().map_or(0, |plan| plan.files.len()),
            "validation": state.validation.as_ref().map(|report| report.success),
            "apply": state.apply.state,
            "busy": state.busy.as_ref().map(|busy| busy.kind),
        }));
    }

    fn dispatch(&mut self, label: &str, event: Event) -> Result<UiState, BridgeError> {
        let state = self.service.dispatch(event)?;
        self.record(label, &state);
        Ok(state)
    }

    /// Wait for the worker to finish, then record the settled state.
    fn settle(&mut self, label: &str) -> Result<UiState, BridgeError> {
        let start = Instant::now();
        loop {
            let state = self.service.snapshot()?;
            if state.busy.is_none() {
                self.record(label, &state);
                return Ok(state);
            }
            if start.elapsed() > WAIT {
                return Err(BridgeError::internal(format!(
                    "the {label} job did not finish within {WAIT:?}"
                )));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn run(mut self) -> Result<Value, BridgeError> {
        let scratch = self.scratch.path().to_string_lossy().into_owned();
        self.dispatch("discover", Event::DiscoverRequested)?;
        self.dispatch(
            "search",
            Event::SearchSubmitted {
                query: "firefox".to_string(),
                filters: nhs_core::catalog::Filters::default(),
                page: 1,
                per_page: 20,
            },
        )?;
        self.dispatch("select", Event::PackageToggled("firefox".to_string()))?;
        self.dispatch("plan", Event::PlanRequested)?;
        self.dispatch("review", Event::DiffReviewed)?;
        self.dispatch("validate-start", Event::ValidationRequested)?;
        let validated = self.settle("validate")?;
        if !validated
            .validation
            .as_ref()
            .is_some_and(|report| report.success)
        {
            return Err(BridgeError::internal("the fake build did not validate"));
        }
        self.dispatch("confirm", Event::ConfirmationRequested)?;
        self.dispatch("apply-start", Event::ApplyConfirmed)?;
        let applied = self.settle("apply")?;
        if applied.apply.state != "applied" {
            return Err(BridgeError::internal("the apply did not finish"));
        }
        let history = self.dispatch("history", Event::HistoryRequested)?;

        let snapshot = applied.apply.snapshot.clone();
        let written = applied.apply.written.len();
        let snapshots = history
            .history
            .as_ref()
            .map_or(0, |value| value.snapshots.len());

        // `self` (and with it `Scratch`) is dropped when this returns, whichever
        // path the caller took. The report names the tree it used.
        Ok(json!({
            "ok": true,
            "scratch": scratch,
            "steps": self.steps,
            "snapshot": snapshot,
            "written": written,
            "history": { "snapshots": snapshots },
        }))
    }
}

/// Run the whole flow. Exists for CI and for the acceptance script.
///
/// # Errors
/// Any step refusing, the fake build failing, or the scratch tree not being
/// writable/removable.
pub fn run() -> Result<Value, BridgeError> {
    run_with(&Setup::default())
}

/// # Errors
/// Same as [`run`]; also used by tests to force a failing fake build.
pub fn run_with(setup: &Setup) -> Result<Value, BridgeError> {
    Flow::new(setup)?.run()
}

/// Kept for tests that want to look at the scratch tree before it is removed.
#[must_use]
pub fn is_temp(path: &Path) -> bool {
    path.starts_with(std::env::temp_dir())
}
