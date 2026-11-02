//! Every user-visible string, in one place.
//!
//! v1 is English only and there is no i18n framework: a table of constants is
//! enough, and it keeps the wording reviewable. When a second language is added,
//! this module is the only thing that has to change shape.

/// Shown wherever activation is mentioned.
pub const SWITCH_DISABLED: &str =
    "Activating the configuration (`switch`) is not available in this version. \
     This build only validates and writes files.";

/// Apply is deliberately build-only.
pub const APPLY_IS_BUILD_ONLY: &str =
    "Applying writes the reviewed files. It does not activate them.";

/// The warning that must be visible on the confirmation screen.
pub const CONFIRM_WARNING: &str =
    "This writes the files listed above to your configuration. Review the diff first: \
     the files are replaced atomically and a backup is kept, but the change is real.";

pub const EMPTY_SELECTION: &str = "Select at least one package before building a plan.";

pub const NO_PLAN: &str = "Build a plan first.";

pub const PLAN_NOT_READY: &str = "The plan is still being built.";

pub const PLAN_STALE: &str = "The selection changed, so the plan was discarded. Build it again.";

pub const NOT_VALIDATED: &str = "Validate the plan before applying it.";

pub const VALIDATION_FAILED: &str =
    "The last validation failed, so the plan cannot be applied. Fix the problem and validate \
     again.";

pub const VALIDATION_STALE: &str =
    "The plan changed after validation, so the result no longer applies.";

pub const DIFF_NOT_REVIEWED: &str = "Review the diff and confirm that you have read it.";

pub const NOT_CONFIRMED: &str = "Apply needs an explicit confirmation on the review screen.";

pub const LAYOUT_NOT_EDITABLE: &str =
    "This configuration cannot be edited by this application: it is either read-only here or \
     there is no configuration file.";

pub const ALREADY_APPLIED: &str =
    "This exact plan has already been written. Change the selection to make a new one.";

pub const NIXOS_READ_ONLY: &str =
    "A NixOS-integrated configuration is shown read-only; this application never edits it.";

pub const HISTORY_READ_ONLY: &str =
    "Generations and past snapshots are listed for reference. Rolling back is not available in \
     this version.";

pub const SEARCH_EMPTY: &str = "No packages match. Try a different word, or clear the filters.";

pub const APPLY_RUNNING: &str = "Writing the reviewed files…";

pub const APPLY_SUCCEEDED: &str = "The files were written. Nothing was activated.";

pub const APPLY_FAILED: &str = "The files could not be written. Nothing was activated.";

pub const BUILD_RUNNING: &str = "Building the plan in a scratch copy…";

/// Name of the application, used in window titles and headings.
pub const APP_NAME: &str = "Nix Home Studio";
