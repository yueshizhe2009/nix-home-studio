//! Turning a failed build into something a person can act on.
//!
//! Nix's failures are precise and unfriendly: `error: value is a string while a
//! list was expected` tells an expert everything and a beginner nothing. This
//! module keeps the original text (it is evidence) and adds two things: a stable
//! [`ErrorKind`] a UI can switch on, and one sentence about what to do next.
//!
//! The classification is a *lookup over known shapes*, not a guess: when nothing
//! matches, the result is [`ErrorKind::Unknown`] and the summary is the actual
//! first error line. Inventing a diagnosis would be worse than admitting one is
//! not available.
//!
//! Structured logs are preferred over text: with `--log-format internal-json`
//! the error message arrives as a `@nix {"action":"msg","level":0,…}` record, so
//! ANSI stripping and line splitting are not guesswork.

use crate::donated::nix_log::NixLog;
use crate::proc::Outcome;

/// A stable category for a failed run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Stopped by cancellation or by the timeout.
    Interrupted,
    /// A licence Nix will not accept automatically.
    UnfreePackage,
    /// A fixed-output derivation produced different content than recorded.
    HashMismatch,
    /// Home Manager does not know the option in this version.
    UndefinedOption,
    /// An attribute the configuration refers to does not exist.
    MissingAttribute,
    /// A name is used before it is defined.
    UndefinedVariable,
    /// The file is not valid Nix.
    SyntaxError,
    /// A path the configuration refers to does not exist.
    MissingFile,
    /// Evaluation needed the network and could not get it.
    Network,
    /// The filesystem refused an operation.
    PermissionDenied,
    /// The disk filled up mid-build.
    DiskFull,
    /// A package's build script failed.
    BuilderFailed,
    /// Evaluation failed for a reason this classifier does not recognise.
    Evaluation,
    /// Unrecognised, with no `error:` line to show either.
    Unknown,
}

impl ErrorKind {
    /// Stable machine-readable name, used in the CLI's `--json` output.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Interrupted => "interrupted",
            Self::UnfreePackage => "unfree-package",
            Self::HashMismatch => "hash-mismatch",
            Self::UndefinedOption => "undefined-option",
            Self::MissingAttribute => "missing-attribute",
            Self::UndefinedVariable => "undefined-variable",
            Self::SyntaxError => "syntax-error",
            Self::MissingFile => "missing-file",
            Self::Network => "network",
            Self::PermissionDenied => "permission-denied",
            Self::DiskFull => "disk-full",
            Self::BuilderFailed => "builder-failed",
            Self::Evaluation => "evaluation",
            Self::Unknown => "unknown",
        }
    }
}

/// What to tell the user about a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnosis {
    pub kind: ErrorKind,
    /// The real message, stripped of colour codes and leading `error: `.
    pub summary: String,
    /// One sentence: what to do about it. Static, so it cannot drift with a log.
    pub hint: &'static str,
    /// `path:line:column` when the output names one.
    pub location: Option<String>,
}

/// Diagnose a finished run, or `None` when it succeeded.
#[must_use]
pub fn diagnose(outcome: &Outcome) -> Option<Diagnosis> {
    if outcome.success() {
        return None;
    }
    if outcome.cancelled {
        return Some(Diagnosis {
            kind: ErrorKind::Interrupted,
            summary: "the command was stopped before it finished".to_string(),
            hint: "Nothing was applied. Run the build again when you are ready.",
            location: None,
        });
    }
    if outcome.timed_out {
        return Some(Diagnosis {
            kind: ErrorKind::Interrupted,
            summary: "the command ran out of time".to_string(),
            hint: "Nothing was applied. A first build can be slow; check the connection, then \
                   try again or allow more time.",
            location: None,
        });
    }
    Some(classify_stderr(&outcome.stderr))
}

/// Diagnose raw stderr, for callers that did not run the command themselves.
#[must_use]
pub fn classify_stderr(stderr: &str) -> Diagnosis {
    let stripped = strip_ansi(stderr);
    let primary = primary_message(&stripped);
    let haystack = stripped.to_lowercase();
    let kind = detect(&haystack);
    Diagnosis {
        kind,
        summary: primary,
        hint: hint_for(kind),
        location: location_in(&stripped),
    }
}

/// The message to show first: the last structured error, else the first
/// `error:` line, else the first non-empty line.
fn primary_message(stripped: &str) -> String {
    let mut structured: Option<String> = None;
    for line in stripped.lines() {
        if let Some(NixLog::Message { level: 0, text }) = NixLog::parse(line) {
            structured = Some(text);
        }
    }
    if let Some(text) = structured {
        return tidy(&text);
    }
    if let Some(line) = stripped
        .lines()
        .find(|line| line.trim_start().starts_with("error:"))
    {
        return tidy(line);
    }
    stripped
        .lines()
        .find(|line| !line.trim().is_empty())
        .map_or_else(|| "the command failed without a message".to_string(), tidy)
}

/// Drop the `error:` marker and trailing whitespace so the text reads as a
/// sentence when a UI prefixes it with "Build failed: ".
fn tidy(text: &str) -> String {
    let trimmed = text.trim();
    let without_prefix = trimmed.strip_prefix("error:").unwrap_or(trimmed).trim();
    without_prefix.to_string()
}

/// Pick a kind from the lowercased output.
///
/// Order matters: the most specific and least ambiguous shapes come first, so
/// `builder failed … hash mismatch` is a hash mismatch, not a generic build
/// failure.
fn detect(haystack: &str) -> ErrorKind {
    const NETWORK: [&str; 6] = [
        "unable to download",
        "could not resolve host",
        "connection refused",
        "network is unreachable",
        "tls handshake",
        "ssl error",
    ];
    // Most specific first: a builder failure often quotes a nested error, and
    // "No such file or directory" appears inside build logs all the time.
    if haystack.contains("unfree") {
        return ErrorKind::UnfreePackage;
    }
    if haystack.contains("hash mismatch") {
        return ErrorKind::HashMismatch;
    }
    if haystack.contains("syntax error") {
        return ErrorKind::SyntaxError;
    }
    if haystack.contains("undefined variable") {
        return ErrorKind::UndefinedVariable;
    }
    if haystack.contains("does not exist")
        && (haystack.contains("option") || haystack.contains("definition values"))
    {
        return ErrorKind::UndefinedOption;
    }
    if haystack.contains("attribute") && haystack.contains("missing") {
        return ErrorKind::MissingAttribute;
    }
    if NETWORK.iter().any(|needle| haystack.contains(needle)) {
        return ErrorKind::Network;
    }
    if haystack.contains("builder for") || haystack.contains("failed with exit code") {
        return ErrorKind::BuilderFailed;
    }
    if haystack.contains("permission denied") {
        return ErrorKind::PermissionDenied;
    }
    if haystack.contains("no space left on device") {
        return ErrorKind::DiskFull;
    }
    if haystack.contains("no such file or directory") {
        return ErrorKind::MissingFile;
    }
    if haystack.contains("error:") {
        ErrorKind::Evaluation
    } else {
        ErrorKind::Unknown
    }
}

fn hint_for(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Interrupted => "Nothing was applied. Run the build again when you are ready.",
        ErrorKind::UnfreePackage => {
            "Nix will not accept this licence without an explicit opt-in. Either allow unfree \
             packages in your configuration, or choose a free alternative."
        }
        ErrorKind::HashMismatch => {
            "A fixed-output derivation produced different content than its hash records. If you \
             did not change the source, treat this as tampering and do not apply."
        }
        ErrorKind::UndefinedOption => {
            "Home Manager does not know this option in your version. Check the name against the \
             options list for the release you are on."
        }
        ErrorKind::MissingAttribute => {
            "Something the configuration refers to does not exist. The line above the message \
             names the attribute to fix."
        }
        ErrorKind::UndefinedVariable => {
            "A name is used before it is defined. Check the spelling and whether it is bound in \
             a `let` block."
        }
        ErrorKind::SyntaxError => {
            "The file is not valid Nix. Fix the first reported position; later errors are often \
             consequences of it."
        }
        ErrorKind::MissingFile => {
            "A path in the configuration does not exist. Check the file name and its `./` prefix."
        }
        ErrorKind::Network => {
            "Evaluation needed to download something. Check the connection and try again; nothing \
             was applied."
        }
        ErrorKind::PermissionDenied => {
            "The filesystem refused access. This application never uses elevated privileges, so \
             the file probably belongs to another user."
        }
        ErrorKind::DiskFull => {
            "The disk is full. Free some space — Nix keeps old store paths until they are \
             collected."
        }
        ErrorKind::BuilderFailed => {
            "A package's build script failed. The last log lines above show which command, in \
             which package."
        }
        ErrorKind::Evaluation => {
            "Evaluation failed before anything was built. The message above is Nix's own."
        }
        ErrorKind::Unknown => {
            "The command failed and printed nothing this application recognises. The raw output is \
             shown above."
        }
    }
}

/// Remove terminal escape sequences, which nix keeps in messages even when its
/// output is not a terminal. The donated parser does this for structured fields;
/// plain stderr needs it too.
use crate::donated::nix_log::strip_ansi;

/// Find `path:line:column` for a `.nix` file in the output.
fn location_in(text: &str) -> Option<String> {
    const MARKER: &str = ".nix:";
    let marker_at = text.rfind(MARKER)?;
    let start = text
        .get(..marker_at)?
        .rfind(|character: char| {
            character.is_whitespace() || matches!(character, '\'' | '"' | '`' | '(' | '[' | ':')
        })
        .map_or(0, |index| index + 1);
    let rest = text.get(marker_at + MARKER.len()..)?;
    let mut fields = rest.split(':');
    let line = fields.next()?;
    let column = fields.next()?;
    let digits = |value: &str| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit());
    if !digits(line) || !digits(column) {
        return None;
    }
    let end = marker_at + MARKER.len() + line.len() + 1 + column.len();
    text.get(start..end).map(str::to_string)
}
