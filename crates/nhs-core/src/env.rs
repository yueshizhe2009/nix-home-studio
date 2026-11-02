//! Process environment snapshot.
//!
//! [`EnvView`] is the *only* place in this crate that reads process environment
//! variables. Discovery and toolchain logic take an `&EnvView`, which is what
//! makes them testable without mutating the process environment (audit R13).

use std::path::PathBuf;

/// Snapshot of the environment inputs that discovery and toolchain resolution
/// need. Tests and the CLI build this explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvView {
    /// Value of `HOME`. Empty when unset.
    pub home: PathBuf,
    /// Value of `USER` (fallback `LOGNAME`), empty values dropped.
    pub user: Option<String>,
    /// `PATH` split into entries, empty entries dropped.
    pub path_entries: Vec<PathBuf>,
    /// `XDG_CONFIG_HOME` when set and non-empty.
    pub xdg_config_home: Option<PathBuf>,
    /// `XDG_STATE_HOME` when set and non-empty.
    pub xdg_state_home: Option<PathBuf>,
    /// Nix store root. `/nix/store` in production; tests point this at a
    /// fixture directory so they never inspect the real store.
    pub store_root: PathBuf,
}

impl EnvView {
    /// Read the real process environment. The only `std::env` access in the
    /// crate.
    #[must_use]
    pub fn from_process() -> Self {
        Self {
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default(),
            user: std::env::var("USER")
                .ok()
                .or_else(|| std::env::var("LOGNAME").ok())
                .filter(|user| !user.is_empty()),
            path_entries: std::env::var_os("PATH")
                .map(|path| {
                    std::env::split_paths(&path)
                        .filter(|entry| !entry.as_os_str().is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            xdg_config_home: non_empty_path(std::env::var_os("XDG_CONFIG_HOME")),
            xdg_state_home: non_empty_path(std::env::var_os("XDG_STATE_HOME")),
            store_root: non_empty_path(std::env::var_os(STORE_ROOT_ENV))
                .unwrap_or_else(|| PathBuf::from(DEFAULT_STORE_ROOT)),
        }
    }

    /// `$XDG_CONFIG_HOME`, defaulting to `$HOME/.config`.
    #[must_use]
    pub fn config_home(&self) -> PathBuf {
        self.xdg_config_home
            .clone()
            .unwrap_or_else(|| self.home.join(".config"))
    }

    /// `$XDG_STATE_HOME`, defaulting to `$HOME/.local/state`.
    #[must_use]
    pub fn state_home(&self) -> PathBuf {
        self.xdg_state_home
            .clone()
            .unwrap_or_else(|| self.home.join(".local").join("state"))
    }
    /// Find `name` on the recorded `PATH`, returning the first executable file.
    ///
    /// Resolution uses the recorded entries rather than the live process
    /// environment, so a test can point at a fixture directory without mutating the
    /// real `PATH` (audit R13).
    #[must_use]
    pub fn find_on_path(&self, name: &str) -> Option<PathBuf> {
        self.path_entries.iter().find_map(|entry| {
            let candidate = entry.join(name);
            let metadata = std::fs::metadata(&candidate).ok()?;
            if !metadata.is_file() {
                return None;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 == 0 {
                    return None;
                }
            }
            Some(candidate)
        })
    }
}

/// Default Nix store root.
pub const DEFAULT_STORE_ROOT: &str = "/nix/store";

/// Overrides [`DEFAULT_STORE_ROOT`]. Exists so acceptance tests can point the
/// store scan at a fixture directory instead of the host's real store.
pub const STORE_ROOT_ENV: &str = "NHS_STORE_ROOT";

fn non_empty_path(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::EnvView;
    use std::path::PathBuf;

    #[test]
    fn xdg_defaults_are_derived_from_home() {
        let env = EnvView {
            home: PathBuf::from("/home/tester"),
            user: Some("tester".to_string()),
            path_entries: Vec::new(),
            xdg_config_home: None,
            xdg_state_home: None,
            store_root: PathBuf::from("/nix/store"),
        };
        assert_eq!(env.config_home(), PathBuf::from("/home/tester/.config"));
        assert_eq!(env.state_home(), PathBuf::from("/home/tester/.local/state"));
    }

    #[test]
    fn xdg_override_wins() {
        let env = EnvView {
            home: PathBuf::from("/home/tester"),
            user: None,
            path_entries: Vec::new(),
            xdg_config_home: Some(PathBuf::from("/tmp/config")),
            xdg_state_home: Some(PathBuf::from("/tmp/state")),
            store_root: PathBuf::from("/nix/store"),
        };
        assert_eq!(env.config_home(), PathBuf::from("/tmp/config"));
        assert_eq!(env.state_home(), PathBuf::from("/tmp/state"));
    }
}
