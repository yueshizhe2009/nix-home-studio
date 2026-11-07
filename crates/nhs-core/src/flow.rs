//! Turning a selection into a reviewable plan.
//!
//! This is the step every front end needs and none of them should reimplement:
//! render or rewrite the sidecar module, add the import line to the user's main
//! file, and stage both so the diff can be shown. It was written for the CLI
//! (M3–M4) and moved here in M6.2 so the GUI calls the same code instead of a
//! copy — the alternative was two implementations of the same safety rules.
//!
//! Nothing here writes: [`EditEngine::stage`] reads and hashes, and the returned
//! [`Prepared::plan`] is what a front end shows before anything is committed.

use std::io;
use std::path::{Path, PathBuf};

use crate::discovery::Layout;
use crate::edit::{EditEngine, EditError, Plan, StateLayout};
use crate::fs::WriteFs;
use crate::import::{inject_sidecar, ImportError};
use crate::module::{self, ModuleError, ModuleSpec, Setting, Style, Target};

/// Shown when discovery found no Home Manager configuration. It is a hint, not
/// an action: nothing here creates a file.
pub const NO_CONFIG_HINT: &str = "create ~/.config/home-manager/home.nix (classic) or ~/.config/home-manager/flake.nix (flake), or run `home-manager init` to generate a starter configuration";

/// What the user picked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// Package attribute paths, relative to `pkgs`.
    pub packages: Vec<String>,
    pub settings: Vec<Setting>,
}

/// Everything a front end needs after planning.
#[derive(Debug)]
pub struct Prepared {
    pub layout: Layout,
    pub config_root: PathBuf,
    pub main_file: PathBuf,
    pub module_file: PathBuf,
    pub state_dir: PathBuf,
    pub plan: Plan,
}

/// Why planning did not produce a plan.
#[derive(Debug)]
pub enum PrepareError {
    /// This layout is not one this application may edit.
    UnsupportedLayout { kind: String, reason: String },
    /// The sidecar module would live outside the configuration tree, so its
    /// import could not be expressed as a relative path.
    ModuleOutsideTree { module: PathBuf, root: PathBuf },
    /// A file could not be read.
    Read { path: PathBuf, source: io::Error },
    /// The module could not be rendered or rewritten safely.
    Module(ModuleError),
    /// The import could not be added safely.
    Import(ImportError),
    /// The plan could not be staged.
    Edit(EditError),
}

impl PrepareError {
    /// True when the answer is "this application will not do that", false when
    /// something broke. Front ends map these to different exit codes or banners.
    #[must_use]
    pub fn is_refusal(&self) -> bool {
        match self {
            Self::Read { .. } => false,
            Self::UnsupportedLayout { .. }
            | Self::ModuleOutsideTree { .. }
            | Self::Module(_)
            | Self::Import(_) => true,
            Self::Edit(error) => matches!(
                error,
                EditError::Conflict { .. }
                    | EditError::InvalidNix { .. }
                    | EditError::StateInsideConfig { .. }
                    | EditError::DanglingSymlink { .. }
                    | EditError::DuplicatePath { .. }
                    | EditError::InconsistentPlan { .. }
                    | EditError::NoopNotOnDisk { .. }
                    | EditError::ForbiddenTarget { .. }
                    | EditError::AlreadyExists { .. }
            ),
        }
    }
}

impl std::fmt::Display for PrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedLayout { kind, reason } => {
                write!(f, "cannot plan a `{kind}` configuration: {reason}")?;
                if kind == "unknown" && reason.contains("no Home Manager configuration found") {
                    write!(f, "\nhint: {NO_CONFIG_HINT}")?;
                }
                Ok(())
            }
            Self::ModuleOutsideTree { module, root } => write!(
                f,
                "{} is outside the configuration tree at {}, so it cannot be imported",
                module.display(),
                root.display()
            ),
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Module(error) => write!(f, "{error}"),
            Self::Import(error) => write!(f, "{error}"),
            Self::Edit(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PrepareError {}

/// Render or rewrite the sidecar, inject the import, and stage both changes.
///
/// `module` overrides the default `<config_root>/modules/nhs-packages.nix`.
pub fn prepare(
    fs: &dyn WriteFs,
    layout: &Layout,
    state_dir: &Path,
    module: Option<&Path>,
    selection: &Selection,
) -> Result<Prepared, PrepareError> {
    let (config_root, main_file) = editable_root(layout)?;
    let module_file = module.map_or_else(
        || config_root.join("modules").join("nhs-packages.nix"),
        Path::to_path_buf,
    );
    // The import line is a relative path, so a module outside the tree has no
    // representable import. Refuse here rather than letting the engine report a
    // forbidden target later.
    if !module_file.starts_with(&config_root) {
        return Err(PrepareError::ModuleOutsideTree {
            module: module_file.clone(),
            root: config_root.clone(),
        });
    }
    let import_path =
        import_path(&main_file, &module_file).ok_or_else(|| PrepareError::ModuleOutsideTree {
            module: module_file.clone(),
            root: config_root.clone(),
        })?;

    let spec = ModuleSpec {
        packages: selection.packages.clone(),
        settings: selection.settings.clone(),
    };
    let target = Target::for_layout(layout);
    module::check_target(&spec, target).map_err(PrepareError::Module)?;

    let engine = EditEngine::new(
        fs,
        StateLayout {
            state_dir: state_dir.to_path_buf(),
            config_root: config_root.clone(),
        },
    );
    // A state directory inside the tree is a layout mistake, not a build problem:
    // refuse it while the user is still looking at a plan.
    engine.check_layout().map_err(PrepareError::Edit)?;

    let mut changes = Vec::new();
    if fs.exists(&module_file) {
        let existing = read_text(fs, &module_file)?;
        let text = module::rewrite(&existing, &spec, target).map_err(PrepareError::Module)?;
        changes.push(
            engine
                .stage(&module_file, &text)
                .map_err(PrepareError::Edit)?,
        );
    } else {
        let text =
            module::render(&spec, &Style::default(), target).map_err(PrepareError::Module)?;
        changes.push(
            engine
                .stage_create(&module_file, &text)
                .map_err(PrepareError::Edit)?,
        );
    }

    let main_text = read_text(fs, &main_file)?;
    let with_import = inject_sidecar(&main_text, &import_path).map_err(PrepareError::Import)?;
    changes.push(
        engine
            .stage(&main_file, &with_import)
            .map_err(PrepareError::Edit)?,
    );

    Ok(Prepared {
        layout: layout.clone(),
        config_root,
        main_file,
        module_file,
        state_dir: state_dir.to_path_buf(),
        plan: engine.plan(changes),
    })
}

/// The configuration root and main file of a layout this application may edit.
fn editable_root(layout: &Layout) -> Result<(PathBuf, PathBuf), PrepareError> {
    match layout {
        Layout::FlakeHome {
            root, main_file, ..
        } => Ok((root.clone(), main_file.clone())),
        Layout::ClassicHome { main_file } => Ok((
            main_file
                .parent()
                .map_or_else(|| PathBuf::from("/"), Path::to_path_buf),
            main_file.clone(),
        )),
        Layout::FlakeNixos { .. } => Err(PrepareError::UnsupportedLayout {
            kind: "flake-nixos".to_string(),
            reason: "NixOS integration is read-only in this version".to_string(),
        }),
        Layout::ProfileOnly { .. } => Err(PrepareError::UnsupportedLayout {
            kind: "profile-only".to_string(),
            reason: "no Home Manager configuration was found, so there is nothing to plan"
                .to_string(),
        }),
        Layout::Unknown { reason } => Err(PrepareError::UnsupportedLayout {
            kind: "unknown".to_string(),
            reason: reason.clone(),
        }),
    }
}

fn read_text(fs: &dyn WriteFs, path: &Path) -> Result<String, PrepareError> {
    fs.read_to_string(path)
        .map_err(|source| PrepareError::Read {
            path: path.to_path_buf(),
            source,
        })
}

/// The `./modules/x.nix` string to put in the user's file.
///
/// Lexical on purpose: both paths come from the same layout, and the result must
/// stay a plain relative path so it can be written into a Nix list.
fn import_path(main_file: &Path, module_file: &Path) -> Option<String> {
    let from = main_file.parent()?;
    let from_parts: Vec<_> = from.components().collect();
    let to_parts: Vec<_> = module_file.components().collect();
    let common = from_parts
        .iter()
        .zip(to_parts.iter())
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from_parts.len() {
        parts.push("..".to_string());
    }
    for component in to_parts.iter().skip(common) {
        parts.push(component.as_os_str().to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        None
    } else {
        Some(format!("./{}", parts.join("/")))
    }
}
