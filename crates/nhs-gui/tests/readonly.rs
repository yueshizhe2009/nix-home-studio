//! The read-only promise, checked two ways: the source may not contain the APIs
//! that write, and the headless run may not change a single byte.

use std::error::Error;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Result<Self, Box<dyn Error>> {
        let path = std::env::temp_dir().join(format!("nhs-gui-{name}-{}", std::process::id()));
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

/// Hash every file under `root` (path + content), so any write shows up.
fn digest(root: &Path) -> Result<String, Box<dyn Error>> {
    let mut entries = Vec::new();
    collect(root, &mut entries)?;
    entries.sort();
    let mut hasher = Sha256::new();
    for path in entries {
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(fs::read(&path)?);
    }
    let mut text = String::new();
    for byte in hasher.finalize() {
        let _ = write!(text, "{byte:02x}");
    }
    Ok(text)
}

fn collect(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

#[test]
fn source_contains_no_write_or_spawn_apis() -> Result<(), Box<dyn Error>> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    // `flowcheck.rs` is the one file allowed to write, because the headless check
    // has to build a scratch HOME and a fake toolchain before it can run. It is
    // held to its own rules below, and `tests/flowcheck.rs` proves at runtime that
    // the caller's HOME stays empty.
    let allowed_to_write = "flowcheck.rs";
    let forbidden = [
        "File::create",
        "OpenOptions",
        "fs::write",
        "fs::remove",
        "fs::rename",
        "fs::copy",
        "Command::new",
        "std::process::Command",
    ];
    let mut checked = 0_usize;
    for entry in fs::read_dir(&source)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let text = fs::read_to_string(&path)?;
        if path.file_name().and_then(|name| name.to_str()) == Some(allowed_to_write) {
            assert!(
                text.contains("std::env::temp_dir()"),
                "the allowed file must work inside the temporary directory"
            );
            assert!(
                !text.contains("Command::new") && !text.contains("std::process::Command"),
                "even the flow check may not start processes itself"
            );
            checked += 1;
            continue;
        }
        for needle in forbidden {
            assert!(
                !text.contains(needle),
                "{} contains {needle}; the GUI must not touch files or spawn processes",
                path.display()
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 5,
        "expected to scan the GUI sources, saw {checked}"
    );
    Ok(())
}

#[test]
fn check_mode_reports_resolved_generation() -> Result<(), Box<dyn Error>> {
    let dir = TempDir::new("check-generation")?;
    let home = dir.path().join("home");
    let state = dir.path().join("state");
    let config = home.join(".config/home-manager");
    fs::create_dir_all(&config)?;
    fs::write(
        config.join("home.nix"),
        "{ pkgs, ... }:\n{\n  imports = [ ];\n  home.stateVersion = \"24.05\";\n}\n",
    )?;

    let store = dir.path().join("store");
    fs::create_dir_all(&store)?;
    let first = store.join("hm-gen1-home-manager-generation");
    let second = store.join("hm-gen2-home-manager-generation");
    fs::write(&first, "generation 1")?;
    fs::write(&second, "generation 2")?;
    let profiles = state.join("nix/profiles");
    fs::create_dir_all(&profiles)?;
    symlink(&first, profiles.join("home-manager-1-link"))?;
    symlink(&second, profiles.join("home-manager-2-link"))?;
    symlink(&second, profiles.join("home-manager"))?;

    let before = digest(dir.path())?;

    let output = Command::new(env!("CARGO_BIN_EXE_nhs-gui"))
        .arg("--check")
        .env("HOME", &home)
        .env("XDG_STATE_HOME", &state)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .output()?;
    assert!(
        output.status.success(),
        "--check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let state_json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let discovery = state_json
        .get("discovery")
        .ok_or("--check emitted no discovery")?;
    assert_eq!(
        discovery.get("kind").and_then(serde_json::Value::as_str),
        Some("classic-home")
    );
    assert_eq!(
        discovery
            .get("generations")
            .and_then(serde_json::Value::as_u64),
        Some(2),
        "generation count must come from the profile snapshot"
    );
    assert_eq!(
        discovery
            .get("current_generation")
            .and_then(serde_json::Value::as_u64),
        Some(2),
        "current generation must be resolved from the profile link"
    );
    let link = discovery
        .get("current_generation_link")
        .and_then(serde_json::Value::as_str)
        .ok_or("current_generation_link is missing")?;
    assert!(
        link.ends_with("home-manager-2-link"),
        "unexpected current generation link: {link}"
    );

    let after = digest(dir.path())?;
    assert_eq!(before, after, "--check modified the generation fixture");
    Ok(())
}

#[test]
fn check_mode_writes_nothing() -> Result<(), Box<dyn Error>> {
    let dir = TempDir::new("check")?;
    let home = dir.path().join("home");
    let state = dir.path().join("state");
    let config = home.join(".config/home-manager");
    fs::create_dir_all(&config)?;
    fs::write(
        config.join("home.nix"),
        "{ pkgs, ... }:\n{\n  imports = [ ];\n  home.stateVersion = \"24.05\";\n}\n",
    )?;
    fs::write(config.join("marker.txt"), "untouched")?;

    let before = digest(dir.path())?;

    let output = Command::new(env!("CARGO_BIN_EXE_nhs-gui"))
        .arg("--check")
        .env("HOME", &home)
        .env("XDG_STATE_HOME", &state)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .output()?;
    assert!(
        output.status.success(),
        "--check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let state_json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let version = state_json
        .get("version")
        .and_then(serde_json::Value::as_u64);
    assert_eq!(
        version,
        Some(u64::from(nhs_gui::bridge::STATE_VERSION)),
        "--check must emit the same state version the window renders"
    );
    let discovery = state_json.get("discovery");
    let kind = discovery
        .and_then(|found| found.get("kind"))
        .and_then(serde_json::Value::as_str);
    assert_eq!(kind, Some("classic-home"));
    let editable = discovery
        .and_then(|found| found.get("editable"))
        .and_then(serde_json::Value::as_bool);
    assert_eq!(editable, Some(true));

    let after = digest(dir.path())?;
    assert_eq!(before, after, "--check modified the tree it read");
    assert!(
        !state.exists(),
        "--check created a state directory at {}",
        state.display()
    );
    Ok(())
}
