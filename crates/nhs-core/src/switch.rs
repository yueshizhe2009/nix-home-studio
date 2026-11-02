//! The activation command — and the confirmation it cannot be built without.
//!
//! Nothing in this version runs it: `nhs apply --switch` is refused outright
//! (see `main.rs`). The type exists so that when activation is added, the
//! confirmation is a **precondition of building the command**, not a check a
//! caller might forget. A `CommandSpec` for `switch` cannot be obtained without
//! a [`Confirmation`], and a [`Confirmation`] is only produced by
//! [`Confirmation::grant`] — which the CLI deliberately never calls.
//!
//! Gate G4 in `tests/safety.rs` covers the source shape (the CLI must not grant
//! a confirmation); the tests here cover the behaviour (no token → no command).

use std::path::PathBuf;
use std::time::Duration;

use crate::discovery::Layout;
use crate::proc::{CommandSpec, Program};

/// Proof that a human asked for activation, not just for a build.
///
/// Zero-sized on purpose: it is a token, not a setting. There is no `Default`
/// and no way to construct one from a `bool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Confirmation(());

impl Confirmation {
    /// Grant permission to build an activation command.
    ///
    /// Call this only after showing the user the exact command line. A refusal
    /// (a missing `--yes`, a closed dialog) must not reach this function.
    #[must_use]
    pub fn grant() -> Self {
        Self(())
    }
}

/// Why an activation command could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchError {
    /// The user did not confirm, so no command was produced.
    MissingConfirmation,
    /// This layout has nothing to activate in v1.
    UnsupportedLayout { kind: String, reason: String },
}

impl std::fmt::Display for SwitchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingConfirmation => write!(
                f,
                "activation needs an explicit confirmation; print the command and ask first"
            ),
            Self::UnsupportedLayout { kind, reason } => {
                write!(f, "cannot activate a `{kind}` configuration: {reason}")
            }
        }
    }
}

impl std::error::Error for SwitchError {}

/// Build the activation command for a layout.
///
/// `confirmation` must be `Some`; passing `None` is the refused path and returns
/// an error instead of a command. The command is never executed by this crate
/// without a caller that holds a [`Confirmation`].
pub fn switch_command(
    layout: &Layout,
    confirmation: Option<Confirmation>,
) -> Result<CommandSpec, SwitchError> {
    let Some(_confirmation) = confirmation else {
        return Err(SwitchError::MissingConfirmation);
    };
    let (cwd, args) = switch_args(layout)?;
    Ok(CommandSpec::new(Program::HomeManager, cwd)
        .args(args)
        .with_timeout(Duration::from_mins(10)))
}

/// The activation command as text, without building anything executable.
///
/// This is what the CLI shows when it refuses `--switch`: the user sees exactly
/// what *would* run, and no [`Confirmation`] is created, so nothing can run it.
pub fn switch_preview(layout: &Layout) -> Result<String, SwitchError> {
    let (cwd, args) = switch_args(layout)?;
    let mut spec = CommandSpec::new(Program::HomeManager, cwd);
    for arg in args {
        spec = spec.arg(arg);
    }
    Ok(spec.display())
}

/// Working directory and arguments for activation.
fn switch_args(layout: &Layout) -> Result<(PathBuf, Vec<String>), SwitchError> {
    match layout {
        Layout::FlakeHome { root, attr, .. } => Ok((
            root.clone(),
            vec![
                "switch".to_string(),
                "--flake".to_string(),
                format!(".{}{attr}", "#"),
            ],
        )),
        Layout::ClassicHome { main_file } => {
            let root = main_file
                .parent()
                .map_or_else(|| PathBuf::from("/"), std::path::Path::to_path_buf);
            Ok((
                root,
                vec![
                    "switch".to_string(),
                    "-f".to_string(),
                    main_file.display().to_string(),
                ],
            ))
        }
        Layout::FlakeNixos { .. } => Err(SwitchError::UnsupportedLayout {
            kind: "flake-nixos".to_string(),
            reason: "NixOS integration is read-only in this version".to_string(),
        }),
        Layout::ProfileOnly { .. } => Err(SwitchError::UnsupportedLayout {
            kind: "profile-only".to_string(),
            reason: "there is no configuration file to activate".to_string(),
        }),
        Layout::Unknown { reason } => Err(SwitchError::UnsupportedLayout {
            kind: "unknown".to_string(),
            reason: reason.clone(),
        }),
    }
}
