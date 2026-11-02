//! Shared fixture harness for the `nhs-core` integration tests.
//!
//! Everything here builds in-memory `MemFs` trees plus an explicit `EnvView`.
//! No test mutates the process environment and no fixture touches the real
//! user configuration (audit R13 / D5).

#![allow(
    dead_code,
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub mod flaky;

use nhs_core::{EnvView, MemFs};

pub const TEST_HOME: &str = "/home/tester";
/// Virtual store root used by the in-memory fixtures. `MemFs` is not the real
/// filesystem, so this never touches the host's `/nix/store`.
pub const TEST_STORE_ROOT: &str = "/nix/store";
pub const TEST_USER: &str = "tester";

pub const USER_CONFIG_DIR: &str = "/home/tester/.config/home-manager";
pub const STATE_PROFILES_DIR: &str = "/home/tester/.local/state/nix/profiles";

pub const PATH_BINARY: &str = "/home/tester/opt/bin/home-manager";

pub const FLAKE_HOME_NIX: &str = r#"
{
  description = "tester home";
  outputs = { self, home-manager, nixpkgs, ... }: {
    homeConfigurations."tester" = home-manager.lib.homeManagerConfiguration {
      pkgs = nixpkgs.legacyPackages.x86_64-linux;
      modules = [ ];
    };
  };
}
"#;

pub const CLASSIC_HOME_NIX: &str = r#"
{ pkgs, ... }:
{
  home.stateVersion = "24.11";
  home.packages = [ ];
}
"#;

pub const NIXOS_FLAKE_NIX: &str = r#"
{
  description = "tester box";
  outputs = { self, nixpkgs, home-manager, ... }: {
    nixosConfigurations.testerbox = nixpkgs.lib.nixosSystem {
      modules = [
        home-manager.nixosModules.home-manager
        {
          networking.hostName = "testerbox";
          home-manager.users.tester = { };
        }
      ];
    };
  };
}
"#;

/// A fixture tree plus the environment that points at it.
#[derive(Debug, Clone)]
pub struct Fixture {
    pub fs: MemFs,
    pub env: EnvView,
    pub generations: Vec<PathBuf>,
}

impl Fixture {
    pub fn new() -> Self {
        Self {
            fs: MemFs::new(),
            env: EnvView {
                home: PathBuf::from(TEST_HOME),
                user: Some(TEST_USER.to_string()),
                path_entries: vec![PathBuf::from("/usr/bin")],
                xdg_config_home: None,
                xdg_state_home: None,
                store_root: PathBuf::from(TEST_STORE_ROOT),
            },
            generations: Vec::new(),
        }
    }

    pub fn file(mut self, path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        self.fs = self.fs.with_file(path, contents);
        self
    }

    pub fn dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.fs = self.fs.with_dir(path);
        self
    }

    pub fn symlink(mut self, path: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        self.fs = self.fs.with_symlink(path, target);
        self
    }

    pub fn path_entry(mut self, path: impl Into<PathBuf>) -> Self {
        self.env.path_entries.push(path.into());
        self
    }
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

/// `~/.config/home-manager/flake.nix` with `homeConfigurations."tester"`.
pub fn flake_home() -> Fixture {
    Fixture::new()
        .dir(USER_CONFIG_DIR)
        .file(format!("{USER_CONFIG_DIR}/flake.nix"), FLAKE_HOME_NIX)
}

/// `~/.config/home-manager/home.nix` without a flake.
pub fn classic_home() -> Fixture {
    Fixture::new()
        .dir(USER_CONFIG_DIR)
        .file(format!("{USER_CONFIG_DIR}/home.nix"), CLASSIC_HOME_NIX)
}

/// Home Manager inside `/etc/nixos/flake.nix` (read-only detection).
pub fn flake_nixos() -> Fixture {
    Fixture::new()
        .dir("/etc/nixos")
        .file("/etc/nixos/flake.nix", NIXOS_FLAKE_NIX)
}

/// Two plausible configurations at once: a user flake and a NixOS flake.
pub fn ambiguous_layouts() -> Fixture {
    flake_home()
        .dir("/etc/nixos")
        .file("/etc/nixos/flake.nix", NIXOS_FLAKE_NIX)
}

fn add_generation(mut fixture: Fixture, index: u32) -> Fixture {
    let store = generation_path(index);
    // A real Home Manager generation has no `home-manager` command: `bin/` holds
    // only `home-manager-generation`, and `home-path` is the package environment.
    // Reproduce that faithfully so tests cannot depend on an impossible layout.
    fixture = fixture
        .file(format!("{}/activate", store.display()), "#!/bin/sh\n")
        .symlink(
            format!("{}/bin/home-manager-generation", store.display()),
            format!("{}/activate", store.display()),
        )
        .symlink(
            format!("{}/home-path", store.display()),
            format!("/nix/store/hm-path{index}-home-manager-path"),
        )
        .file(
            format!("/nix/store/hm-path{index}-home-manager-path/bin/jq"),
            "#!/bin/sh\n",
        );
    fixture.generations.push(store);
    fixture
}

/// The audit host's real shape: the `home-manager` command lives in its own
/// store derivation, never inside the generation. Also plants the decoys that
/// must not be mistaken for the command.
pub fn with_home_manager_in_store(mut fixture: Fixture) -> Fixture {
    fixture = fixture
        .file(
            "/nix/store/86nwi02p-home-manager-0-unstable-2026-08-06/bin/home-manager",
            "#!/bin/sh\n",
        )
        .file(
            "/nix/store/lxmrgjxp-home-manager-0-unstable-2026-08-06/bin/home-manager",
            "#!/bin/sh\n",
        )
        .file(
            "/nix/store/42ksgk4d-home-manager-0-unstable-2026-08-06.drv",
            "",
        )
        .file(
            "/nix/store/50sdv1f4-home-manager-path/bin/jq",
            "#!/bin/sh\n",
        )
        .file("/nix/store/0wlp74z7-home-manager-files/home-files/x", "")
        .file("/nix/store/4dj7q6qz-home-manager-source/flake.nix", "");
    fixture
}

/// A real-layout machine with several Home Manager builds in the store.
///
/// `entries` is `(store hash prefix, version)`. Order is preserved into the
/// `MemFs` directory listing so tests can prove the choice does not depend on
/// listing order.
pub fn with_home_manager_versions(entries: &[(&str, &str)]) -> Fixture {
    let mut fixture = profile_only();
    for (hash, version) in entries {
        fixture = fixture.file(
            format!("/nix/store/{hash}-home-manager-{version}/bin/home-manager"),
            "#!/bin/sh\n",
        );
    }
    fixture
}

/// A store containing only the near-miss paths, never the command.
pub fn with_home_manager_decoys_only() -> Fixture {
    let mut fixture = profile_only();
    for path in [
        "/nix/store/aaaa-home-manager-generation/bin/home-manager",
        "/nix/store/bbbb-home-manager-path/bin/home-manager",
        "/nix/store/cccc-home-manager-files/bin/home-manager",
        "/nix/store/dddd-home-manager-source/bin/home-manager",
    ] {
        fixture = fixture.file(path, "#!/bin/sh\n");
    }
    fixture = fixture.file("/nix/store/eeee-home-manager-26.11.drv", "");
    fixture
}

pub fn generation_path(index: u32) -> PathBuf {
    PathBuf::from(format!("/nix/store/hm-gen{index}-home-manager-generation"))
}

/// The audit host's shape: no config, a `home-manager` profile symlink and
/// three `home-manager-<n>-link` generations, with `nix` on `/usr/bin`.
pub fn profile_only() -> Fixture {
    let mut fixture = Fixture::new().file("/usr/bin/nix", "");
    for index in 1..=3 {
        fixture = add_generation(fixture, index);
    }
    fixture
        .symlink(
            format!("{STATE_PROFILES_DIR}/home-manager"),
            generation_path(3),
        )
        .symlink(
            format!("{STATE_PROFILES_DIR}/home-manager-1-link"),
            generation_path(1),
        )
        .symlink(
            format!("{STATE_PROFILES_DIR}/home-manager-2-link"),
            generation_path(2),
        )
        .symlink(
            format!("{STATE_PROFILES_DIR}/home-manager-3-link"),
            generation_path(3),
        )
}

/// A `home-manager` binary on `PATH`, plus a generation that must lose.
pub fn toolchain_path_binary() -> Fixture {
    let mut fixture = Fixture::new().file(PATH_BINARY, "#!/bin/sh\n");
    fixture = add_generation(fixture, 9);
    fixture = fixture.symlink(
        format!("{STATE_PROFILES_DIR}/home-manager"),
        generation_path(9),
    );
    fixture.path_entry("/home/tester/opt/bin")
}

/// No HM on `PATH`; only the resolved generation contains it (D5 primary case).
pub fn toolchain_generation() -> Fixture {
    profile_only()
}

/// HM installed as a `nix profile` element, not on `PATH`.
pub fn toolchain_profile_element() -> Fixture {
    Fixture::new().file("/home/tester/.nix-profile/bin/home-manager", "#!/bin/sh\n")
}

/// Nothing installed, but `nix` is available for the `nix run` fallback.
pub fn toolchain_nix_run() -> Fixture {
    Fixture::new().file("/usr/bin/nix", "")
}

/// Nothing at all: every strategy must fail.
pub fn toolchain_absent() -> Fixture {
    Fixture::new()
}

/// A machine where `nixos-rebuild` exists (detection only).
pub fn with_nixos_rebuild() -> Fixture {
    Fixture::new().file("/run/current-system/sw/bin/nixos-rebuild", "#!/bin/sh\n")
}

/// Fake `nix` / `home-manager` executables.
///
/// Each script appends its argv to `<dir>/argv.log`, prints canned
/// stdout/stderr and exits with the requested code. It never invokes Nix and
/// nothing in M1 executes it; M4 will.
pub struct FakeToolchain {
    dir: tempfile::TempDir,
}

impl FakeToolchain {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("create fake toolchain dir"),
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn argv_log(&self) -> PathBuf {
        self.dir.path().join("argv.log")
    }

    pub fn install(&self, name: &str, stdout: &str, stderr: &str, exit_code: i32) -> PathBuf {
        let path = self.dir.path().join(name);
        let argv_log = self.argv_log();
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nprintf '%s\\n' '{stdout}'\nprintf '%s\\n' '{stderr}' >&2\nexit {exit_code}\n",
            argv_log.display()
        );
        fs::write(&path, script).expect("write fake toolchain script");
        let mut permissions = fs::metadata(&path)
            .expect("stat fake toolchain script")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod fake toolchain script");
        path
    }

    /// An `EnvView` whose `PATH` contains only the fake toolchain directory.
    pub fn env(&self) -> EnvView {
        EnvView {
            home: PathBuf::from(TEST_HOME),
            user: Some(TEST_USER.to_string()),
            path_entries: vec![self.dir.path().to_path_buf()],
            xdg_config_home: None,
            xdg_state_home: None,
            store_root: PathBuf::from(TEST_STORE_ROOT),
        }
    }
}

impl Default for FakeToolchain {
    fn default() -> Self {
        Self::new()
    }
}

/// Every Rust source file of `nhs-core`, as `(path, contents)`.
pub fn core_sources() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rust_files(&root, &mut files);
    files
}

/// Every Rust source file of the crate, including integration tests.
pub fn all_crate_sources() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for subdir in ["src", "tests"] {
        collect_rust_files(&root.join(subdir), &mut files);
    }
    files
}

fn collect_rust_files(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            if let Ok(contents) = fs::read_to_string(&path) {
                out.push((path, contents));
            }
        }
    }
}

/// Paths of source files containing `needle`.
pub fn sources_containing(files: &[(PathBuf, String)], needle: &str) -> Vec<String> {
    files
        .iter()
        .filter(|(_, contents)| contents.contains(needle))
        .map(|(path, _)| path.display().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Edit-engine fixtures (M2)
// ---------------------------------------------------------------------------

pub const EDIT_CONFIG_ROOT: &str = "/home/tester/.config/home-manager";
pub const EDIT_STATE_DIR: &str = "/home/tester/.local/state/nix-home-studio";

/// A `StateLayout` whose state dir sits outside the configuration tree.
pub fn edit_layout() -> nhs_core::StateLayout {
    nhs_core::StateLayout {
        state_dir: PathBuf::from(EDIT_STATE_DIR),
        config_root: PathBuf::from(EDIT_CONFIG_ROOT),
    }
}

/// A minimal, valid Home Manager module.
pub fn valid_module(extra: &str) -> String {
    format!("{{ pkgs, ... }}:\n{{\n  home.packages = [ pkgs.fd ];\n{extra}}}\n")
}

/// A fixture with one managed module already on disk.
pub fn edit_fixture() -> MemFs {
    MemFs::new()
        .with_file(
            format!("{EDIT_CONFIG_ROOT}/modules/nhs-packages.nix"),
            valid_module(""),
        )
        .with_file(
            format!("{EDIT_CONFIG_ROOT}/home.nix"),
            "{\n  imports = [ ];\n}\n",
        )
}
