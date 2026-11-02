//! M5.4 acceptance tests: `nhs search`, both tiers, through the real binary.
//!
//! Curated searches need no fake at all — that is the point of the offline tier.
//! The full tier is exercised with a fake `nix` that prints a canned
//! `nix search --json` object and records its argv, so "the second run made no
//! call" is a fact rather than an intention.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

use tempfile::TempDir;

const REV: &str = "0123456789abcdef0123456789abcdef01234567";

struct Fixture {
    home: PathBuf,
    state: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    #[allow(dead_code)]
    dir: TempDir,
}

impl Fixture {
    /// A classic config with a `flake.lock` and a fake `nix`.
    fn new(entries: &[(&str, &str)], lock: Option<&str>) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let config = home.join(".config").join("home-manager");
        fs::create_dir_all(&config).expect("config dir");
        fs::write(config.join("home.nix"), "{ pkgs, ... }:\n{\n}\n").expect("home.nix");
        if let Some(lock) = lock {
            fs::write(config.join("flake.lock"), lock).expect("flake.lock");
        }
        let state = dir.path().join("state");
        fs::create_dir_all(&state).expect("state");
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).expect("bin");
        let log = dir.path().join("argv.log");

        let mut json = String::from("{");
        for (index, (attr, pname)) in entries.iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            json.push_str(&format!(
                r#""legacyPackages.x86_64-linux.{attr}":{{"pname":"{pname}","version":"1.0","description":"desc {pname}"}}"#
            ));
        }
        json.push('}');

        let script = format!(
            "#!/bin/sh\nprintf '%s\\t%s\\n' \"$PWD\" \"$*\" >> '{}'\nprintf '%s\\n' '{}'\nexit 0\n",
            log.display(),
            json
        );
        let path = bin.join("nix");
        fs::write(&path, script).expect("fake nix");
        let mut permissions = fs::metadata(&path).expect("stat").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod");

        Self {
            home,
            state,
            bin,
            log,
            dir,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_nhs"))
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_STATE_HOME", &self.state)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("USER", "tester")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .expect("run nhs")
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout(output)).unwrap_or_else(|error| {
        panic!(
            "not JSON ({error})\nstdout:\n{}\nstderr:\n{}",
            stdout(output),
            stderr(output)
        )
    })
}

fn lock() -> String {
    format!(
        r#"{{ "nodes": {{ "root": {{ "inputs": {{ "nixpkgs": "nixpkgs" }} }}, "nixpkgs": {{ "locked": {{ "type": "github", "owner": "NixOS", "repo": "nixpkgs", "rev": "{REV}" }} }} }}, "root": "root", "version": 7 }}"#
    )
}

/// The curated tier works with no `nix` at all, and ranks an exact name first.
#[test]
fn curated_search_is_offline_and_ranks_exact_names_first() {
    let fixture = Fixture::new(&[], None);
    // Point PATH at a directory without nix: the curated tier must not need it.
    let output = Command::new(env!("CARGO_BIN_EXE_nhs"))
        .args(["search", "firefox", "--json"])
        .env("HOME", &fixture.home)
        .env("XDG_STATE_HOME", &fixture.state)
        .env("PATH", "/usr/bin:/bin")
        .env("USER", "tester")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("run nhs");

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value = json(&output);
    assert_eq!(value["source"]["kind"], "curated");
    assert_eq!(value["source"]["pinned"], serde_json::Value::Null);
    assert_eq!(value["source"]["reference"], serde_json::Value::Null);
    let hits = value["hits"].as_array().expect("hits");
    assert!(!hits.is_empty());
    assert_eq!(hits[0]["attr"], "firefox");
    assert_eq!(hits[0]["version"], serde_json::Value::Null);
    assert!(hits[0]["category"].is_string());
    assert!(
        hits[0]["platform"].is_array(),
        "curated hits carry the catalogue's platform tags: {}",
        hits[0]["platform"]
    );
    assert!(fixture.calls().is_empty(), "no process may run");
}

/// Metadata filters reach the curated search.
#[test]
fn filters_reach_the_cli() {
    let fixture = Fixture::new(&[], None);
    let darwin = fixture.run(&["search", "kitty", "--json", "--platform", "aarch64-darwin"]);
    let value = json(&darwin);
    let attrs: Vec<&str> = value["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .filter_map(|hit| hit["attr"].as_str())
        .collect();
    assert!(attrs.contains(&"kitty"), "{attrs:?}");

    let linux_only = fixture.run(&[
        "search",
        "virt-manager",
        "--json",
        "--platform",
        "aarch64-darwin",
    ]);
    assert_eq!(
        json(&linux_only)["total"],
        0,
        "virt-manager declares Linux only"
    );

    let hidden = fixture.run(&["search", "vscode", "--json"]);
    let attrs: Vec<String> = json(&hidden)["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .filter_map(|hit| hit["attr"].as_str().map(str::to_string))
        .collect();
    assert!(!attrs.contains(&"vscode".to_string()), "{attrs:?}");

    let shown = fixture.run(&["search", "vscode", "--json", "--include-unfree"]);
    let attrs: Vec<String> = json(&shown)["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .filter_map(|hit| hit["attr"].as_str().map(str::to_string))
        .collect();
    assert!(attrs.contains(&"vscode".to_string()), "{attrs:?}");
}

/// The full tier builds once, then reuses the snapshot without calling nix again.
#[test]
fn full_search_builds_then_reuses_the_snapshot() {
    let fixture = Fixture::new(&[("hello", "hello"), ("ripgrep", "ripgrep")], Some(&lock()));

    let first = fixture.run(&["search", "hello", "--full", "--json"]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    let value = json(&first);
    assert_eq!(value["source"]["kind"], "flake-lock");
    assert_eq!(value["source"]["pinned"], true);
    assert_eq!(value["source"]["revision"], REV);
    let hits = value["hits"].as_array().expect("hits");
    assert_eq!(hits[0]["attr"], "hello");
    assert_eq!(hits[0]["version"], "1.0");
    assert_eq!(hits[0]["category"], serde_json::Value::Null);
    assert_eq!(hits[0]["license"], serde_json::Value::Null);
    assert_eq!(hits[0]["platform"], serde_json::Value::Null);
    assert_eq!(fixture.calls().len(), 1, "one nix search");
    assert!(
        fixture.calls()[0].contains("search --json"),
        "{:?}",
        fixture.calls()
    );
    assert!(fixture.calls()[0].contains(REV));

    // Second run: cache, no process, and the progress line says so.
    let second = fixture.run(&["search", "ripgrep", "--full", "--json"]);
    assert_eq!(second.status.code(), Some(0), "{}", stderr(&second));
    assert_eq!(
        fixture.calls().len(),
        1,
        "a cached catalogue must not call nix again"
    );
    assert!(
        stderr(&second).contains("using cached catalogue"),
        "{}",
        stderr(&second)
    );
    assert_eq!(json(&second)["hits"][0]["attr"], "ripgrep");
}

/// Metadata filters are refused for the full tier, before any process starts.
#[test]
fn full_search_refuses_metadata_filters() {
    let fixture = Fixture::new(&[("hello", "hello")], Some(&lock()));
    for filter in [
        "--include-unfree",
        "--include-broken",
        "--include-nixos-only",
        "--platform",
    ] {
        let output = if filter == "--platform" {
            fixture.run(&["search", "hello", "--full", "--platform", "linux"])
        } else {
            fixture.run(&["search", "hello", "--full", filter])
        };
        assert_eq!(
            output.status.code(),
            Some(3),
            "`{filter}` must be a refusal: {}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains("--full"),
            "the message must name the conflict: {}",
            stderr(&output)
        );
        assert!(
            fixture.calls().is_empty(),
            "a refusal must not start nix: {:?}",
            fixture.calls()
        );
    }

    // The licence filters are refused the same way.
    let output = fixture.run(&["search", "hello", "--full", "--license-allow", "MIT"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(fixture.calls().is_empty());
}

/// A broken lock is reported as a lock problem, with no `--flake-ref` to mask it.
#[test]
fn full_search_reports_a_broken_lock() {
    let fixture = Fixture::new(&[("hello", "hello")], Some("{ not a lock"));
    let output = fixture.run(&["search", "hello", "--full", "--json"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("flake.lock"),
        "the message must name the lock file: {}",
        stderr(&output)
    );
    assert!(fixture.calls().is_empty(), "nothing may be run");
}

/// Paging is stable and disjoint on both tiers.
#[test]
fn pagination_is_stable_and_disjoint() {
    let fixture = Fixture::new(&[], None);
    let first = fixture.run(&["search", "", "--json", "--per-page", "5", "--page", "1"]);
    let second = fixture.run(&["search", "", "--json", "--per-page", "5", "--page", "2"]);
    let one = json(&first);
    let two = json(&second);
    let attrs = |value: &serde_json::Value| -> Vec<String> {
        value["hits"]
            .as_array()
            .expect("hits")
            .iter()
            .filter_map(|hit| hit["attr"].as_str().map(str::to_string))
            .collect()
    };
    assert_eq!(attrs(&one).len(), 5);
    for attr in attrs(&one) {
        assert!(!attrs(&two).contains(&attr), "{attr} appears on both pages");
    }
    assert_eq!(one["total"], two["total"]);

    // The same for the full tier.
    let entries: Vec<(String, String)> = (0..12)
        .map(|index| (format!("pkg{index:02}"), format!("pkg{index:02}")))
        .collect();
    let entries: Vec<(&str, &str)> = entries
        .iter()
        .map(|(attr, name)| (attr.as_str(), name.as_str()))
        .collect();
    let fixture = Fixture::new(&entries, Some(&lock()));
    let first = fixture.run(&[
        "search",
        "",
        "--full",
        "--json",
        "--per-page",
        "5",
        "--page",
        "1",
    ]);
    let second = fixture.run(&[
        "search",
        "",
        "--full",
        "--json",
        "--per-page",
        "5",
        "--page",
        "2",
    ]);
    let one = json(&first);
    let two = json(&second);
    assert_eq!(one["total"], 12, "{}", stderr(&first));
    assert_eq!(attrs(&one).len(), 5);
    for attr in attrs(&one) {
        assert!(!attrs(&two).contains(&attr), "{attr} appears on both pages");
    }
}

/// Both tiers emit the same keys, with `null` where the tier has no data.
#[test]
fn json_is_stable_for_both_tiers() {
    let fixture = Fixture::new(&[("hello", "hello")], Some(&lock()));
    for args in [
        vec!["search", "firefox", "--json"],
        vec!["search", "hello", "--full", "--json"],
    ] {
        let output = fixture.run(&args);
        let value = json(&output);
        for key in [
            "schema", "query", "source", "page", "per_page", "total", "pages", "hits",
        ] {
            assert!(value.get(key).is_some(), "{key} missing in {value}");
        }
        for key in ["kind", "reference", "revision", "pinned"] {
            assert!(
                value["source"].get(key).is_some(),
                "source.{key} missing in {value}"
            );
        }
        let hit = &value["hits"][0];
        for key in [
            "attr",
            "name",
            "version",
            "description",
            "category",
            "license",
            "platform",
        ] {
            assert!(hit.get(key).is_some(), "hit.{key} missing in {hit}");
        }
    }
}

/// Zero results are a successful search, not an error — on both tiers.
#[test]
fn a_search_with_no_hits_is_not_an_error() {
    let fixture = Fixture::new(&[], None);
    let output = fixture.run(&["search", "zzzznotapackage", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value = json(&output);
    assert_eq!(value["total"], 0);
    assert_eq!(value["pages"], 0);
    assert_eq!(value["page"], 1);
    assert_eq!(value["hits"].as_array().expect("hits").len(), 0);

    // The full tier must not fall back to "return everything" for a real query.
    let fixture = Fixture::new(&[("hello", "hello")], Some(&lock()));
    let output = fixture.run(&["search", "zzzznotapackage", "--full", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value = json(&output);
    assert_eq!(value["total"], 0, "{value}");
    assert_eq!(value["hits"].as_array().expect("hits").len(), 0);
}

/// Flags that only mean something for `--full` are usage errors without it.
#[test]
fn full_only_flags_without_full_are_usage_errors() {
    let fixture = Fixture::new(&[], None);
    for args in [
        vec!["search", "firefox", "--flake-ref", "github:NixOS/nixpkgs/x"],
        vec!["search", "firefox", "--timeout", "60"],
        vec!["search", "firefox", "--timeout", "0", "--full"],
        vec!["search", "firefox", "--page", "0"],
        vec!["search", "firefox", "--per-page", "0"],
        vec!["search"],
    ] {
        let output = fixture.run(&args);
        assert_eq!(
            output.status.code(),
            Some(1),
            "`{args:?}` must be a usage error: {}",
            stderr(&output)
        );
    }
}

/// A state directory that cannot be created is reported, not silently skipped.
#[test]
fn a_storage_failure_is_reported() {
    let fixture = Fixture::new(&[("hello", "hello")], Some(&lock()));
    let blocker = fixture.dir.path().join("blocker");
    fs::write(&blocker, "not a directory").expect("write blocker");
    let state = blocker.join("state");
    let output = fixture.run(&[
        "search",
        "hello",
        "--full",
        "--json",
        "--state-dir",
        state.to_str().expect("utf-8"),
    ]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("cannot use"),
        "the storage error must be reported: {}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "", "--json must not emit partial JSON");
}

/// An unpinned source is labelled as such, in the progress line and in JSON.
#[test]
fn an_unpinned_source_is_labelled() {
    let fixture = Fixture::new(&[("hello", "hello")], None);
    let output = fixture.run(&["search", "hello", "--full", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value = json(&output);
    assert_eq!(value["source"]["kind"], "unpinned-default");
    assert_eq!(value["source"]["pinned"], false);
    assert_eq!(value["source"]["reference"], "nixpkgs");
    assert!(
        stderr(&output).contains("not a pinned revision"),
        "{}",
        stderr(&output)
    );
}

/// The human output mentions the page footer and no JSON.
#[test]
fn the_human_output_is_not_json() {
    let fixture = Fixture::new(&[], None);
    let output = fixture.run(&["search", "firefox"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("result(s), page 1/"), "{text}");
    assert!(!text.trim_start().starts_with('{'), "{text}");
    assert!(text.contains("firefox"), "{text}");
}
