//! Adding the generated module to the user's main file.
//!
//! The only change this module ever makes is *one* import entry: no reformatting,
//! no reordering, no second import, and no edit to any other line. The `imports`
//! attribute is located through the syntax tree, never through a regular
//! expression or a line scan, and every construct that cannot be handled by
//! inserting a list element is refused with a reason instead of guessed at.
//!
//! Refusing is the default: a module that computes `imports`, or keeps it in a
//! nested attribute set, or lists it as a string, is left alone. The caller can
//! then show the user what to add by hand.

use rnix::{SyntaxKind, SyntaxNode};

use crate::edit::validate;
use crate::module::Style;

/// Default location of the generated sidecar module, relative to the config root.
pub const DEFAULT_SIDECAR: &str = "./modules/nhs-packages.nix";

/// Why the sidecar import could not be added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// The file does not parse, so its structure is unknown.
    NotParseable { detail: String },
    /// The file's top-level expression is not a module: nothing resolves to the
    /// attribute set a Home Manager module returns.
    NoModuleAttrset { detail: String },
    /// `imports` is present but is not a list literal (a string, a computation,
    /// `++` concatenation, a function call, …).
    ImportsNotAList { line: usize, detail: String },
    /// The list contains something other than a plain path literal.
    ComputedImport { line: usize, text: String },
    /// The list contains a string; Home Manager needs a path, and a relative
    /// string would not resolve.
    StringImport { line: usize, text: String },
    /// More than one `imports` binding makes the target ambiguous.
    DuplicateImports {
        first_line: usize,
        again_line: usize,
    },
    /// `imports` exists, but inside a nested attribute set, where adding a second
    /// one at the top would silently shadow or duplicate it.
    NestedImports { line: usize },
    /// `imports` is brought into scope by `inherit`, so there is no list literal to
    /// extend and adding a second binding would duplicate the attribute.
    InheritedImports { line: usize },
    /// The path this application was asked to inject is not a plain relative path.
    InvalidSidecar { path: String, reason: String },
    /// The edited text did not parse. A bug, reported instead of returned.
    RenderedTextDoesNotParse { detail: String },
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotParseable { detail } => {
                write!(
                    f,
                    "the main file is not valid Nix ({detail}); it was not touched"
                )
            }
            Self::NoModuleAttrset { detail } => write!(
                f,
                "could not find the module's attribute set: {detail}; the file was not touched"
            ),
            Self::ImportsNotAList { line, detail } => write!(
                f,
                "`imports` on line {line} is not a list literal ({detail}); add the import by \
                 hand, the file was not touched"
            ),
            Self::ComputedImport { line, text } => write!(
                f,
                "the import `{text}` on line {line} is computed; this application only adds to \
                 a list of plain paths, so the file was not touched"
            ),
            Self::StringImport { line, text } => write!(
                f,
                "the import \"{text}\" on line {line} is a string, not a path; Home Manager \
                 needs a path, so the file was not touched"
            ),
            Self::DuplicateImports {
                first_line,
                again_line,
            } => write!(
                f,
                "`imports` is set on lines {first_line} and {again_line}; the target is \
                 ambiguous, so the file was not touched"
            ),
            Self::NestedImports { line } => write!(
                f,
                "`imports` on line {line} is inside a nested attribute set; adding another one \
                 at the top could shadow it, so the file was not touched"
            ),
            Self::InheritedImports { line } => write!(
                f,
                "`imports` on line {line} comes from an `inherit`, so there is no list literal \
                 to add to; this application only edits an explicit `imports = [ … ];`, so the \
                 file was not touched"
            ),
            Self::InvalidSidecar { path, reason } => {
                write!(f, "`{path}` is not a usable import path: {reason}")
            }
            Self::RenderedTextDoesNotParse { detail } => write!(
                f,
                "the edited module is not valid Nix ({detail}); nothing was written"
            ),
        }
    }
}

impl std::error::Error for ImportError {}

/// Add `sidecar` to the `imports` list of the module in `main_text`.
///
/// Returns the text unchanged when the import is already there (in any
/// equivalent spelling), so applying the same selection twice is a no-op.
pub fn inject_sidecar(main_text: &str, sidecar: &str) -> Result<String, ImportError> {
    check_sidecar_path(sidecar)?;
    validate::parse(main_text).map_err(|error| ImportError::NotParseable {
        detail: error.detail,
    })?;

    let parsed = rnix::Root::parse(main_text);
    let root = parsed.syntax();
    let top = root
        .first_child()
        .ok_or_else(|| ImportError::NoModuleAttrset {
            detail: "the file is empty".to_string(),
        })?;
    let module = resolve_module_attrset(top)?;

    if let Some(line) = direct_inherited_imports(main_text, &module) {
        return Err(ImportError::InheritedImports { line });
    }
    let bindings = imports_bindings(&module);
    match bindings.as_slice() {
        [] => {}
        [binding] => return insert_into_existing(main_text, binding, sidecar),
        [first, second, ..] => {
            return Err(ImportError::DuplicateImports {
                first_line: line_of(main_text, usize::from(first.text_range().start())),
                again_line: line_of(main_text, usize::from(second.text_range().start())),
            })
        }
    }

    if let Some(nested) = nested_imports(main_text, &module) {
        return Err(ImportError::NestedImports { line: nested });
    }

    let splice = plan_new_binding(main_text, &module, sidecar);
    finish(main_text, &splice)
}

/// Resolve the expression a module returns to its attribute set.
///
/// A Home Manager module is `{ … }: <attrset>`; the body may pass through `let …
/// in`, `with …;`, `assert …;` or parentheses. Anything else is refused rather
/// than searched for an attrset somewhere inside.
fn resolve_module_attrset(mut expr: SyntaxNode) -> Result<SyntaxNode, ImportError> {
    for _ in 0..32 {
        match expr.kind() {
            SyntaxKind::NODE_ATTR_SET => return Ok(expr),
            SyntaxKind::NODE_LAMBDA
            | SyntaxKind::NODE_LET_IN
            | SyntaxKind::NODE_WITH
            | SyntaxKind::NODE_ASSERT
            | SyntaxKind::NODE_PAREN => {
                let Some(body) = expr.children().last() else {
                    return Err(ImportError::NoModuleAttrset {
                        detail: format!("the `{}` expression has no body", kind_name(expr.kind())),
                    });
                };
                expr = body;
            }
            other => {
                return Err(ImportError::NoModuleAttrset {
                    detail: format!("the top-level expression is `{}`", kind_name(other)),
                })
            }
        }
    }
    Err(ImportError::NoModuleAttrset {
        detail: "the module is nested too deeply to resolve".to_string(),
    })
}

/// Human-readable name for the kinds this module talks about.
fn kind_name(kind: SyntaxKind) -> &'static str {
    match kind {
        SyntaxKind::NODE_ATTR_SET => "attribute set",
        SyntaxKind::NODE_LAMBDA => "function",
        SyntaxKind::NODE_LET_IN => "let … in",
        SyntaxKind::NODE_WITH => "with …;",
        SyntaxKind::NODE_ASSERT => "assert …;",
        SyntaxKind::NODE_PAREN => "parentheses",
        SyntaxKind::NODE_APPLY => "function application",
        SyntaxKind::NODE_BIN_OP => "operator expression",
        SyntaxKind::NODE_STRING => "string",
        _ => "expression",
    }
}

/// `imports = …;` bindings that are direct children of the module's attrset.
fn imports_bindings(module: &SyntaxNode) -> Vec<SyntaxNode> {
    module
        .children()
        .filter(|child| child.kind() == SyntaxKind::NODE_ATTRPATH_VALUE)
        .filter(|child| {
            child
                .children()
                .find(|node| node.kind() == SyntaxKind::NODE_ATTRPATH)
                .is_some_and(|path| path.text() == "imports")
        })
        .collect()
}

/// True when an `inherit` entry brings the name `imports` into scope.
///
/// Only the entries that are direct children count: the identifiers inside the
/// `(…)` source expression belong to `NODE_INHERIT_FROM` and are somebody else's
/// names — `inherit (lib.imports) foo;` does not bring in `imports`.
fn inherit_includes_imports(node: &SyntaxNode) -> bool {
    node.children().any(|child| match child.kind() {
        SyntaxKind::NODE_IDENT | SyntaxKind::NODE_STRING => {
            let text = child.text().to_string();
            text == "imports" || text == "\"imports\""
        }
        _ => false,
    })
}

/// `inherit imports;` / `inherit (source) imports;` directly in the module's
/// attribute set, which already defines `imports`.
fn direct_inherited_imports(text: &str, module: &SyntaxNode) -> Option<usize> {
    module
        .children()
        .filter(|node| node.kind() == SyntaxKind::NODE_INHERIT)
        .find(inherit_includes_imports)
        .map(|node| line_of(text, usize::from(node.text_range().start())))
}

/// Line of `imports` that is declared somewhere other than the module's own
/// attribute set — as a binding or through `inherit`.
fn nested_imports(text: &str, module: &SyntaxNode) -> Option<usize> {
    let binding = module
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::NODE_ATTRPATH_VALUE)
        .filter(|node| node.parent() != Some(module.clone()))
        .find(|node| {
            node.children()
                .find(|child| child.kind() == SyntaxKind::NODE_ATTRPATH)
                .is_some_and(|path| path.text() == "imports")
        });
    let inherited = module
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::NODE_INHERIT)
        .filter(|node| node.parent() != Some(module.clone()))
        .find(inherit_includes_imports);
    binding
        .or(inherited)
        .map(|node| line_of(text, usize::from(node.text_range().start())))
}

/// Append the import to an existing `imports` list.
fn insert_into_existing(
    text: &str,
    binding: &SyntaxNode,
    sidecar: &str,
) -> Result<String, ImportError> {
    let line = line_of(text, usize::from(binding.text_range().start()));
    let value = binding
        .children()
        .filter(|node| node.kind() != SyntaxKind::NODE_ATTRPATH)
        .last()
        .ok_or_else(|| ImportError::ImportsNotAList {
            line,
            detail: "the binding has no value".to_string(),
        })?;
    if value.kind() != SyntaxKind::NODE_LIST {
        return Err(ImportError::ImportsNotAList {
            line,
            detail: format!("it is a `{}`", kind_name(value.kind())),
        });
    }

    let mut already_imported = false;
    for element in value.children() {
        let element_line = line_of(text, usize::from(element.text_range().start()));
        let element_text = element.text().to_string();
        match element.kind() {
            SyntaxKind::NODE_PATH_REL | SyntaxKind::NODE_PATH_ABS => {
                if same_path(&element_text, sidecar) {
                    already_imported = true;
                }
            }
            SyntaxKind::NODE_STRING => {
                return Err(ImportError::StringImport {
                    line: element_line,
                    text: element_text,
                })
            }
            // `~/…`, `<nixpkgs>`, a parenthesised expression, an application:
            // anything that is not a plain path is computed.
            _ => {
                return Err(ImportError::ComputedImport {
                    line: element_line,
                    text: element_text,
                })
            }
        }
    }
    if already_imported {
        return Ok(text.to_string());
    }

    let style = Style::detect(text);
    let splice = plan_list_insertion(text, &value, binding, sidecar, &style);
    finish(text, &splice)
}

/// Where to put the new element inside an existing list.
fn plan_list_insertion(
    text: &str,
    list: &SyntaxNode,
    binding: &SyntaxNode,
    sidecar: &str,
    style: &Style,
) -> Splice {
    let close = usize::from(list.text_range().end()).saturating_sub(1);
    let multiline = text
        .get(usize::from(list.text_range().start())..close)
        .is_some_and(|body| body.contains('\n'));

    if !multiline {
        let before = text
            .get(..close)
            .and_then(|head| head.chars().next_back())
            .is_some_and(|character| character == ' ' || character == '\t');
        let insert = if before {
            format!("{sidecar} ")
        } else {
            format!(" {sidecar} ")
        };
        return Splice {
            offset: close,
            insert,
        };
    }

    let line_start = line_start_of(text, close);
    let tail = text.get(line_start..close).unwrap_or("");
    let close_alone = tail.trim().is_empty();
    let indent = if close_alone {
        // Line up with the last element; if the list is empty, one step deeper
        // than the `imports` line.
        last_element_indent(text, list).unwrap_or_else(|| {
            let binding_line = line_start_of(text, usize::from(binding.text_range().start()));
            format!("{}{}", indent_at(text, binding_line), style.indent)
        })
    } else {
        indent_at(text, line_start)
    };
    Splice {
        offset: line_start,
        insert: format!("{indent}{sidecar}{}", style.eol),
    }
}

/// Indentation of the line holding the last element of `list`.
fn last_element_indent(text: &str, list: &SyntaxNode) -> Option<String> {
    let element = list.children().last()?;
    let line = line_start_of(text, usize::from(element.text_range().start()));
    Some(indent_at(text, line))
}

/// Where to add a whole `imports = [ … ];` binding.
fn plan_new_binding(text: &str, module: &SyntaxNode, sidecar: &str) -> Splice {
    let style = Style::detect(text);
    let open = module
        .children_with_tokens()
        .filter_map(rnix::NodeOrToken::into_token)
        .find(|token| token.kind() == SyntaxKind::TOKEN_L_BRACE)
        .map_or_else(
            || usize::from(module.text_range().start()),
            |token| usize::from(token.text_range().start()),
        );
    let close = usize::from(module.text_range().end()).saturating_sub(1);

    let first_entry = module.children().find(|node| {
        matches!(
            node.kind(),
            SyntaxKind::NODE_ATTRPATH_VALUE | SyntaxKind::NODE_INHERIT
        )
    });
    let Some(entry) = first_entry else {
        // Empty attribute set.
        if line_start_of(text, close) == line_start_of(text, open) {
            let before = text
                .get(..close)
                .and_then(|head| head.chars().next_back())
                .is_some_and(|character| character == ' ' || character == '\t');
            let statement = format!("imports = [ {sidecar} ];");
            let insert = if before {
                format!("{statement} ")
            } else {
                format!(" {statement} ")
            };
            return Splice {
                offset: close,
                insert,
            };
        }
        let indent = format!(
            "{}{}",
            indent_at(text, line_start_of(text, close)),
            style.indent
        );
        return Splice {
            offset: line_start_of(text, close),
            insert: format!("{indent}imports = [ {sidecar} ];{}", style.eol),
        };
    };

    let entry_start = usize::from(entry.text_range().start());
    if line_start_of(text, entry_start) == line_start_of(text, open) {
        // The whole attribute set is on one line: add the binding inline, before
        // the first entry, so the line stays one line.
        return Splice {
            offset: entry_start,
            insert: format!("imports = [ {sidecar} ]; "),
        };
    }
    let line = line_start_of(text, entry_start);
    Splice {
        offset: line,
        insert: format!(
            "{}imports = [ {sidecar} ];{}",
            indent_at(text, line),
            style.eol
        ),
    }
}

/// A byte offset and the text to insert there.
struct Splice {
    offset: usize,
    insert: String,
}

/// Apply a splice and re-parse the result.
fn finish(text: &str, splice: &Splice) -> Result<String, ImportError> {
    let mut out = String::with_capacity(text.len() + splice.insert.len());
    out.push_str(text.get(..splice.offset).unwrap_or(""));
    out.push_str(&splice.insert);
    out.push_str(text.get(splice.offset..).unwrap_or(""));
    validate::parse(&out).map_err(|error| ImportError::RenderedTextDoesNotParse {
        detail: error.detail,
    })?;
    Ok(out)
}

/// Reject a sidecar path that is not a plain relative or absolute path.
///
/// This value is written into the file as Nix path syntax, so it must not be
/// able to become an expression.
fn check_sidecar_path(path: &str) -> Result<(), ImportError> {
    let reason = if path.is_empty() {
        Some("the path is empty".to_string())
    } else if !(path.starts_with("./") || path.starts_with("../") || path.starts_with('/')) {
        Some("a path must start with `./`, `../` or `/`".to_string())
    } else if path.chars().any(char::is_whitespace) {
        Some("a path cannot contain whitespace".to_string())
    } else if path.contains(['$', '"', '\'', '(', ')', ';', '`']) {
        Some("a path cannot contain quotes, `$`, parentheses, `;` or backticks".to_string())
    } else if normalize_path(path).is_none() {
        Some("the path cannot be normalised".to_string())
    } else {
        None
    };
    match reason {
        Some(reason) => Err(ImportError::InvalidSidecar {
            path: path.to_string(),
            reason,
        }),
        None => Ok(()),
    }
}

/// Normalise `a/./b`, `a//b` and `a/../b` so two spellings of one path compare
/// equal. Returns `None` when the path cannot be normalised lexically (for
/// example when `..` would climb above the root).
fn normalize_path(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    let absolute = path.starts_with('/');
    let mut out = String::new();
    if absolute {
        out.push('/');
    }
    out.push_str(&parts.join("/"));
    Some(out)
}

/// True when both spellings name the same path.
fn same_path(left: &str, right: &str) -> bool {
    match (normalize_path(left), normalize_path(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// 1-based line of a byte offset.
fn line_of(text: &str, offset: usize) -> usize {
    1 + text
        .get(..offset)
        .map_or(0, |before| before.matches('\n').count())
}

/// Byte offset of the first character of the line containing `offset`.
fn line_start_of(text: &str, offset: usize) -> usize {
    text.get(..offset)
        .and_then(|before| before.rfind('\n'))
        .map_or(0, |index| index + 1)
}

/// Indentation of the line starting at `start`.
fn indent_at(text: &str, start: usize) -> String {
    let rest = text.get(start..).unwrap_or("");
    let trimmed = rest.trim_start_matches([' ', '\t']);
    rest.get(..rest.len().saturating_sub(trimmed.len()))
        .unwrap_or("")
        .to_string()
}
