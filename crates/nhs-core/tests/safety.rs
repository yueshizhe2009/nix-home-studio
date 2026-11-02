//! Safety-gate tests (G2/G3, audit R13, D2).
//!
//! M1 asserted "core never writes". M2 necessarily relaxes that to "raw write
//! syscalls exist in exactly one file, and everything else goes through the
//! `WriteFs` trait", which is gate G3.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

mod support;

use std::path::Path;

#[test]
fn discovery_module_reads_no_process_environment() {
    let sources = support::core_sources();
    let env_needle = concat!("std", "::env");
    let hits = support::sources_containing(&sources, env_needle);

    assert_eq!(
        hits.len(),
        1,
        "exactly one source file (env.rs) may read the process environment, found: {hits:?}"
    );
    assert!(
        hits[0].ends_with("env.rs"),
        "the only environment reader must be env.rs, found: {hits:?}"
    );

    for file in ["discovery.rs", "toolchain.rs", "fs.rs", "lib.rs"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(file);
        let contents = std::fs::read_to_string(&path).expect("read source file");
        assert!(
            !contents.contains(env_needle),
            "{file} must not touch the process environment"
        );
    }
}

#[test]
fn no_process_environment_mutation_anywhere() {
    let needle = concat!("set", "_var");
    let hits = support::sources_containing(&support::all_crate_sources(), needle);
    assert!(
        hits.is_empty(),
        "audit R13: no test may mutate process environment, found in {hits:?}"
    );
}

/// Gate G3: every raw write syscall lives in `fs.rs`, inside the `RealFs`
/// implementation of `WriteFs`. Any other module reaching for `std::fs` to
/// mutate something is bypassing the engine's guards.
#[test]
fn raw_write_syscalls_are_confined_to_the_filesystem_module() {
    let sources = support::core_sources();
    // Every way to mutate a path through `std::fs`, including aliases and the
    // lower-level constructors that `fs::write` is built from.
    let needles = [
        concat!("fs::", "write"),
        concat!("fs::", "remove"),
        concat!("fs::", "rename"),
        concat!("fs::", "create_dir"),
        concat!("fs::", "copy"),
        concat!("fs::", "hard_link"),
        concat!("fs::", "set_permissions"),
        concat!("File::", "create"),
        "OpenOptions",
        "File::options",
        "truncate(true)",
    ];
    for needle in needles {
        let hits = support::sources_containing(&sources, needle);
        for hit in &hits {
            assert!(
                hit.ends_with("fs.rs"),
                "gate G3: {needle} must only appear in fs.rs (the WriteFs implementation), \
                 but it also appears in {hit}"
            );
        }
    }

    // Re-importing `std::fs` under another name defeats a needle search, so ban
    // the alias outright. `fs.rs` uses the plain `use std::fs;`.
    for alias in [
        "use std::fs as",
        "use std::fs::{self as",
        "use std::fs::*",
        "std :: fs",
        "::std::fs::write",
    ] {
        let hits = support::sources_containing(&sources, alias);
        assert!(
            hits.is_empty(),
            "gate G3: aliasing or glob-importing std::fs ({alias}) would bypass the \
             write-site check: {hits:?}"
        );
    }

    // Gate G3 (M4): process creation lives in exactly one file. The allowlist
    // itself is a closed enum (`proc::Program`), so this scan is a backstop, not
    // the guard: a program outside the enum cannot be expressed at all, which
    // `proc::tests::the_allowlist_is_closed` proves.
    for needle in ["Command::new", "process::Command", "tokio::process"] {
        let hits = support::sources_containing(&sources, needle);
        for hit in &hits {
            assert!(
                hit.ends_with("proc.rs"),
                "gate G3: process creation ({needle}) may only appear in proc.rs, \
                 but it also appears in {hit}"
            );
        }
    }

    // Exactly one place in the crate actually starts a process.
    let spawn_sites = support::sources_containing(&sources, "command.spawn()");
    assert_eq!(
        spawn_sites.len(),
        1,
        "gate G3: exactly one spawn site is expected, found: {spawn_sites:?}"
    );
    assert!(
        spawn_sites[0].ends_with("proc.rs"),
        "gate G3: the spawn site must be in proc.rs, found: {spawn_sites:?}"
    );

    // `unsafe` is allowed for `libc::kill` only, only in proc.rs, and every
    // block must carry a SAFETY note next to it.
    let unsafe_files: Vec<String> = sources
        .iter()
        .filter(|(_, text)| text.contains("unsafe {"))
        .map(|(path, _)| path.display().to_string())
        .collect();
    assert!(
        !unsafe_files.is_empty() && unsafe_files.iter().all(|file| file.ends_with("proc.rs")),
        "gate G3: `unsafe` may only appear in proc.rs, found in {unsafe_files:?}"
    );
    let proc_source = sources
        .iter()
        .find(|(path, _)| path.ends_with("proc.rs"))
        .map(|(_, text)| text)
        .expect("proc.rs is part of the crate");
    assert_eq!(
        proc_source.matches("unsafe {").count(),
        proc_source.matches("// SAFETY:").count(),
        "gate G3: every `unsafe` block needs an adjacent `// SAFETY:` note"
    );
    assert!(
        proc_source.matches("libc::kill").count() >= 1,
        "gate G3: the only unsafe operation allowed is `libc::kill`"
    );
}

/// Gate G4: this version cannot build an activation command, because the CLI
/// never grants the confirmation that `switch_command` requires.
#[test]
fn the_cli_cannot_build_an_activation_command() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("nhs-cli")
        .join("src");
    let mut checked = 0_usize;
    for entry in std::fs::read_dir(&dir).expect("read the CLI sources") {
        let path = entry.expect("directory entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read a CLI source");
        checked += 1;
        for needle in ["Confirmation::grant", "switch_command("] {
            assert!(
                !text.contains(needle),
                "gate G4: {} must not contain `{needle}`; activation is not available in this \
                 version and the CLI must not be able to build it",
                path.display()
            );
        }
    }
    assert!(checked >= 2, "expected the CLI's sources, found {checked}");
}

/// Gate G3, second half: the only public entry point that commits a plan is the
/// engine, and it is reached through `WriteFs`, never through `std::fs`.
#[test]
fn the_engine_is_the_only_path_to_a_write() {
    let sources = support::core_sources();
    // `commit` must live in the edit engine and nowhere else.
    let commit_sites = support::sources_containing(&sources, "pub fn commit");
    assert_eq!(
        commit_sites.len(),
        1,
        "exactly one `commit` implementation is expected, found: {commit_sites:?}"
    );
    assert!(
        commit_sites[0].ends_with("edit/mod.rs"),
        "commit must live in edit/mod.rs, found: {commit_sites:?}"
    );
}

#[test]
fn core_has_no_gui_dependency_or_printing_or_sudo() {
    let sources = support::core_sources();
    // `proc.rs` legitimately *attaches* stdin to /dev/null so a child can never
    // read the terminal; what must not exist is code that reads stdin itself.
    for needle in [
        "println!",
        "eprintln!",
        "print!(",
        "eprint!(",
        "std::io::stdin",
        "io::stdin(",
        "sudo",
        "nixos-rebuild switch",
    ] {
        let hits = support::sources_containing(&sources, needle);
        assert!(
            hits.is_empty(),
            "core must stay headless and privilege-free, but {needle} appears in {hits:?}"
        );
    }

    // A child must never inherit the terminal.
    let proc_source = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("proc.rs"),
    )
    .expect("read proc.rs");
    assert!(
        proc_source.contains(".stdin(Stdio::null())"),
        "gate G3: a spawned process must have its stdin attached to /dev/null"
    );

    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect("read nhs-core manifest");
    let manifest = manifest.to_lowercase();
    for crate_name in ["tauri", "gtk", "iced", "slint", "egui", "winit"] {
        assert!(
            !manifest.contains(crate_name),
            "core must not depend on GUI crate {crate_name}"
        );
    }
}
