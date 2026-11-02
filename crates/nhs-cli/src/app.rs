//! `plan`, `validate` and `apply`.
//!
//! The orchestration lives here rather than in the core because it is policy for
//! a human at a terminal: which layout may be edited, what to print, which exit
//! code to use. Everything that must be provable — module rendering, import
//! injection, the edit engine, the build, the classifier — is in `nhs-core` and
//! tested there.
//!
//! Two rules shape this file:
//!
//! * `apply` never activates anything. `--switch` is refused before a single
//!   file is read, and the refusal prints the exact command it would have run.
//! * a refusal is not a failure. Exit code 3 means "this application will not do
//!   that", 1 means "something broke", 2 means "nothing to do" — a UI (or a
//!   script) can tell them apart without parsing English.

use std::path::PathBuf;

use nhs_core::discovery::detect;
use nhs_core::edit::{EditEngine, StateLayout};
use nhs_core::module::{Scope, Setting, SettingValue};
use nhs_core::proc::{CancelToken, RealRunner, Stream};
use nhs_core::validation::{validate, ValidationError, ValidationReport};
use nhs_core::{EnvView, RealFs};

/// What the process exits with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// The command did what it said.
    Ok,
    /// Something failed: a build, an I/O error, a bad argument.
    Failed,
    /// Nothing needed doing; no files would change.
    NoChanges,
    /// A policy refusal: this application will not do that.
    Refused,
}

impl Exit {
    pub fn code(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Failed => 1,
            Self::NoChanges => 2,
            Self::Refused => 3,
        }
    }
}

/// A refusal or failure, with the message to show.
pub struct Failure {
    pub exit: Exit,
    pub message: String,
}

impl Failure {
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            exit: Exit::Failed,
            message: message.into(),
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            exit: Exit::Failed,
            message: message.into(),
        }
    }

    pub fn refused(message: impl Into<String>) -> Self {
        Self {
            exit: Exit::Refused,
            message: message.into(),
        }
    }
}

/// Parsed options for `plan`, `validate` and `apply`.
///
/// A flag struct is exactly what a CLI has; splitting it into themed structs
/// would only move the flags somewhere else.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default)]
pub struct Options {
    pub home: Option<PathBuf>,
    pub module: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
    pub packages: Vec<String>,
    pub settings: Vec<Setting>,
    pub json: bool,
    pub build_only: bool,
    pub switch: bool,
    pub yes: bool,
}

impl Options {
    /// Parse the flags after the subcommand name.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self::default();
        let mut index = 0;
        while let Some(arg) = args.get(index) {
            index += 1;
            match arg.as_str() {
                "--json" => options.json = true,
                "--build-only" => options.build_only = true,
                "--switch" => options.switch = true,
                "--yes" => options.yes = true,
                "--home" => options.home = Some(PathBuf::from(next_value(args, &mut index, arg)?)),
                "--module" => {
                    options.module = Some(PathBuf::from(next_value(args, &mut index, arg)?));
                }
                "--state-dir" => {
                    options.state_dir = Some(PathBuf::from(next_value(args, &mut index, arg)?));
                }
                "--packages" => {
                    let value = next_value(args, &mut index, arg)?;
                    for name in value.split(',') {
                        let name = name.trim();
                        if !name.is_empty() {
                            options.packages.push(name.to_string());
                        }
                    }
                }
                "--set" => {
                    let value = next_value(args, &mut index, arg)?;
                    options.settings.push(parse_setting(&value)?);
                }
                other => return Err(format!("unexpected argument `{other}`")),
            }
        }
        if options.yes && !options.switch {
            return Err("`--yes` only makes sense with `--switch`".to_string());
        }
        Ok(options)
    }
}

fn next_value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    let value = args
        .get(*index)
        .cloned()
        .ok_or_else(|| format!("`{flag}` needs a value"))?;
    *index += 1;
    Ok(value)
}

/// `--set programs.git.enable=true` — the value is JSON, never a Nix expression.
fn parse_setting(raw: &str) -> Result<Setting, String> {
    let (path, value) = raw
        .split_once('=')
        .ok_or_else(|| format!("`--set {raw}` needs the form path=<json>"))?;
    let parsed: serde_json::Value = serde_json::from_str(value)
        .map_err(|error| format!("`--set {path}`: the value is not JSON ({error})"))?;
    let value = match parsed {
        serde_json::Value::Bool(flag) => SettingValue::Bool(flag),
        serde_json::Value::Number(number) => SettingValue::Int(
            number
                .as_i64()
                .ok_or_else(|| format!("`--set {path}`: only integers are supported"))?,
        ),
        serde_json::Value::String(text) => SettingValue::Str(text),
        serde_json::Value::Array(items) => {
            let mut strings = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    serde_json::Value::String(text) => strings.push(text),
                    _ => return Err(format!("`--set {path}`: a list may only contain strings")),
                }
            }
            SettingValue::StrList(strings)
        }
        _ => {
            return Err(format!(
                "`--set {path}`: only booleans, integers, strings and lists of strings are \
                 supported"
            ))
        }
    };
    Ok(Setting {
        path: path.trim().to_string(),
        value,
        // The CLI cannot express a NixOS-only option; the UI that knows a
        // setting's scope does that through the core type.
        scope: Scope::Home,
    })
}

/// A plan ready to be shown, validated or applied.
///
/// The type lives in `nhs-core::flow` because the GUI builds plans the same way;
/// keeping a second copy here would have meant two implementations of the same
/// safety rules.
pub use nhs_core::flow::Prepared;

/// Build the plan for a selection: render or rewrite the sidecar, add the
/// import, stage both.
pub fn prepare(options: &Options) -> Result<Prepared, Failure> {
    let mut env = EnvView::from_process();
    if let Some(home) = &options.home {
        env.home.clone_from(home);
    }
    // `--state-dir` names the *final* directory: no `nix-home-studio` suffix is
    // appended, because a flag that says "state dir" should be the state dir.
    let state_dir = options
        .state_dir
        .clone()
        .unwrap_or_else(|| env.state_home().join("nix-home-studio"));

    let discovery = detect(&RealFs, &env);
    let selection = nhs_core::flow::Selection {
        packages: options.packages.clone(),
        settings: options.settings.clone(),
    };
    nhs_core::flow::prepare(
        &RealFs,
        &discovery.layout,
        &state_dir,
        options.module.as_deref(),
        &selection,
    )
    .map_err(|error| {
        let message = error.to_string();
        if error.is_refusal() {
            Failure::refused(message)
        } else {
            Failure::failed(message)
        }
    })
}

/// Classify a commit failure: a policy refusal is not a crash.
fn refusal_from_edit(error: &nhs_core::EditError) -> Failure {
    match error {
        nhs_core::EditError::Conflict { .. }
        | nhs_core::EditError::InvalidNix { .. }
        | nhs_core::EditError::InconsistentPlan { .. }
        | nhs_core::EditError::NoopNotOnDisk { .. }
        | nhs_core::EditError::DuplicatePath { .. }
        | nhs_core::EditError::AlreadyExists { .. }
        | nhs_core::EditError::ForbiddenTarget { .. }
        | nhs_core::EditError::DanglingSymlink { .. }
        | nhs_core::EditError::StateInsideConfig { .. } => Failure::refused(error.to_string()),
        _ => Failure::failed(error.to_string()),
    }
}

/// `nhs plan`: show the diff, write nothing.
pub fn run_plan(options: &Options) -> Result<Exit, Failure> {
    let prepared = prepare(options)?;
    if prepared.plan.is_empty() {
        if options.json {
            print_json(&serde_json::json!({ "schema": 1, "changes": [] }))?;
        } else {
            println!("nothing to do: the configuration already matches the selection");
        }
        return Ok(Exit::NoChanges);
    }

    if options.json {
        let changes: Vec<serde_json::Value> = prepared
            .plan
            .changes
            .iter()
            .map(|change| {
                serde_json::json!({
                    "path": change.path,
                    "kind": match change.kind {
                        nhs_core::ChangeKind::Create => "create",
                        nhs_core::ChangeKind::Replace => "replace",
                    },
                    "added_lines": change.added_lines,
                    "removed_lines": change.removed_lines,
                    "unified_diff": change.unified_diff,
                    "resolved_from": change.resolved_from,
                })
            })
            .collect();
        print_json(&serde_json::json!({
            "schema": 1,
            "changes": changes,
            "notes": prepared.plan.notes,
        }))?;
    } else {
        for change in &prepared.plan.changes {
            if change.is_noop() {
                continue;
            }
            println!("{}:", change.path.display());
            if let Some(link) = &change.resolved_from {
                println!("  (through the symlink {})", link.display());
            }
            print!("{}", change.unified_diff);
            if !change.unified_diff.ends_with('\n') {
                println!();
            }
        }
        for note in &prepared.plan.notes {
            println!("note: {note}");
        }
    }
    Ok(Exit::Ok)
}

/// `nhs validate`: build the plan in a scratch copy.
pub fn run_validate(options: &Options) -> Result<Exit, Failure> {
    let prepared = prepare(options)?;
    if prepared.plan.is_empty() {
        if options.json {
            print_json(&serde_json::json!({ "schema": 1, "ok": true, "changes": 0 }))?;
        } else {
            println!("nothing to do: the configuration already matches the selection");
        }
        return Ok(Exit::NoChanges);
    }

    let report = validate_plan(&prepared, options)?;
    print_validation(&report, options)?;
    Ok(if report.success {
        Exit::Ok
    } else {
        Exit::Failed
    })
}

fn validate_plan(prepared: &Prepared, options: &Options) -> Result<ValidationReport, Failure> {
    let env = EnvView::from_process();
    let runner = RealRunner::new(&env);
    let cancel = CancelToken::new();
    let mut sink = |stream: Stream, line: &str| {
        if !options.json {
            match stream {
                Stream::Stdout => println!("{line}"),
                Stream::Stderr => eprintln!("{line}"),
            }
        }
    };
    validate(
        &RealFs,
        &prepared.layout,
        &prepared.plan,
        &prepared.state_dir,
        &runner,
        &cancel,
        &mut sink,
    )
    .map_err(|error| match error {
        ValidationError::UnsupportedLayout { .. } => Failure::refused(error.to_string()),
        _ => Failure::failed(error.to_string()),
    })
}

fn print_validation(report: &ValidationReport, options: &Options) -> Result<(), Failure> {
    if options.json {
        let diagnosis = report.diagnosis.as_ref();
        print_json(&serde_json::json!({
            "schema": 1,
            "ok": report.success,
            "command": report.command,
            "kind": diagnosis.map(|d| d.kind.label()),
            "summary": diagnosis.map(|d| d.summary.clone()),
            "hint": diagnosis.map(|d| d.hint),
            "location": diagnosis.and_then(|d| d.location.clone()),
            "scratch": report.kept_scratch,
            "record": report.record,
            "elapsed_ms": report.elapsed.as_millis(),
        }))?;
        return Ok(());
    }
    println!("command: {}", report.command);
    if report.success {
        println!("validation passed in {} ms", report.elapsed.as_millis());
        return Ok(());
    }
    match &report.diagnosis {
        Some(diagnosis) => {
            println!("validation failed ({})", diagnosis.kind.label());
            println!("  {}", diagnosis.summary);
            if let Some(location) = &diagnosis.location {
                println!("  at {location}");
            }
            println!("  what to do: {}", diagnosis.hint);
        }
        None => println!("validation failed"),
    }
    if let Some(scratch) = &report.kept_scratch {
        println!("  the copy was kept for inspection: {}", scratch.display());
    }
    if let Some(record) = &report.record {
        println!("  recorded in {}", record.display());
    }
    Ok(())
}

/// `nhs apply`: validate, then write. Never activates.
pub fn run_apply(options: &Options) -> Result<Exit, Failure> {
    if options.switch {
        // Refused here, before the environment is read and before any path is
        // looked at: no `detect`, no file read, no process. `--yes` does not
        // unlock it, and the message is static, so its text cannot depend on what
        // is on disk.
        return Err(Failure::refused(
            "activation is not available in this version: no file was read and nothing was \
             changed.\nWhen it lands it will run `home-manager switch` for your layout:\n  \
             flake:   home-manager switch --flake .#<attribute>\n  classic: home-manager switch \
             -f <main file>\nRun `nhs discover` to see which one applies to this machine.",
        ));
    }

    let prepared = prepare(options)?;
    if prepared.plan.is_empty() {
        if options.json {
            print_json(&serde_json::json!({ "schema": 1, "applied": false, "changes": 0 }))?;
        } else {
            println!("nothing to do: the configuration already matches the selection");
        }
        return Ok(Exit::NoChanges);
    }

    // Validate before writing: a failing build must not touch the real tree.
    let report = validate_plan(&prepared, options)?;
    if !report.success {
        print_validation(&report, options)?;
        return Err(Failure::failed(
            "the build failed, so nothing was written to your configuration",
        ));
    }
    if !options.json {
        println!("validation passed in {} ms", report.elapsed.as_millis());
    }

    let engine = EditEngine::new(
        &RealFs,
        StateLayout {
            state_dir: prepared.state_dir.clone(),
            config_root: prepared.config_root.clone(),
        },
    );
    let applied = engine.commit(&prepared.plan).map_err(|error| {
        let exit = refusal_from_edit(&error).exit;
        let message = error.to_string();
        match exit {
            Exit::Refused => Failure::refused(message),
            _ => Failure::failed(message),
        }
    })?;

    if options.json {
        print_json(&serde_json::json!({
            "schema": 1,
            "applied": true,
            "snapshot_id": applied.snapshot_id,
            "written": applied.written,
            "warning": applied.warning,
            "activated": false,
        }))?;
    } else {
        println!("applied snapshot {}", applied.snapshot_id);
        for path in &applied.written {
            println!("  wrote {}", path.display());
        }
        if let Some(warning) = &applied.warning {
            println!("warning: {warning}");
        }
        println!("no activation was performed: `switch` is not available in this version");
    }
    Ok(Exit::Ok)
}

fn print_json(payload: &serde_json::Value) -> Result<(), Failure> {
    let rendered = serde_json::to_string_pretty(payload)
        .map_err(|error| Failure::failed(error.to_string()))?;
    println!("{rendered}");
    Ok(())
}
