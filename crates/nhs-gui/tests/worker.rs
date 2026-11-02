//! Background worker: busy slot, completion, cancellation, and the guarantee
//! that no job writes while another is registered.

use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nhs_app::Event;
use nhs_core::EnvView;
use nhs_gui::service::Service;
use serde_json::Value;

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Result<Self, Box<dyn Error>> {
        let unique = NEXT_DIR.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("nhs-gui-{name}-{}-{unique}", std::process::id()));
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

struct Fixture {
    dir: TempDir,
    service: Service,
    state: PathBuf,
    home: PathBuf,
}

fn fixture(name: &str, tool: &str) -> Result<Fixture, Box<dyn Error>> {
    let dir = TempDir::new(name)?;
    let home = dir.path().join("home");
    let config = home.join(".config/home-manager");
    fs::create_dir_all(&config)?;
    fs::write(
        config.join("home.nix"),
        "{ pkgs, ... }:\n{\n  imports = [ ];\n  home.stateVersion = \"24.05\";\n}\n",
    )?;
    let bin = dir.path().join("bin");
    fs::create_dir_all(&bin)?;
    let script = bin.join("home-manager");
    fs::write(&script, tool)?;
    let mut permissions = fs::metadata(&script)?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
    }
    fs::set_permissions(&script, permissions)?;

    let state = dir.path().join("state");
    let env = EnvView {
        home: home.clone(),
        user: Some("tester".to_string()),
        path_entries: vec![bin],
        xdg_config_home: None,
        xdg_state_home: None,
        store_root: dir.path().join("store"),
    };
    let session = nhs_adapter::Session::with_env(env, state.clone());
    Ok(Fixture {
        dir,
        service: Service::new(session),
        state,
        home,
    })
}

fn drive_to_plan(service: &Service) -> Result<(), Box<dyn Error>> {
    service.dispatch(Event::DiscoverRequested)?;
    service.dispatch(Event::SearchSubmitted {
        query: "firefox".to_string(),
        filters: nhs_core::catalog::Filters::default(),
        page: 1,
        per_page: 20,
    })?;
    service.dispatch(Event::PackageToggled("firefox".to_string()))?;
    service.dispatch(Event::PlanRequested)?;
    service.dispatch(Event::DiffReviewed)?;
    Ok(())
}

fn wait_idle(service: &Service, timeout: Duration) -> Result<(), Box<dyn Error>> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if service.snapshot()?.busy.is_none() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(format!("job did not finish within {timeout:?}").into())
}

#[test]
fn validation_runs_in_the_background_and_returns_to_the_state_machine() -> Result<(), Box<dyn Error>>
{
    let fixture = fixture("worker-ok", "#!/bin/sh\necho fake build\nexit 0\n")?;
    drive_to_plan(&fixture.service)?;

    let started = fixture.service.dispatch(Event::ValidationRequested)?;
    let busy = started.busy.ok_or("dispatch must report the running job")?;
    assert_eq!(busy.kind, "validation");
    assert!(
        started.validation.is_none(),
        "the result must not be invented before the worker finishes"
    );

    // A second dispatch is refused while the job owns the slot.
    let busy_error = fixture
        .service
        .dispatch(Event::SearchSubmitted {
            query: "x".to_string(),
            filters: nhs_core::catalog::Filters::default(),
            page: 1,
            per_page: 20,
        })
        .err()
        .ok_or("dispatch during a job must be refused")?;
    assert_eq!(busy_error.code, "busy");

    wait_idle(&fixture.service, Duration::from_secs(30))?;
    let state = fixture.service.snapshot()?;
    assert!(state.busy.is_none());
    let validation = state.validation.ok_or("validation result missing")?;
    assert!(validation.success, "fake build exits 0");
    Ok(())
}

#[test]
fn apply_after_validation_writes_one_snapshot() -> Result<(), Box<dyn Error>> {
    let fixture = fixture("worker-apply", "#!/bin/sh\nexit 0\n")?;
    drive_to_plan(&fixture.service)?;
    fixture.service.dispatch(Event::ValidationRequested)?;
    wait_idle(&fixture.service, Duration::from_secs(30))?;

    fixture.service.dispatch(Event::ConfirmationRequested)?;
    let started = fixture.service.dispatch(Event::ApplyConfirmed)?;
    assert_eq!(
        started.busy.map(|busy| busy.kind),
        Some("apply"),
        "apply must also run in the background"
    );
    wait_idle(&fixture.service, Duration::from_secs(30))?;

    let state = fixture.service.snapshot()?;
    assert_eq!(state.apply.state, "applied");
    assert!(
        state
            .apply
            .snapshot
            .as_deref()
            .is_some_and(|id| !id.is_empty()),
        "an applied state carries the snapshot id"
    );
    assert_eq!(state.apply.written.len(), 2);
    let module = fixture
        .home
        .join(".config/home-manager/modules/nhs-packages.nix");
    assert!(module.exists(), "the module must be written through core");
    let home_nix = fs::read_to_string(fixture.home.join(".config/home-manager/home.nix"))?;
    assert!(
        home_nix.contains("nhs-packages.nix"),
        "the import must be injected"
    );
    let snapshots = fixture.state.join("snapshots");
    let count = fs::read_dir(snapshots)?.count();
    assert_eq!(count, 1, "exactly one snapshot");
    Ok(())
}

#[test]
fn cancel_stops_the_running_job_and_does_not_poison_the_next_one() -> Result<(), Box<dyn Error>> {
    let fixture = fixture("worker-cancel", "#!/bin/sh\nsleep 20\nexit 0\n")?;
    drive_to_plan(&fixture.service)?;

    fixture.service.dispatch(Event::ValidationRequested)?;
    thread::sleep(Duration::from_millis(200));
    let ack = fixture.service.cancel()?;
    assert!(ack.job > 0);

    wait_idle(&fixture.service, Duration::from_secs(10))?;
    let state = fixture.service.snapshot()?;
    let validation = state.validation.ok_or("validation result missing")?;
    assert!(!validation.success, "a cancelled build cannot succeed");
    let kind = validation.kind.ok_or("a cancelled run is diagnosed")?;
    assert_eq!(kind.code, "interrupted");

    // Idle cancel is an honest error.
    let idle = fixture
        .service
        .cancel()
        .err()
        .ok_or("idle cancel must fail")?;
    assert_eq!(idle.code, "cancel_unavailable");

    // The cancelled flag must not leak into the next job: a fresh validation
    // against a fast toolchain has to succeed. Swap the fake tool first.
    fixture_fast_tool(&fixture)?;
    Ok(())
}

/// Replace the sleeping fake with a fast one so a second run can succeed.
fn fixture_fast_tool(fixture: &Fixture) -> Result<(), Box<dyn Error>> {
    let script = fixture.dir.path().join("bin").join("home-manager");
    fs::write(&script, "#!/bin/sh\nexit 0\n")?;
    fixture.service.dispatch(Event::ValidationRequested)?;
    wait_idle(&fixture.service, Duration::from_secs(15))?;
    let state = fixture.service.snapshot()?;
    let validation = state.validation.ok_or("second validation missing")?;
    assert!(
        validation.success,
        "a job started after a cancel must run normally"
    );
    Ok(())
}

#[test]
fn state_without_a_job_has_no_busy_slot() -> Result<(), Box<dyn Error>> {
    let fixture = fixture("worker-idle", "#!/bin/sh\nexit 0\n")?;
    let state = fixture.service.snapshot()?;
    assert!(state.busy.is_none());

    let cancel = fixture.service.cancel().err().ok_or("nothing to cancel")?;
    assert_eq!(cancel.code, "cancel_unavailable");

    // And the JSON contract carries the slot as null.
    let value = serde_json::to_value(&state)?;
    assert_eq!(value.get("busy"), Some(&Value::Null));
    Ok(())
}

#[test]
fn the_worker_notifies_the_shell_with_a_finished_state() -> Result<(), Box<dyn Error>> {
    let fixture = fixture("worker-notify", "#!/bin/sh\nexit 0\n")?;
    let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    fixture.service.set_notifier(Arc::new(move |state| {
        if let Ok(mut log) = sink.lock() {
            if let Ok(value) = serde_json::to_value(state) {
                log.push(value);
            }
        }
    }))?;

    drive_to_plan(&fixture.service)?;
    fixture.service.dispatch(Event::ValidationRequested)?;
    wait_idle(&fixture.service, Duration::from_secs(30))?;

    let states = seen.lock().map_err(|_| "notifier log poisoned")?;
    assert!(
        !states.is_empty(),
        "the worker must notify the shell when it finishes"
    );
    let last = states.last().ok_or("no state")?;
    assert_eq!(last.get("busy"), Some(&Value::Null), "busy is cleared");
    assert_eq!(
        last.pointer("/validation/success"),
        Some(&Value::Bool(true)),
        "the finished state carries the result"
    );
    Ok(())
}

#[test]
fn logs_stream_before_the_build_finishes() -> Result<(), Box<dyn Error>> {
    // Twenty lines, 40 ms apart: long enough that a non-streaming worker would
    // show nothing until the end.
    let tool = "#!/bin/sh\nfor i in $(seq 1 20); do echo \"line $i\"; sleep 0.04; done\nexit 0\n";
    let fixture = fixture("worker-stream", tool)?;
    let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    fixture.service.set_notifier(Arc::new(move |state| {
        if let Ok(mut log) = sink.lock() {
            if let Ok(value) = serde_json::to_value(state) {
                log.push(value);
            }
        }
    }))?;

    drive_to_plan(&fixture.service)?;
    fixture.service.dispatch(Event::ValidationRequested)?;
    wait_idle(&fixture.service, Duration::from_secs(30))?;

    let states = seen.lock().map_err(|_| "notifier log poisoned")?;
    let during = states
        .iter()
        .filter(|state| state.get("busy").is_some_and(|busy| !busy.is_null()))
        .count();
    let with_lines = states
        .iter()
        .filter(|state| {
            state
                .pointer("/log/lines")
                .and_then(Value::as_array)
                .is_some_and(|lines| !lines.is_empty())
        })
        .count();
    assert!(
        during > 1 && with_lines > 1,
        "log lines must be pushed while the build runs (busy states: {during}, with lines: {with_lines})"
    );
    Ok(())
}

#[test]
fn a_failed_worker_start_leaves_no_stuck_job() -> Result<(), Box<dyn Error>> {
    let fixture = fixture("worker-spawn-fail", "#!/bin/sh\nexit 0\n")?;
    drive_to_plan(&fixture.service)?;

    // Validation runs on a real thread; the apply that follows cannot start.
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let service = Service::with_spawner(
        nhs_adapter::Session::with_env(
            EnvView {
                home: fixture.home.clone(),
                user: Some("tester".to_string()),
                path_entries: vec![fixture.dir.path().join("bin")],
                xdg_config_home: None,
                xdg_state_home: None,
                store_root: fixture.dir.path().join("store"),
            },
            fixture.state.clone(),
        ),
        Arc::new(move |task: Box<dyn FnOnce() + Send>| {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                std::thread::spawn(task);
                Ok(())
            } else {
                Err(io::Error::other("no thread for you"))
            }
        }),
    );

    service.dispatch(Event::DiscoverRequested)?;
    service.dispatch(Event::SearchSubmitted {
        query: "firefox".to_string(),
        filters: nhs_core::catalog::Filters::default(),
        page: 1,
        per_page: 20,
    })?;
    service.dispatch(Event::PackageToggled("firefox".to_string()))?;
    service.dispatch(Event::PlanRequested)?;
    service.dispatch(Event::DiffReviewed)?;
    service.dispatch(Event::ValidationRequested)?;
    wait_idle(&service, Duration::from_secs(30))?;
    assert!(service.snapshot()?.validation.is_some_and(|v| v.success));

    service.dispatch(Event::ConfirmationRequested)?;
    let started = service.dispatch(Event::ApplyConfirmed)?;
    assert_eq!(started.busy.map(|busy| busy.kind), Some("apply"));

    let state = service.snapshot()?;
    assert!(state.busy.is_none(), "a dead worker must not stay busy");
    assert_eq!(
        state.apply.state, "failed",
        "apply must say it did not run, not stay running"
    );
    assert!(
        state
            .apply
            .message
            .as_deref()
            .is_some_and(|message| message.contains("could not start")),
        "the failure explains itself"
    );
    Ok(())
}

/// A quiet build must still reach the window: one line, then silence, then exit.
/// Flushing only when the next line arrives would hold that line until the end.
#[test]
fn a_quiet_build_still_flushes_within_the_interval() -> Result<(), Box<dyn Error>> {
    let tool = "#!/bin/sh\necho \"only line\"\nsleep 0.8\nexit 0\n";
    let fixture = fixture("worker-quiet", tool)?;
    let seen: Arc<Mutex<Vec<(Instant, Value)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    fixture.service.set_notifier(Arc::new(move |state| {
        if let Ok(mut log) = sink.lock() {
            if let Ok(value) = serde_json::to_value(state) {
                log.push((Instant::now(), value));
            }
        }
    }))?;

    drive_to_plan(&fixture.service)?;
    let started = Instant::now();
    fixture.service.dispatch(Event::ValidationRequested)?;
    wait_idle(&fixture.service, Duration::from_secs(30))?;

    let states = seen.lock().map_err(|_| "notifier log poisoned")?;
    let first = states.iter().find(|(_, state)| {
        state
            .pointer("/log/lines")
            .and_then(Value::as_array)
            .is_some_and(|lines| !lines.is_empty())
    });

    let (at, state) = first.ok_or("the line never reached the window")?;
    let delay = at.duration_since(started);
    assert!(
        delay < Duration::from_millis(500),
        "the line arrived after {delay:?}, which is only at process exit"
    );
    // The build itself ran for ~800 ms, and the notification still says `busy`:
    // the line was delivered by the ticker while the process was silent, not by
    // the next line or by the final result.
    assert!(
        !state["busy"].is_null(),
        "the line must arrive while the job is still running"
    );
    Ok(())
}

#[test]
fn a_failed_build_is_diagnosed_and_keeps_the_scratch_copy() -> Result<(), Box<dyn Error>> {
    let tool = "#!/bin/sh\necho \"error: syntax error, unexpected '}'\" >&2\nexit 1\n";
    let fixture = fixture("worker-fail", tool)?;
    drive_to_plan(&fixture.service)?;
    fixture.service.dispatch(Event::ValidationRequested)?;
    wait_idle(&fixture.service, Duration::from_secs(30))?;

    let state = fixture.service.snapshot()?;
    let report = state.validation.ok_or("a failed run still reports")?;
    assert!(!report.success);
    let kind = report.kind.ok_or("a recognised failure carries a code")?;
    assert!(!kind.code.is_empty(), "the code is what the UI translates");
    assert!(report.summary.is_some(), "the raw message is kept");
    assert!(
        report.scratch.is_some(),
        "a failed run keeps its scratch copy for inspection"
    );
    assert!(
        state.apply.state == "idle",
        "a failed validation must not apply anything"
    );
    Ok(())
}

#[test]
fn a_second_apply_is_refused_and_history_lists_one_snapshot() -> Result<(), Box<dyn Error>> {
    let fixture = fixture("worker-twice", "#!/bin/sh\nexit 0\n")?;
    drive_to_plan(&fixture.service)?;
    fixture.service.dispatch(Event::ValidationRequested)?;
    wait_idle(&fixture.service, Duration::from_secs(30))?;
    fixture.service.dispatch(Event::ConfirmationRequested)?;
    fixture.service.dispatch(Event::ApplyConfirmed)?;
    wait_idle(&fixture.service, Duration::from_secs(30))?;
    assert_eq!(fixture.service.snapshot()?.apply.state, "applied");

    // Going back to the confirmation screen must not authorise a second write.
    let refused = fixture
        .service
        .dispatch(Event::ConfirmationRequested)
        .err()
        .ok_or("the state machine must refuse a second confirmation")?;
    assert_eq!(refused.code, "refused");
    assert_eq!(refused.detail.as_deref(), Some("already_applied"));
    let state = fixture.service.snapshot()?;
    assert_eq!(state.apply.state, "applied", "the first result stands");
    assert_eq!(
        state.readiness.as_ref().map(|readiness| readiness.code),
        Some("already_applied"),
        "the UI learns why confirmation is closed"
    );

    let history = fixture.service.dispatch(Event::HistoryRequested)?;
    let history = history.history.ok_or("history must load")?;
    assert_eq!(history.reason_code, "history_read_only");
    assert_eq!(history.snapshots.len(), 1, "one snapshot, not two");
    let first = history.snapshots.first().ok_or("one snapshot expected")?;
    assert_eq!(first.outcome, "applied");
    Ok(())
}

/// A write the filesystem refuses must end in `failed`, be recorded as a failed
/// attempt (so history shows what was tried), roll the files back, and leave no
/// half-written module.
#[test]
fn a_failed_write_is_rolled_back_and_recorded() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;

    let fixture = fixture("worker-apply-fail", "#!/bin/sh\nexit 0\n")?;
    drive_to_plan(&fixture.service)?;
    fixture.service.dispatch(Event::ValidationRequested)?;
    wait_idle(&fixture.service, Duration::from_secs(30))?;
    fixture.service.dispatch(Event::ConfirmationRequested)?;

    let config = fixture.home.join(".config/home-manager");
    let original = fs::metadata(&config)?.permissions();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o555))?;

    fixture.service.dispatch(Event::ApplyConfirmed)?;
    wait_idle(&fixture.service, Duration::from_secs(30))?;

    fs::set_permissions(&config, original)?;

    let state = fixture.service.snapshot()?;
    assert_eq!(
        state.apply.state, "failed",
        "the write must be reported as failed"
    );
    assert!(
        state
            .apply
            .message
            .as_deref()
            .is_some_and(|message| !message.is_empty()),
        "the failure carries a message"
    );
    assert!(
        state.apply.snapshot.is_none(),
        "a failed write must not invent a snapshot"
    );
    // A failed attempt is recorded as `failed` — the history must show what was
    // tried and that it was rolled back, not pretend nothing happened.
    let snapshots = fixture.state.join("snapshots");
    let files: Vec<_> = fs::read_dir(&snapshots)?.filter_map(Result::ok).collect();
    assert_eq!(files.len(), 1, "one attempt record");
    let first = files.first().ok_or("one attempt record")?;
    let raw = fs::read_to_string(first.path())?;
    let record: serde_json::Value = serde_json::from_str(&raw)?;
    assert_eq!(
        record.get("outcome").and_then(serde_json::Value::as_str),
        Some("failed")
    );
    assert!(
        record
            .get("error")
            .and_then(serde_json::Value::as_str)
            .is_some(),
        "the record keeps the reason"
    );

    // And the files are exactly as they were: the import was rolled back and the
    // module was never left behind.
    let home_nix = fs::read_to_string(config.join("home.nix"))?;
    assert!(
        !home_nix.contains("nhs-packages.nix"),
        "the configuration must be left untouched"
    );
    assert!(
        !config.join("modules/nhs-packages.nix").exists(),
        "no half-written module may remain"
    );
    Ok(())
}
