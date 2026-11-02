//! Nix Home Studio — the desktop application.
//!
//! Layering, from the bottom up: `nhs-core` owns the rules about files and
//! processes, `nhs-app` owns the flow and the safety gate, `nhs-adapter` performs
//! the work, and this crate is a thin shell: it translates JSON into events, runs
//! exactly the intents this milestone allows, and renders the resulting state in a
//! Tauri window. No safety rule is duplicated here.

pub mod bridge;
pub mod check;
pub mod flowcheck;
pub mod runtime;
pub mod service;
