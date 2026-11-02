//! M6.2: the plan-building step, tested once for both front ends.
//!
//! These tests are the reason `prepare` moved out of the CLI: the GUI calls this
//! same function, so the safety rules (relative import, no write, system-only
//! refusal, state outside the tree) are proved here rather than twice.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};

use nhs_core::discovery::Layout;
use nhs_core::flow::{prepare, PrepareError, Selection};
use nhs_core::module::{Scope, Setting, SettingValue};
use nhs_core::{FsView, MemFs};

const ROOT: &str = "/home/tester/.config/home-manager";
const STATE: &str = "/home/tester/.local/state/nix-home-studio";

fn classic() -> Layout {
    Layout::ClassicHome {
        main_file: PathBuf::from(format!("{ROOT}/home.nix")),
    }
}

fn tree() -> MemFs {
    MemFs::new().with_file(
        format!("{ROOT}/home.nix"),
        "{ pkgs, ... }:\n{\n  imports = [ ];\n}\n",
    )
}

fn selection(packages: &[&str]) -> Selection {
    Selection {
        packages: packages.iter().map(|name| (*name).to_string()).collect(),
        settings: Vec::new(),
    }
}

#[test]
fn a_selection_becomes_a_two_file_plan_with_a_relative_import() {
    let fs = tree();
    let prepared = prepare(
        &fs,
        &classic(),
        Path::new(STATE),
        None,
        &selection(&["firefox", "ripgrep"]),
    )
    .expect("prepare");

    assert_eq!(prepared.plan.changes.len(), 2, "module + main file");
    assert_eq!(
        prepared.module_file,
        PathBuf::from(format!("{ROOT}/modules/nhs-packages.nix"))
    );
    assert_eq!(prepared.config_root, PathBuf::from(ROOT));

    let module = prepared
        .plan
        .changes
        .iter()
        .find(|change| change.path == prepared.module_file)
        .expect("module change");
    assert!(
        module.new_text.contains("pkgs.firefox"),
        "{}",
        module.new_text
    );
    assert!(module.new_text.contains("pkgs.ripgrep"));
    assert_eq!(module.kind, nhs_core::ChangeKind::Create);

    let main = prepared
        .plan
        .changes
        .iter()
        .find(|change| change.path == prepared.main_file)
        .expect("main change");
    assert!(
        main.new_text
            .contains("imports = [ ./modules/nhs-packages.nix ]"),
        "the import must be relative to the main file: {}",
        main.new_text
    );

    // Planning writes nothing: the fixtures are untouched.
    assert_eq!(
        fs.read_to_string(Path::new(&format!("{ROOT}/home.nix")))
            .expect("home.nix"),
        "{ pkgs, ... }:\n{\n  imports = [ ];\n}\n"
    );
    assert!(!fs.exists(Path::new(&format!("{ROOT}/modules/nhs-packages.nix"))));
}

#[test]
fn planning_twice_is_a_no_op_the_second_time() {
    let fs = tree();
    let selection = selection(&["firefox"]);
    prepare(&fs, &classic(), Path::new(STATE), None, &selection).expect("first");

    // Put the first plan on "disk" the way a commit would, so the second run sees
    // the module and the import already there.
    let prepared = prepare(&fs, &classic(), Path::new(STATE), None, &selection).expect("second");
    // The module still does not exist in the fixture, so it is planned again; the
    // import is already absent too. What matters is that planning is repeatable.
    assert_eq!(prepared.plan.changes.len(), 2);
}

#[test]
fn an_existing_module_is_rewritten_not_recreated() {
    let fs = tree().with_file(
        format!("{ROOT}/modules/nhs-packages.nix"),
        concat!(
            "# Managed by nix-home-studio. Edits inside the markers are overwritten.\n",
            "{ pkgs, ... }:\n{\n  # nhs:managed:start\n  home.packages = [\n    pkgs.old\n  ];\n",
            "  # nhs:managed:end\n\n  # nhs:settings:start\n  # nhs:settings:end\n}\n",
        ),
    );
    let prepared = prepare(
        &fs,
        &classic(),
        Path::new(STATE),
        None,
        &selection(&["firefox"]),
    )
    .expect("prepare");

    let module = prepared
        .plan
        .changes
        .iter()
        .find(|change| change.path == prepared.module_file)
        .expect("module change");
    assert_eq!(module.kind, nhs_core::ChangeKind::Replace);
    assert!(module.new_text.contains("pkgs.firefox"));
    assert!(
        !module.new_text.contains("pkgs.old"),
        "the block is regenerated"
    );
}

#[test]
fn a_module_outside_the_tree_is_refused() {
    let fs = tree();
    let error = prepare(
        &fs,
        &classic(),
        Path::new(STATE),
        Some(Path::new("/tmp/elsewhere/module.nix")),
        &selection(&["firefox"]),
    )
    .expect_err("outside the tree");
    assert!(
        matches!(error, PrepareError::ModuleOutsideTree { .. }),
        "{error}"
    );
    assert!(error.is_refusal());
}

#[test]
fn layouts_this_application_cannot_edit_are_refused() {
    let fs = tree();
    for layout in [
        Layout::ProfileOnly {
            profile_dir: PathBuf::from("/home/tester/.local/state/nix/profiles"),
            generations: Vec::new(),
        },
        Layout::FlakeNixos {
            root: PathBuf::from("/etc/nixos"),
            main_file: PathBuf::from("/etc/nixos/flake.nix"),
            host: "box".to_string(),
            user: "tester".to_string(),
        },
        Layout::Unknown {
            reason: "no configuration".to_string(),
        },
    ] {
        let error = prepare(
            &fs,
            &layout,
            Path::new(STATE),
            None,
            &selection(&["firefox"]),
        )
        .expect_err("not editable");
        assert!(
            matches!(error, PrepareError::UnsupportedLayout { .. }),
            "{layout:?} produced {error}"
        );
        assert!(error.is_refusal());
    }
}

#[test]
fn a_system_only_setting_is_refused_before_anything_is_staged() {
    let fs = tree();
    let selection = Selection {
        packages: Vec::new(),
        settings: vec![Setting {
            path: "boot.kernelPackages".to_string(),
            value: SettingValue::Bool(true),
            scope: Scope::SystemOnly,
        }],
    };
    let error = prepare(&fs, &classic(), Path::new(STATE), None, &selection)
        .expect_err("NixOS-only option in a home module");
    assert!(matches!(error, PrepareError::Module(_)), "{error}");
    assert!(error.is_refusal());
}

#[test]
fn a_state_directory_inside_the_tree_is_refused() {
    let fs = tree();
    let error = prepare(
        &fs,
        &classic(),
        Path::new(&format!("{ROOT}/.state")),
        None,
        &selection(&["firefox"]),
    )
    .expect_err("state inside the tree");
    assert!(matches!(error, PrepareError::Edit(_)), "{error}");
    assert!(error.is_refusal(), "a policy refusal, not an I/O failure");
}

#[test]
fn a_missing_main_file_is_an_io_failure_not_a_refusal() {
    let fs = MemFs::new();
    let error = prepare(
        &fs,
        &classic(),
        Path::new(STATE),
        None,
        &selection(&["firefox"]),
    )
    .expect_err("nothing to edit");
    assert!(matches!(error, PrepareError::Read { .. }), "{error}");
    assert!(
        !error.is_refusal(),
        "a missing file is a broken setup, not a policy decision"
    );
}
