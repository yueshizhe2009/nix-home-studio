//! M4.2 golden corpus: stderr → a stable kind, a summary and a hint.
//!
//! **Provenance, stated per case.** No `nix` command was run to produce these
//! texts (that is a standing constraint). Each case says where its text comes
//! from:
//!
//! * `DonorCapture` — copied verbatim from the NixBox test suite at commit
//!   `7bab22d`: a real captured `@nix` structured message.
//! * `Constructed` — written to match the message *format* this classifier
//!   targets, from the wording Nix and Home Manager use for that failure. It is
//!   labelled as constructed rather than presented as a capture. When a real
//!   transcript is available it can replace these bodies; the assertions on
//!   `kind`, `location` and the summary being non-empty stay the same.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

use nhs_core::classify::{classify_stderr, diagnose, Diagnosis, ErrorKind};
use nhs_core::proc::Outcome;

enum Source {
    /// Verbatim from an upstream test at commit `7bab22d`.
    DonorCapture(&'static str),
    /// Written to match the documented format; not a capture.
    Constructed,
}

struct Case {
    name: &'static str,
    stderr: &'static str,
    kind: ErrorKind,
    location: Option<&'static str>,
    source: Source,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "undefined variable from a captured structured log",
            // Verbatim from NixBox's progress tests, which assert this exact
            // record renders as `error: undefined variable 'foo'`.
            stderr: "@nix {\"action\":\"msg\",\"level\":0,\"msg\":\"\\u001b[31;1merror:\\u001b[0m undefined variable 'foo'\"}",
            kind: ErrorKind::UndefinedVariable,
            location: None,
            source: Source::DonorCapture(
                "nixbox@7bab22d crates/nixbox-core/src/progress.rs:778",
            ),
        },
        Case {
            name: "unfree package",
            stderr: "error: Package ‘some-app-1.0’ in /nix/store/abc-source has an unfree \
                     license (‘unfree’); refuse to evaluate the package.",
            kind: ErrorKind::UnfreePackage,
            location: None,
            source: Source::Constructed,
        },
        Case {
            name: "hash mismatch",
            stderr: "error: hash mismatch in fixed-output derivation \
                     '/nix/store/aaa-foo.drv':\n         specified: sha256-AAAA\n            got: \
                     sha256-BBBB",
            kind: ErrorKind::HashMismatch,
            location: None,
            source: Source::Constructed,
        },
        Case {
            name: "home manager option that does not exist",
            stderr: "error: The option `programs.nope.enable' does not exist. Definition values:\n\
                     - In `/home/tester/.config/home-manager/home.nix':",
            kind: ErrorKind::UndefinedOption,
            location: None,
            source: Source::Constructed,
        },
        Case {
            name: "missing attribute",
            stderr: "error: attribute 'nope' missing",
            kind: ErrorKind::MissingAttribute,
            location: None,
            source: Source::Constructed,
        },
        Case {
            name: "syntax error with a position",
            stderr: "error: syntax error, unexpected end of file, at \
                     /home/tester/.config/home-manager/home.nix:12:1",
            kind: ErrorKind::SyntaxError,
            location: Some("/home/tester/.config/home-manager/home.nix:12:1"),
            source: Source::Constructed,
        },
        Case {
            name: "network failure while fetching",
            stderr: "error: unable to download 'https://example.invalid/x': Could not resolve \
                     host: example.invalid",
            kind: ErrorKind::Network,
            location: None,
            source: Source::Constructed,
        },
        Case {
            name: "permission denied",
            stderr: "error: opening file '/etc/secret.conf': Permission denied",
            kind: ErrorKind::PermissionDenied,
            location: None,
            source: Source::Constructed,
        },
        Case {
            name: "disk full",
            stderr: "error: writing to file: No space left on device",
            kind: ErrorKind::DiskFull,
            location: None,
            source: Source::Constructed,
        },
        Case {
            name: "builder failed",
            stderr: "error: builder for '/nix/store/bbb-foo.drv' failed with exit code 1;\n\
                     last 2 log lines:\n  make: *** [Makefile:3: all] Error 2",
            kind: ErrorKind::BuilderFailed,
            location: None,
            source: Source::Constructed,
        },
        Case {
            name: "missing file with a position",
            stderr: "error: opening file '/home/tester/.config/home-manager/modules/typo.nix': \
                     No such file or directory, at \
                     /home/tester/.config/home-manager/home.nix:4:15",
            kind: ErrorKind::MissingFile,
            location: Some("/home/tester/.config/home-manager/home.nix:4:15"),
            source: Source::Constructed,
        },
        Case {
            name: "generic evaluation error",
            stderr: "error: value is a string while a list was expected",
            kind: ErrorKind::Evaluation,
            location: None,
            source: Source::Constructed,
        },
        Case {
            name: "nothing recognisable",
            stderr: "segfault",
            kind: ErrorKind::Unknown,
            location: None,
            source: Source::Constructed,
        },
    ]
}

#[test]
fn exit_status_and_stderr_are_classified_into_a_known_error_kind() {
    for case in cases() {
        let diagnosis = classify_stderr(case.stderr);
        assert_eq!(
            diagnosis.kind, case.kind,
            "{}: got {:?}\nstderr:\n{}",
            case.name, diagnosis, case.stderr
        );
        assert!(
            !diagnosis.summary.is_empty(),
            "{}: the summary must show the real message",
            case.name
        );
        assert!(
            !diagnosis.summary.starts_with("error:"),
            "{}: the `error:` marker is stripped",
            case.name
        );
        assert!(
            !diagnosis.hint.is_empty(),
            "{}: every kind needs a next step",
            case.name
        );
        assert_eq!(
            diagnosis.location.as_deref(),
            case.location,
            "{}: location",
            case.name
        );
        match case.source {
            Source::DonorCapture(origin) => assert!(
                origin.contains("7bab22d"),
                "a donor capture must cite the commit"
            ),
            Source::Constructed => {}
        }
    }
}

/// The structured form wins over the plain one: with `internal-json` the last
/// error-level message is the real error, even when ANSI codes wrap it.
#[test]
fn the_structured_error_message_is_preferred() {
    let stderr = concat!(
        "copying path '/nix/store/x' from 'https://cache.nixos.org'...\n",
        "@nix {\"action\":\"msg\",\"level\":0,\"msg\":\"\\u001b[31;1merror:\\u001b[0m builder for '/nix/store/a.drv' failed with exit code 1;\"}\n",
        "some trailing noise\n",
    );
    let diagnosis = classify_stderr(stderr);
    assert_eq!(diagnosis.kind, ErrorKind::BuilderFailed);
    assert_eq!(
        diagnosis.summary,
        "builder for '/nix/store/a.drv' failed with exit code 1;"
    );
    assert!(
        !diagnosis.summary.contains('\u{1b}'),
        "escape codes must not reach the UI"
    );
}

/// A stopped run is reported as interrupted, not as a build failure, and never
/// as a success.
#[test]
fn cancellation_and_timeout_are_interrupted_not_failures() {
    let cancelled = Outcome {
        status: ExitStatus::from_raw(0x000f),
        cancelled: true,
        timed_out: false,
        stdout: String::new(),
        stderr: String::new(),
        truncated: false,
    };
    let diagnosis = diagnose(&cancelled).expect("a stopped run is not success");
    assert_eq!(diagnosis.kind, ErrorKind::Interrupted);
    assert!(diagnosis.summary.contains("stopped"));

    let timed_out = Outcome {
        status: ExitStatus::from_raw(0x000f),
        cancelled: false,
        timed_out: true,
        stdout: String::new(),
        stderr: String::new(),
        truncated: false,
    };
    let diagnosis = diagnose(&timed_out).expect("a timed-out run is not success");
    assert_eq!(diagnosis.kind, ErrorKind::Interrupted);
    assert!(diagnosis.summary.contains("time"));

    // A program that exited 0 but was killed is still not a success.
    let killed = Outcome {
        status: ExitStatus::from_raw(0x0000),
        cancelled: true,
        timed_out: false,
        stdout: String::new(),
        stderr: String::new(),
        truncated: false,
    };
    assert!(!killed.success());
    assert_eq!(
        diagnose(&killed).map(|d| d.kind),
        Some(ErrorKind::Interrupted)
    );
}

/// A successful run needs no diagnosis at all.
#[test]
fn a_successful_run_has_no_diagnosis() {
    let ok = Outcome {
        status: ExitStatus::from_raw(0x0000),
        cancelled: false,
        timed_out: false,
        stdout: "bye".to_string(),
        stderr: String::new(),
        truncated: false,
    };
    assert!(ok.success());
    assert!(diagnose(&ok).is_none());
}

/// Every kind has a distinct machine-readable label, because the CLI's JSON
/// output and the GUI switch on it.
#[test]
fn every_kind_has_a_distinct_label() {
    let kinds = [
        ErrorKind::Interrupted,
        ErrorKind::UnfreePackage,
        ErrorKind::HashMismatch,
        ErrorKind::UndefinedOption,
        ErrorKind::MissingAttribute,
        ErrorKind::UndefinedVariable,
        ErrorKind::SyntaxError,
        ErrorKind::MissingFile,
        ErrorKind::Network,
        ErrorKind::PermissionDenied,
        ErrorKind::DiskFull,
        ErrorKind::BuilderFailed,
        ErrorKind::Evaluation,
        ErrorKind::Unknown,
    ];
    let mut labels: Vec<&str> = kinds.iter().map(|kind| kind.label()).collect();
    labels.sort_unstable();
    let count = labels.len();
    labels.dedup();
    assert_eq!(labels.len(), count, "labels must be unique");
}

/// The hint is advice, never a restatement: it must differ from the summary.
#[test]
fn hints_are_advice_not_a_restatement() {
    let diagnosis: Diagnosis = classify_stderr("error: attribute 'nope' missing");
    assert_ne!(diagnosis.summary, diagnosis.hint);
    assert!(!diagnosis.hint.is_empty());
}

/// Plain (non-structured) stderr still carries colour codes; they must not reach
/// the summary, and they must not stop the text from being recognised.
#[test]
fn colour_codes_in_plain_stderr_are_stripped() {
    let stderr = "\u{1b}[31;1merror:\u{1b}[0m attribute 'nope' missing";
    let diagnosis = classify_stderr(stderr);
    assert_eq!(diagnosis.kind, ErrorKind::MissingAttribute);
    assert_eq!(diagnosis.summary, "attribute 'nope' missing");
    assert!(!diagnosis.summary.contains('\u{1b}'));
}
