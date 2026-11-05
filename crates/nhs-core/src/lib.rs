//! Headless core for nix-home-studio.
//!
//! M1 surface: an injectable filesystem/environment view ([`FsView`],
//! [`EnvView`]), layout discovery ([`detect`]) and toolchain resolution
//! ([`resolve_toolchain`]).
//!
//! Hard invariants of this crate:
//!
//! * it never prints to standard output or standard error, and never reads
//!   standard input;
//! * every child process must be created and managed through [`proc`]; the
//!   executable is restricted to an explicit allowlist, arguments are built by
//!   internal logic and never interpreted by a shell;
//! * configuration validation only ever runs against an isolated scratch copy
//!   and must not modify the real configuration or activate the system;
//! * every mutation goes through [`edit::EditEngine::commit`]; [`edit`] is the
//!   only module that writes, and it writes only through [`fs::WriteFs`];
//! * the only module that touches process environment variables is
//!   [`env`] ([`EnvView::from_process`]).
//!
//! Any change that widens execution rights must update the safety gates in
//! `tests/safety.rs` and their tests.

#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::option_if_let_else,
    clippy::return_self_not_must_use,
    clippy::struct_excessive_bools,
    clippy::too_many_lines
)]

pub mod catalog;
pub mod classify;
pub mod discovery;
pub mod donated;
pub mod edit;
pub mod env;
pub mod flow;
pub mod fs;
pub mod fullcatalog;
pub mod import;
pub mod module;
pub mod proc;
pub mod revision;
pub mod rollback;
pub mod switch;
pub mod toolchain;
pub mod validation;

pub use discovery::{detect, Discovery, DiscoveryAttempt, GenerationLink, Layout};
pub use edit::hash::Sha256;
pub use edit::{Applied, ChangeKind, EditEngine, EditError, FileChange, Plan, StateLayout};
pub use env::EnvView;
pub use flow::{prepare, PrepareError, Prepared, Selection};
pub use fs::{FsView, MemFs, RealFs, WriteFs};
pub use import::{inject_sidecar, ImportError, DEFAULT_SIDECAR};
pub use module::{
    check_target, system_only_paths, ModuleError, ModuleSpec, Scope, Setting, SettingValue, Style,
    Target,
};
pub use proc::{CommandSpec, Program, Runner};
pub use revision::{resolve as resolve_source, Resolution, RevisionError, SourceKind};
pub use switch::{switch_command, switch_preview, Confirmation, SwitchError};
pub use toolchain::{resolve_toolchain, HmInvocation, Toolchain};
pub use validation::{validate, ValidationError, ValidationReport};
