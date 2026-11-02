//! Module generation: a [`ModuleSpec`] becomes Nix text.
//!
//! Everything here is a pure function of its arguments. Nothing in this module
//! reads or writes a file, and nothing renders a timestamp, a host name or a
//! process id, so the same spec always produces the same bytes — a diff the user
//! approved stays the diff that is written.
//!
//! The generated file is *marker structured* (architecture §4.2): only the
//! regions between the markers are owned by this application, and the markers
//! are ordinary comments so Nix ignores them. Rewriting an existing file is
//! [`crate::module`]'s second job; this part only renders and validates.

use rnix::ast::{self, HasEntry};

use crate::discovery::Layout;
use crate::edit::validate;

/// First line of a generated module.
pub const HEADER: &str = "# Managed by nix-home-studio. Edits inside the markers are overwritten.";

/// Start of the block that lists packages.
pub const MANAGED_START: &str = "# nhs:managed:start";
/// End of the block that lists packages.
pub const MANAGED_END: &str = "# nhs:managed:end";
/// Start of the block that holds option settings.
pub const SETTINGS_START: &str = "# nhs:settings:start";
/// End of the block that holds option settings.
pub const SETTINGS_END: &str = "# nhs:settings:end";

/// Which module system an option belongs to.
///
/// `SystemOnly` options are legal in a NixOS module and meaningless in a Home
/// Manager module; [`crate::module`] refuses to place them in a home layout
/// instead of writing something that silently does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Valid in a Home Manager module.
    Home,
    /// NixOS-only. Never written into a Home Manager module (D2).
    SystemOnly,
}

/// The value side of a setting.
///
/// Deliberately small: every variant can be rendered without guessing, and a
/// caller that needs an arbitrary expression is not served by this API. That is
/// the point — an unrepresentable value is refused rather than spliced in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingValue {
    Bool(bool),
    Int(i64),
    Str(String),
    StrList(Vec<String>),
}

/// One `option.path = value;` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    /// Dotted attribute path, e.g. `programs.git.enable`.
    pub path: String,
    pub value: SettingValue,
    /// Whether this option is allowed in a Home Manager module.
    pub scope: Scope,
}

/// Which module system the generated text is for.
///
/// v1 only ever writes a Home Manager module (D2: NixOS integration is read-only),
/// so this is a policy input rather than a user choice — but it is explicit so a
/// NixOS-only option can never be rendered into a home module by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A Home Manager module.
    Home,
    /// A NixOS module. Not written by v1; the branch exists so the refusal above
    /// it has a defined counterpart.
    Nixos,
}

impl Target {
    /// Map a detected layout to the module system it would be written for.
    #[must_use]
    pub fn for_layout(layout: &Layout) -> Self {
        match layout {
            Layout::FlakeNixos { .. } => Self::Nixos,
            Layout::FlakeHome { .. }
            | Layout::ClassicHome { .. }
            | Layout::ProfileOnly { .. }
            | Layout::Unknown { .. } => Self::Home,
        }
    }

    /// Name used in messages.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Home => "a Home Manager module",
            Self::Nixos => "a NixOS module",
        }
    }
}

/// Options in `spec` that only exist in a NixOS module, sorted.
#[must_use]
pub fn system_only_paths(spec: &ModuleSpec) -> Vec<String> {
    let mut paths: Vec<String> = spec
        .settings
        .iter()
        .filter(|setting| setting.scope == Scope::SystemOnly)
        .map(|setting| setting.path.clone())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Refuse a spec whose options cannot live in `target`, and explain the ones that
/// can only live in a NixOS module.
///
/// This runs before any rendering or file inspection, so an unsupported option is
/// reported instead of being written into a module that would ignore it.
pub fn check_target(spec: &ModuleSpec, target: Target) -> Result<Vec<String>, ModuleError> {
    let system_only = system_only_paths(spec);
    if system_only.is_empty() {
        return Ok(Vec::new());
    }
    match target {
        Target::Home => Err(ModuleError::SystemOnlyInHome { paths: system_only }),
        Target::Nixos => Ok(system_only
            .into_iter()
            .map(|path| format!("`{path}` is a NixOS-only option"))
            .collect()),
    }
}

/// What the user selected.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModuleSpec {
    /// Package attribute paths *relative to* `pkgs`, e.g. `firefox`,
    /// `python3Packages.requests`.
    pub packages: Vec<String>,
    pub settings: Vec<Setting>,
}

/// Whitespace conventions taken from the file being edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Style {
    /// One indentation step. Never a hard-coded tab: it is whatever the file
    /// already uses (architecture §4.2).
    pub indent: String,
    /// Line ending, `"\n"` or `"\r\n"`.
    pub eol: String,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            indent: "  ".to_string(),
            eol: "\n".to_string(),
        }
    }
}

impl Style {
    /// Infer indentation and line endings from an existing file.
    ///
    /// The indentation step is the smallest positive indentation in the file
    /// (clamped to 1–8 columns); a file that indents with tabs gets a tab. This
    /// is a heuristic, so it is deliberately conservative: when nothing can be
    /// measured the two-space default is used, and a wrong guess only affects
    /// indentation inside the markers, never content outside them.
    #[must_use]
    pub fn detect(existing: &str) -> Self {
        let eol = if existing.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let mut smallest_spaces: Option<usize> = None;
        let mut tab_lines = 0_usize;
        let mut space_lines = 0_usize;
        for line in existing.split('\n') {
            let indent = leading_whitespace(line);
            let body = line.get(indent.len()..).unwrap_or("");
            if indent.is_empty() || body.trim().is_empty() {
                continue;
            }
            if indent.contains('\t') {
                tab_lines += 1;
            } else if indent.len() <= 16 {
                space_lines += 1;
                smallest_spaces =
                    Some(smallest_spaces.map_or(indent.len(), |best| best.min(indent.len())));
            }
        }
        let indent = if tab_lines > space_lines {
            "\t".to_string()
        } else {
            " ".repeat(smallest_spaces.unwrap_or(2).clamp(1, 8))
        };
        Self {
            indent,
            eol: eol.to_string(),
        }
    }
}

/// Why a spec could not be rendered.
///
/// Every variant is a refusal with a reason the UI can show. Rendering never
/// produces partial output: either the whole module is returned, or an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleError {
    /// A package attribute path is not a plain dotted path of identifiers.
    InvalidPackage { attr: String, reason: String },
    /// A setting path is not a plain dotted path of identifiers.
    InvalidSettingPath { path: String, reason: String },
    /// Two settings target the same attribute path.
    DuplicateSetting { path: String },
    /// A setting is NixOS-only and the target is a Home Manager module.
    SystemOnlyInHome { paths: Vec<String> },
    /// The rendered text did not parse. This is a bug in the renderer, and it is
    /// reported instead of written.
    RenderedTextDoesNotParse { detail: String },
    /// The existing file is not valid Nix, so nothing can be located in it.
    NotParseable { detail: String },
    /// A marker this application needs is not in the file.
    MarkerMissing { marker: Marker },
    /// A marker appears more than once, so the block boundaries are ambiguous.
    MarkerRepeated {
        marker: Marker,
        first_line: usize,
        again_line: usize,
    },
    /// The marker text appears, but not as a comment — for example inside a
    /// string. Rewriting there would corrupt the string.
    MarkerNotAComment { marker: Marker, line: usize },
    /// The markers are not in the order this application writes them.
    MarkerOutOfOrder { marker: Marker, line: usize },
    /// A block holds something this application cannot re-emit, so rewriting it
    /// would silently drop content. `line` is 1-based in the file.
    BlockHandModified {
        block: Block,
        line: usize,
        reason: String,
    },
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPackage { attr, reason } => {
                write!(f, "`{attr}` is not a usable package name: {reason}")
            }
            Self::InvalidSettingPath { path, reason } => {
                write!(f, "`{path}` is not a usable option path: {reason}")
            }
            Self::DuplicateSetting { path } => {
                write!(f, "`{path}` is set twice; remove one of them")
            }
            Self::SystemOnlyInHome { paths } => write!(
                f,
                "{} only exist{} in a NixOS module, not in a Home Manager module; \
                 nothing was generated",
                paths.join("`, `"),
                if paths.len() == 1 { "s" } else { "" }
            ),
            Self::RenderedTextDoesNotParse { detail } => write!(
                f,
                "the generated module is not valid Nix ({detail}); nothing was written"
            ),
            Self::NotParseable { detail } => write!(
                f,
                "the existing file is not valid Nix ({detail}); it was not touched"
            ),
            Self::MarkerMissing { marker } => write!(
                f,
                "{} is missing, so there is no managed region to rewrite; \
                 the file was not touched",
                marker.label()
            ),
            Self::MarkerRepeated {
                marker,
                first_line,
                again_line,
            } => write!(
                f,
                "{} appears on lines {first_line} and {again_line}; the boundaries are \
                 ambiguous, so the file was not touched",
                marker.label()
            ),
            Self::MarkerNotAComment { marker, line } => write!(
                f,
                "the text of {} on line {line} is not a comment (it is inside a string?); \
                 the file was not touched",
                marker.label()
            ),
            Self::MarkerOutOfOrder { marker, line } => write!(
                f,
                "{} on line {line} is out of order; expected the managed block before the \
                 settings block, so the file was not touched",
                marker.label()
            ),
            Self::BlockHandModified {
                block,
                line,
                reason,
            } => write!(
                f,
                "{}, line {line}: {reason}; the file was not touched",
                block.label()
            ),
        }
    }
}

impl std::error::Error for ModuleError {}

/// Render a complete sidecar module.
///
/// Packages are sorted by their byte value (not by locale) and de-duplicated, so
/// the output depends only on the *set* of selections and never on the order the
/// user clicked them. Settings are sorted by path for the same reason.
pub fn render(spec: &ModuleSpec, style: &Style, target: Target) -> Result<String, ModuleError> {
    check_target(spec, target)?;
    let packages = normalize_packages(&spec.packages)?;
    let settings = normalize_settings(&spec.settings)?;

    let unit = &style.indent;
    let mut lines: Vec<String> = vec![
        HEADER.to_string(),
        "{ pkgs, ... }:".to_string(),
        "{".to_string(),
        format!("{unit}{MANAGED_START}"),
    ];
    if packages.is_empty() {
        lines.push(format!("{unit}home.packages = [ ];"));
    } else {
        lines.push(format!("{unit}home.packages = ["));
        for package in &packages {
            lines.push(format!("{unit}{unit}pkgs.{package}"));
        }
        lines.push(format!("{unit}];"));
    }
    lines.push(format!("{unit}{MANAGED_END}"));
    lines.push(String::new());
    lines.push(format!("{unit}{SETTINGS_START}"));
    for setting in &settings {
        lines.push(format!(
            "{unit}{} = {};",
            setting.path,
            render_value(&setting.value)
        ));
    }
    lines.push(format!("{unit}{SETTINGS_END}"));
    lines.push("}".to_string());

    let mut text = lines.join(&style.eol);
    text.push_str(&style.eol);

    // A renderer bug must not reach the disk. `commit` re-parses as well; this is
    // the first of two nets.
    validate::parse(&text).map_err(|error| ModuleError::RenderedTextDoesNotParse {
        detail: error.detail,
    })?;
    Ok(text)
}

/// Validate, sort and de-duplicate package attribute paths.
fn normalize_packages(packages: &[String]) -> Result<Vec<String>, ModuleError> {
    let mut out = Vec::with_capacity(packages.len());
    for attr in packages {
        check_attribute_path(attr).map_err(|reason| ModuleError::InvalidPackage {
            attr: attr.clone(),
            reason,
        })?;
        if attr == "pkgs" || attr.starts_with("pkgs.") {
            return Err(ModuleError::InvalidPackage {
                attr: attr.clone(),
                reason: "package names are written relative to `pkgs`, so drop the `pkgs.` prefix"
                    .to_string(),
            });
        }
        out.push(attr.clone());
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Validate, sort and de-duplicate settings.
fn normalize_settings(settings: &[Setting]) -> Result<Vec<&Setting>, ModuleError> {
    let mut out: Vec<&Setting> = Vec::with_capacity(settings.len());
    for setting in settings {
        check_attribute_path(&setting.path).map_err(|reason| ModuleError::InvalidSettingPath {
            path: setting.path.clone(),
            reason,
        })?;
        if out.iter().any(|seen| seen.path == setting.path) {
            return Err(ModuleError::DuplicateSetting {
                path: setting.path.clone(),
            });
        }
        out.push(setting);
    }
    out.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    Ok(out)
}

/// Accept only a dotted path of plain identifiers.
///
/// The check is done by the Nix parser, not by a regular expression: the path is
/// placed in a one-binding probe attrset and the resulting AST must be exactly
/// that binding, with every segment a plain identifier and the source text
/// byte-identical to the input. That rejects quoting, interpolation, `${…}`,
/// comments, extra bindings and anything else that could change meaning once
/// spliced into a larger file.
pub(crate) fn check_attribute_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("an attribute path cannot be empty".to_string());
    }
    if path != path.trim() {
        return Err("an attribute path cannot start or end with whitespace".to_string());
    }
    let probe = format!("{{ {path} = null; }}");
    let parsed = rnix::Root::parse(&probe);
    if let Some(error) = parsed.errors().first() {
        return Err(format!("not a plain attribute path ({error})"));
    }
    let root = parsed.tree();
    let Some(ast::Expr::AttrSet(set)) = root.expr() else {
        return Err("not a plain attribute path".to_string());
    };
    let bindings: Vec<ast::AttrpathValue> = set.attrpath_values().collect();
    let [binding] = bindings.as_slice() else {
        return Err("must be exactly one attribute path".to_string());
    };
    let Some(attrpath) = binding.attrpath() else {
        return Err("not a plain attribute path".to_string());
    };
    let segments: Vec<ast::Attr> = attrpath.attrs().collect();
    if segments.is_empty() {
        return Err("an attribute path cannot be empty".to_string());
    }
    // Rebuild the path from the parsed segments and require it to match the
    // input byte for byte. This rejects `a . b`, comments between segments and
    // any quoting, without trusting a regular expression.
    let mut rebuilt = String::new();
    for segment in &segments {
        let ast::Attr::Ident(ident) = segment else {
            return Err(
                "only plain identifiers are allowed; quoting and `${…}` are refused".to_string(),
            );
        };
        let Some(token) = ident.ident_token() else {
            return Err("`or` cannot be used as a segment of an attribute path".to_string());
        };
        if !rebuilt.is_empty() {
            rebuilt.push('.');
        }
        rebuilt.push_str(token.text());
    }
    if rebuilt != path {
        return Err("write the path without spaces or comments between segments".to_string());
    }
    Ok(())
}

/// Render a value so that it reads back as exactly the same value.
fn render_value(value: &SettingValue) -> String {
    match value {
        SettingValue::Bool(true) => "true".to_string(),
        SettingValue::Bool(false) => "false".to_string(),
        SettingValue::Int(number) => number.to_string(),
        SettingValue::Str(text) => quote(text),
        SettingValue::StrList(items) if items.is_empty() => "[ ]".to_string(),
        SettingValue::StrList(items) => {
            let quoted: Vec<String> = items.iter().map(|item| quote(item)).collect();
            format!("[ {} ]", quoted.join(" "))
        }
    }
}

/// Quote a string so it cannot interpolate.
///
/// `${` is escaped as `\${`, which is the only way a double-quoted Nix string can
/// carry those two characters literally. Backslashes are escaped first, so an
/// input `\${x}` survives as data instead of becoming an escape sequence.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Escape only the `${` pair; `$` on its own is literal.
            '$' if chars.peek() == Some(&'{') => out.push_str("\\$"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Leading run of spaces and tabs.
fn leading_whitespace(line: &str) -> &str {
    let end = line
        .char_indices()
        .find(|(_, character)| *character != ' ' && *character != '\t')
        .map_or(line.len(), |(index, _)| index);
    line.get(..end).unwrap_or("")
}

// ---------------------------------------------------------------------------
// Rewriting an existing generated file
// ---------------------------------------------------------------------------

/// A marker comment this application writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// Opens the block that lists packages.
    ManagedStart,
    /// Closes the block that lists packages.
    ManagedEnd,
    /// Opens the block that holds option settings.
    SettingsStart,
    /// Closes the block that holds option settings.
    SettingsEnd,
}

impl Marker {
    /// The literal comment text.
    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            Self::ManagedStart => MANAGED_START,
            Self::ManagedEnd => MANAGED_END,
            Self::SettingsStart => SETTINGS_START,
            Self::SettingsEnd => SETTINGS_END,
        }
    }

    /// Name used in messages.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ManagedStart => "`# nhs:managed:start`",
            Self::ManagedEnd => "`# nhs:managed:end`",
            Self::SettingsStart => "`# nhs:settings:start`",
            Self::SettingsEnd => "`# nhs:settings:end`",
        }
    }
}

/// Which marker block a refusal is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    /// The package list.
    Managed,
    /// The option settings.
    Settings,
}

impl Block {
    fn label(self) -> &'static str {
        match self {
            Self::Managed => "the managed package block",
            Self::Settings => "the settings block",
        }
    }
}

/// Rewrite the marker blocks of an existing module from `spec`.
///
/// Only the bytes *between* the markers change. Everything else — the header, the
/// argument pattern, bindings the user added outside the markers, blank lines,
/// comments, indentation, line endings and the final newline — is copied through
/// untouched, so regenerating a file cannot reformat the user's work.
///
/// Before anything is replaced, each block must parse into exactly the shape this
/// module generates (architecture §4.2, approved deviation):
///
/// * the managed block holds one binding, `home.packages = [ pkgs.<attr> … ];`,
///   with every element a plain `pkgs.` select;
/// * the settings block holds only `<plain.path> = <bool|int|string|list of
///   strings>;` bindings;
/// * no comment appears inside either block.
///
/// Anything else is refused with the line it was found on. The returned text is
/// re-parsed before it is returned; a rewrite that would not parse is an error,
/// never output.
pub fn rewrite(old_text: &str, spec: &ModuleSpec, target: Target) -> Result<String, ModuleError> {
    check_target(spec, target)?;
    let packages = normalize_packages(&spec.packages)?;
    let settings = normalize_settings(&spec.settings)?;

    validate::parse(old_text).map_err(|error| ModuleError::NotParseable {
        detail: error.detail,
    })?;

    let style = Style::detect(old_text);
    let found = locate_markers(old_text)?;

    check_managed_block(
        found.managed_interior(old_text),
        found.managed_start.number + 1,
    )?;
    check_settings_block(
        found.settings_interior(old_text),
        found.settings_start.number + 1,
    )?;

    let new_managed = render_packages(&packages, &found.managed_indent(old_text), &style);
    let new_settings = render_settings(&settings, &found.settings_indent(old_text), &style);

    let mut out = String::with_capacity(old_text.len() + new_managed.len() + new_settings.len());
    out.push_str(old_text.get(..found.managed_interior_start()).unwrap_or(""));
    out.push_str(&new_managed);
    // Keep the end marker itself (from its line start, so its indentation is
    // untouched), the blank line after it and the settings start marker. Only the
    // bytes *between* markers are rewritten.
    out.push_str(
        old_text
            .get(found.managed_end.start..found.settings_interior_start())
            .unwrap_or(""),
    );
    out.push_str(&new_settings);
    out.push_str(old_text.get(found.settings_end.start..).unwrap_or(""));

    validate::parse(&out).map_err(|error| ModuleError::RenderedTextDoesNotParse {
        detail: error.detail,
    })?;
    Ok(out)
}

/// One source line: byte range of its content (without the line ending) and the
/// byte where the next line starts.
#[derive(Debug, Clone, Copy)]
struct Line {
    start: usize,
    end: usize,
    next_start: usize,
    /// 1-based line number.
    number: usize,
}

fn lines_of(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut start = 0_usize;
    let mut number = 1_usize;
    for (index, byte) in text.bytes().enumerate() {
        if byte != b'\n' {
            continue;
        }
        let mut end = index;
        if end > start && text.as_bytes().get(end - 1) == Some(&b'\r') {
            end -= 1;
        }
        out.push(Line {
            start,
            end,
            next_start: index + 1,
            number,
        });
        start = index + 1;
        number += 1;
    }
    if start < text.len() {
        out.push(Line {
            start,
            end: text.len(),
            next_start: text.len(),
            number,
        });
    }
    out
}

/// Where the four markers were found.
struct Located {
    managed_start: Line,
    managed_end: Line,
    settings_start: Line,
    settings_end: Line,
}

impl Located {
    fn managed_interior_start(&self) -> usize {
        self.managed_start.next_start
    }

    fn managed_interior<'a>(&self, text: &'a str) -> &'a str {
        text.get(self.managed_start.next_start..self.managed_end.start)
            .unwrap_or("")
    }

    fn settings_interior_start(&self) -> usize {
        self.settings_start.next_start
    }

    fn settings_interior<'a>(&self, text: &'a str) -> &'a str {
        text.get(self.settings_start.next_start..self.settings_end.start)
            .unwrap_or("")
    }

    /// Indentation of the marker line, used as the base for the block's lines.
    fn indent_of(text: &str, line: &Line) -> String {
        let content = text.get(line.start..line.end).unwrap_or("");
        let trimmed = content.trim_start_matches([' ', '\t']);
        content
            .get(..content.len().saturating_sub(trimmed.len()))
            .unwrap_or("")
            .to_string()
    }

    fn managed_indent(&self, text: &str) -> String {
        Self::indent_of(text, &self.managed_start)
    }

    fn settings_indent(&self, text: &str) -> String {
        Self::indent_of(text, &self.settings_start)
    }
}

fn locate_markers(text: &str) -> Result<Located, ModuleError> {
    let markers = [
        Marker::ManagedStart,
        Marker::ManagedEnd,
        Marker::SettingsStart,
        Marker::SettingsEnd,
    ];
    let comments = comment_starts(text);
    let mut found: [Option<Line>; 4] = [None; 4];

    for line in lines_of(text) {
        let content = text.get(line.start..line.end).unwrap_or("");
        let trimmed = content.trim_start_matches([' ', '\t']);
        let Some(index) = markers.iter().position(|marker| marker.text() == trimmed) else {
            continue;
        };
        let Some(marker) = markers.get(index).copied() else {
            continue;
        };
        let hash_offset = line
            .start
            .saturating_add(content.len().saturating_sub(trimmed.len()));
        // The same text inside a string is not a marker.
        if !comments.contains(&hash_offset) {
            return Err(ModuleError::MarkerNotAComment {
                marker,
                line: line.number,
            });
        }
        if let Some(slot) = found.get_mut(index) {
            if let Some(first) = *slot {
                return Err(ModuleError::MarkerRepeated {
                    marker,
                    first_line: first.number,
                    again_line: line.number,
                });
            }
            *slot = Some(line);
        }
    }

    let take = |index: usize, marker: Marker| -> Result<Line, ModuleError> {
        found
            .get(index)
            .and_then(|slot| *slot)
            .ok_or(ModuleError::MarkerMissing { marker })
    };
    let managed_start = take(0, Marker::ManagedStart)?;
    let managed_end = take(1, Marker::ManagedEnd)?;
    let settings_start = take(2, Marker::SettingsStart)?;
    let settings_end = take(3, Marker::SettingsEnd)?;

    let order = [
        (managed_start.number, Marker::ManagedStart),
        (managed_end.number, Marker::ManagedEnd),
        (settings_start.number, Marker::SettingsStart),
        (settings_end.number, Marker::SettingsEnd),
    ];
    let mut previous: Option<(usize, Marker)> = None;
    for entry in order {
        if let Some((line, _)) = previous {
            if line >= entry.0 {
                return Err(ModuleError::MarkerOutOfOrder {
                    marker: entry.1,
                    line: entry.0,
                });
            }
        }
        previous = Some(entry);
    }

    Ok(Located {
        managed_start,
        managed_end,
        settings_start,
        settings_end,
    })
}

/// Byte offsets of real `#` comment tokens.
fn comment_starts(text: &str) -> Vec<usize> {
    rnix::Root::parse(text)
        .syntax()
        .descendants_with_tokens()
        .filter_map(rnix::NodeOrToken::into_token)
        .filter(|token| token.kind() == rnix::SyntaxKind::TOKEN_COMMENT)
        .map(|token| usize::from(token.text_range().start()))
        .collect()
}

/// Every direct child of the block's attribute set must be an ordinary
/// `name = value;` binding.
///
/// The check is on *all* entries, not on the entries that were collected: an
/// `inherit (pkgs) hello;` is a legal sibling of a binding, is not an
/// `AttrpathValue`, and would otherwise be dropped by the rewrite without a word.
/// `inherit` and any future entry form are therefore refused by name and line.
fn require_only_bindings(
    set: &rnix::SyntaxNode,
    block: Block,
    interior: &str,
    first_line: usize,
) -> Result<Vec<rnix::SyntaxNode>, ModuleError> {
    let mut bindings = Vec::new();
    for child in set.children() {
        if child.kind() != rnix::SyntaxKind::NODE_ATTRPATH_VALUE {
            let text = child.text().to_string();
            let first = text.lines().next().unwrap_or("").trim();
            return Err(ModuleError::BlockHandModified {
                block,
                line: block_line(
                    interior,
                    usize::from(child.text_range().start()),
                    first_line,
                ),
                reason: format!(
                    "`{first}` is not a plain `name = value;` binding; the rewrite would \
                     drop it"
                ),
            });
        }
        bindings.push(child);
    }
    Ok(bindings)
}

/// Validate the package block's shape.
fn check_managed_block(interior: &str, first_line: usize) -> Result<(), ModuleError> {
    // A leading and trailing newline keep the interior at byte offset 2 and let a
    // block whose binding shares a line with its marker still parse.
    let probe = format!("{{\n{interior}\n}}");
    let parsed = rnix::Root::parse(&probe);
    if let Some(error) = parsed.errors().first() {
        return Err(ModuleError::BlockHandModified {
            block: Block::Managed,
            line: first_line,
            reason: format!("the block is not valid Nix ({error})"),
        });
    }
    let root = parsed.syntax();
    let set = root
        .descendants()
        .find(|node| node.kind() == rnix::SyntaxKind::NODE_ATTR_SET)
        .ok_or_else(|| ModuleError::BlockHandModified {
            block: Block::Managed,
            line: first_line,
            reason: "the block is not an attribute set".to_string(),
        })?;
    let bindings = require_only_bindings(&set, Block::Managed, interior, first_line)?;
    let [binding] = bindings.as_slice() else {
        return Err(ModuleError::BlockHandModified {
            block: Block::Managed,
            line: block_line(interior, usize::from(set.text_range().start()), first_line),
            reason: format!(
                "expected exactly one binding (`home.packages = [ … ];`), found {}",
                bindings.len()
            ),
        });
    };
    let binding_line = block_line(
        interior,
        usize::from(binding.text_range().start()),
        first_line,
    );

    let attrpath = binding
        .children()
        .find(|node| node.kind() == rnix::SyntaxKind::NODE_ATTRPATH)
        .ok_or_else(|| ModuleError::BlockHandModified {
            block: Block::Managed,
            line: binding_line,
            reason: "the binding has no attribute path".to_string(),
        })?;
    if attrpath.text() != "home.packages" {
        return Err(ModuleError::BlockHandModified {
            block: Block::Managed,
            line: binding_line,
            reason: format!(
                "the managed block must only set `home.packages`, found `{}`",
                attrpath.text()
            ),
        });
    }

    let value = binding
        .children()
        .find(|node| node.kind() == rnix::SyntaxKind::NODE_LIST)
        .ok_or_else(|| ModuleError::BlockHandModified {
            block: Block::Managed,
            line: binding_line,
            reason: "`home.packages` must be a list literal".to_string(),
        })?;

    for element in value.children() {
        let text = element.text().to_string();
        let element_line = block_line(
            interior,
            usize::from(element.text_range().start()),
            first_line,
        );
        if element.kind() != rnix::SyntaxKind::NODE_SELECT {
            return Err(ModuleError::BlockHandModified {
                block: Block::Managed,
                line: element_line,
                reason: format!(
                    "`{text}` is not a plain package reference; expected `pkgs.<name>`"
                ),
            });
        }
        let Some(rest) = text.strip_prefix("pkgs.") else {
            return Err(ModuleError::BlockHandModified {
                block: Block::Managed,
                line: element_line,
                reason: format!("`{text}` does not start with `pkgs.`"),
            });
        };
        check_attribute_path(rest).map_err(|reason| ModuleError::BlockHandModified {
            block: Block::Managed,
            line: element_line,
            reason: format!("`{text}` is not a plain package reference ({reason})"),
        })?;
    }

    refuse_comment_in(interior, Block::Managed, first_line)?;
    Ok(())
}

/// Validate the settings block's shape.
fn check_settings_block(interior: &str, first_line: usize) -> Result<(), ModuleError> {
    let probe = format!("{{\n{interior}\n}}");
    let parsed = rnix::Root::parse(&probe);
    if let Some(error) = parsed.errors().first() {
        return Err(ModuleError::BlockHandModified {
            block: Block::Settings,
            line: first_line,
            reason: format!("the block is not valid Nix ({error})"),
        });
    }
    let root = parsed.syntax();
    let set = root
        .descendants()
        .find(|node| node.kind() == rnix::SyntaxKind::NODE_ATTR_SET)
        .ok_or_else(|| ModuleError::BlockHandModified {
            block: Block::Settings,
            line: first_line,
            reason: "the block is not an attribute set".to_string(),
        })?;

    for binding in require_only_bindings(&set, Block::Settings, interior, first_line)? {
        let line = block_line(
            interior,
            usize::from(binding.text_range().start()),
            first_line,
        );
        let attrpath = binding
            .children()
            .find(|node| node.kind() == rnix::SyntaxKind::NODE_ATTRPATH)
            .ok_or_else(|| ModuleError::BlockHandModified {
                block: Block::Settings,
                line,
                reason: "the binding has no attribute path".to_string(),
            })?;
        let path = attrpath.text().to_string();
        check_attribute_path(&path).map_err(|reason| ModuleError::BlockHandModified {
            block: Block::Settings,
            line,
            reason: format!("`{path}` is not a plain option path ({reason})"),
        })?;
        // The children are the attribute path and then the value expression.
        // Filter out the punctuation the node keeps as separate tokens.
        let value = binding
            .children()
            .filter(|node| node.kind() != rnix::SyntaxKind::NODE_ATTRPATH)
            .last()
            .filter(|node| node.first_token().is_some())
            .ok_or_else(|| ModuleError::BlockHandModified {
                block: Block::Settings,
                line,
                reason: "the binding has no value".to_string(),
            })?;
        if !is_simple_value(&value) {
            return Err(ModuleError::BlockHandModified {
                block: Block::Settings,
                line,
                reason: format!(
                    "`{}` is not a value this application can re-emit; only booleans, \
                     integers, strings and lists of strings are allowed",
                    value.text()
                ),
            });
        }
    }

    refuse_comment_in(interior, Block::Settings, first_line)?;
    Ok(())
}

/// True for the value shapes [`SettingValue`] can reproduce exactly.
fn is_simple_value(node: &rnix::SyntaxNode) -> bool {
    match node.kind() {
        // `true`/`false` arrive as an identifier, not as a literal.
        rnix::SyntaxKind::NODE_IDENT => {
            let text = node.text().to_string();
            text == "true" || text == "false"
        }
        rnix::SyntaxKind::NODE_LITERAL => node.text().to_string().parse::<i64>().is_ok(),
        rnix::SyntaxKind::NODE_UNARY_OP => node
            .text()
            .to_string()
            .strip_prefix('-')
            .is_some_and(|rest| rest.parse::<i64>().is_ok()),
        rnix::SyntaxKind::NODE_STRING => true,
        rnix::SyntaxKind::NODE_LIST => node
            .children()
            .all(|item| item.kind() == rnix::SyntaxKind::NODE_STRING),
        _ => false,
    }
}

/// Refuse any comment inside a block: it would be silently dropped by the
/// rewrite, and a note the user wrote must never disappear without a word.
fn refuse_comment_in(interior: &str, block: Block, first_line: usize) -> Result<(), ModuleError> {
    let Some(offset) = first_comment(interior) else {
        return Ok(());
    };
    Err(ModuleError::BlockHandModified {
        block,
        line: block_line(interior, offset, first_line),
        reason: "a comment inside the markers would be overwritten; move it outside the block"
            .to_string(),
    })
}

/// Offset of the first `#` or `/*` that is not inside a string.
///
/// Both string forms are handled: a double-quoted string, where a backslash
/// escapes the next character, and an indented string (two single quotes), where
/// a backslash is literal and only a run of single quotes can close it.
fn first_comment(text: &str) -> Option<usize> {
    let mut in_double = false;
    let mut escaped = false;
    let mut in_indented = false;
    let mut quotes = 0_usize;
    let mut previous = '\0';

    for (index, character) in text.char_indices() {
        if in_double {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_double = false;
            }
            continue;
        }
        if in_indented {
            if character == '\'' {
                quotes += 1;
            } else {
                if quotes >= 2 {
                    in_indented = false;
                }
                quotes = 0;
            }
            continue;
        }
        match character {
            '"' => in_double = true,
            '\'' if previous == '\'' => {
                in_indented = true;
                quotes = 2;
            }
            '#' => return Some(index),
            '/' if previous == '/' || previous == '*' => return Some(index - 1),
            _ => {}
        }
        previous = character;
    }
    None
}

/// Translate a byte offset inside the probe into a 1-based line in the file.
fn block_line(interior: &str, probe_offset: usize, first_line: usize) -> usize {
    let offset = probe_offset.saturating_sub(2);
    let before = interior.get(..offset).unwrap_or("");
    first_line.saturating_add(before.matches('\n').count())
}

/// Render the package block interior.
fn render_packages(packages: &[String], base_indent: &str, style: &Style) -> String {
    let unit = &style.indent;
    let eol = &style.eol;
    let mut out = String::new();
    if packages.is_empty() {
        out.push_str(base_indent);
        out.push_str("home.packages = [ ];");
        out.push_str(eol);
        return out;
    }
    out.push_str(base_indent);
    out.push_str("home.packages = [");
    out.push_str(eol);
    for package in packages {
        out.push_str(base_indent);
        out.push_str(unit);
        out.push_str("pkgs.");
        out.push_str(package);
        out.push_str(eol);
    }
    out.push_str(base_indent);
    out.push_str("];");
    out.push_str(eol);
    out
}

/// Render the settings block interior.
fn render_settings(settings: &[&Setting], base_indent: &str, style: &Style) -> String {
    let mut out = String::new();
    for setting in settings {
        out.push_str(base_indent);
        out.push_str(&setting.path);
        out.push_str(" = ");
        out.push_str(&render_value(&setting.value));
        out.push(';');
        out.push_str(&style.eol);
    }
    out
}
