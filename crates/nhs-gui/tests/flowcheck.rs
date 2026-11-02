//! The headless full-flow check, run as the user would run it.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Result<Self, Box<dyn Error>> {
        let path =
            std::env::temp_dir().join(format!("nhs-flowcheck-{name}-{}", std::process::id()));
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

fn entries(root: &Path) -> usize {
    fs::read_dir(root).map_or(0, Iterator::count)
}

#[test]
fn flow_check_runs_the_whole_chain_in_isolation() -> Result<(), Box<dyn Error>> {
    let caller = TempDir::new("caller")?;
    let home = caller.path().join("home");
    let config = caller.path().join("config");
    let state = caller.path().join("state");
    fs::create_dir_all(&home)?;

    let output = Command::new(env!("CARGO_BIN_EXE_nhs-gui"))
        .arg("--flow-check")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config)
        .env("XDG_STATE_HOME", &state)
        .output()?;
    assert!(
        output.status.success(),
        "--flow-check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report.get("ok"), Some(&serde_json::json!(true)));
    let steps = report
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .ok_or("steps missing")?;
    assert_eq!(steps.len(), 11, "every step of the flow is recorded");
    let labels: Vec<&str> = steps
        .iter()
        .filter_map(|step| step.get("step").and_then(serde_json::Value::as_str))
        .collect();
    assert_eq!(
        labels,
        [
            "discover",
            "search",
            "select",
            "plan",
            "review",
            "validate-start",
            "validate",
            "confirm",
            "apply-start",
            "apply",
            "history"
        ]
    );

    let validation = steps
        .iter()
        .find(|step| step.get("step") == Some(&serde_json::json!("validate")))
        .ok_or("no validation step")?;
    assert_eq!(
        validation.get("validation"),
        Some(&serde_json::json!(true)),
        "the fake build must validate"
    );

    let apply = steps
        .iter()
        .find(|step| step.get("step") == Some(&serde_json::json!("apply")))
        .ok_or("no apply step")?;
    assert_eq!(apply.get("apply"), Some(&serde_json::json!("applied")));
    assert_eq!(
        report.get("written"),
        Some(&serde_json::json!(2)),
        "two files are written by the module plan"
    );
    assert!(report
        .get("snapshot")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|id| !id.is_empty()));
    assert_eq!(
        report.pointer("/history/snapshots"),
        Some(&serde_json::json!(1))
    );

    // Isolation: the caller's HOME and state directory stay untouched, and the
    // scratch tree is removed.
    assert_eq!(entries(&home), 0, "the caller's HOME gained files");
    assert!(!state.exists(), "the caller's state directory was created");
    assert!(
        !config.exists(),
        "the caller's config directory was created"
    );
    let scratch = report
        .get("scratch")
        .and_then(serde_json::Value::as_str)
        .ok_or("scratch path missing")?;
    assert!(
        !Path::new(scratch).exists(),
        "the scratch tree was left behind at {scratch}"
    );
    Ok(())
}

/// A failing step must still remove the scratch tree: cleanup lives in `Drop`,
/// not at the end of the success path.
#[test]
fn a_failed_flow_still_cleans_its_scratch_tree() -> Result<(), Box<dyn Error>> {
    let caller = TempDir::new("caller-fail")?;
    let home = caller.path().join("home");
    fs::create_dir_all(&home)?;
    let scratch = caller.path().join("scratch-for-failure");

    let setup = nhs_gui::flowcheck::Setup {
        tool_exit: 1,
        scratch: Some(scratch.clone()),
        fail_setup: false,
    };
    let result = nhs_gui::flowcheck::run_with(&setup);
    assert!(
        result.is_err(),
        "a fake build that exits 1 must fail the flow"
    );
    assert!(
        !scratch.exists(),
        "the scratch tree was left behind at {}",
        scratch.display()
    );
    // The caller's HOME is still untouched even on the failure path.
    assert_eq!(entries(&home), 0, "the caller's HOME gained files");
    Ok(())
}

/// The guard is created immediately after the root, so a failure during setup
/// (before any service exists) must also remove the tree.
#[test]
fn a_failed_setup_still_cleans_its_scratch_tree() -> Result<(), Box<dyn Error>> {
    let caller = TempDir::new("caller-setup")?;
    let home = caller.path().join("home");
    fs::create_dir_all(&home)?;
    let scratch = caller.path().join("scratch-for-setup-failure");

    let setup = nhs_gui::flowcheck::Setup {
        tool_exit: 0,
        scratch: Some(scratch.clone()),
        fail_setup: true,
    };
    let result = nhs_gui::flowcheck::run_with(&setup);
    assert!(result.is_err(), "the injected setup failure must surface");
    assert!(
        !scratch.exists(),
        "setup left the scratch tree behind at {}",
        scratch.display()
    );
    assert_eq!(entries(&home), 0, "the caller's HOME gained files");
    Ok(())
}

/// An existing directory must never be adopted: `create_dir` fails instead of
/// taking over (and potentially deleting) somebody else's tree.
#[test]
fn an_existing_scratch_path_is_refused_not_reused() -> Result<(), Box<dyn Error>> {
    let caller = TempDir::new("caller-existing")?;
    let scratch = caller.path().join("already-here");
    fs::create_dir_all(&scratch)?;
    fs::write(scratch.join("keep.txt"), "do not delete me")?;

    let setup = nhs_gui::flowcheck::Setup {
        tool_exit: 0,
        scratch: Some(scratch.clone()),
        ..Default::default()
    };
    let result = nhs_gui::flowcheck::run_with(&setup);
    assert!(result.is_err(), "an existing path must be refused");
    assert!(
        scratch.join("keep.txt").exists(),
        "the existing tree must be left untouched"
    );
    Ok(())
}
