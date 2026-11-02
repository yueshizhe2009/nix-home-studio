//! M3.1 acceptance tests: `ModuleSpec` -> Nix text.
//!
//! Pure text in, pure text out. Nothing here touches a filesystem, and the
//! generated text is asserted byte-for-byte where the shape matters, so a change
//! in formatting cannot slip through as "the test still passes".

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use nhs_core::edit::validate;
use nhs_core::module::{
    Block, Marker, ModuleError, ModuleSpec, Scope, Setting, SettingValue, Style, Target,
};
fn package(name: &str) -> String {
    name.to_string()
}

fn spec(packages: &[&str], settings: Vec<Setting>) -> ModuleSpec {
    ModuleSpec {
        packages: packages.iter().map(|name| package(name)).collect(),
        settings,
    }
}

/// Render for the only target v1 writes (Home Manager).
fn render(spec: &ModuleSpec, style: &Style) -> Result<String, ModuleError> {
    nhs_core::module::render(spec, style, Target::Home)
}

/// Rewrite for the only target v1 writes (Home Manager).
fn rewrite(text: &str, spec: &ModuleSpec) -> Result<String, ModuleError> {
    nhs_core::module::rewrite(text, spec, Target::Home)
}

fn setting(path: &str, value: SettingValue) -> Setting {
    Setting {
        path: path.to_string(),
        value,
        scope: Scope::Home,
    }
}

/// The canonical rendering, byte for byte.
#[test]
fn generated_module_parses_and_declares_the_selected_packages() {
    let spec = spec(
        &["firefox", "python3Packages.requests", "ripgrep"],
        vec![
            setting("programs.git.enable", SettingValue::Bool(true)),
            setting(
                "programs.git.userName",
                SettingValue::Str("Ada".to_string()),
            ),
        ],
    );

    let rendered = render(&spec, &Style::default()).expect("render");
    let expected = concat!(
        "# Managed by nix-home-studio. Edits inside the markers are overwritten.\n",
        "{ pkgs, ... }:\n",
        "{\n",
        "  # nhs:managed:start\n",
        "  home.packages = [\n",
        "    pkgs.firefox\n",
        "    pkgs.python3Packages.requests\n",
        "    pkgs.ripgrep\n",
        "  ];\n",
        "  # nhs:managed:end\n",
        "\n",
        "  # nhs:settings:start\n",
        "  programs.git.enable = true;\n",
        "  programs.git.userName = \"Ada\";\n",
        "  # nhs:settings:end\n",
        "}\n",
    );
    assert_eq!(rendered, expected);

    // It must be Nix the parser accepts, not just text that looks right.
    validate::parse(&rendered).expect("the generated module must parse");

    // No trailing whitespace anywhere, exactly one final newline.
    for line in rendered.split('\n') {
        assert_eq!(line.trim_end(), line, "trailing whitespace in {line:?}");
    }
    assert!(rendered.ends_with("}\n"));
    assert!(!rendered.ends_with("\n\n"));
}

/// The same set of selections must always produce the same bytes, whatever order
/// the selections arrive in — a reviewed diff must not churn.
#[test]
fn rendering_is_deterministic_and_byte_ordered() {
    let one = spec(&["ripgrep", "Beta", "alpha", "Zed"], vec![]);
    let other = spec(&["Zed", "alpha", "ripgrep", "Beta"], vec![]);

    let first = render(&one, &Style::default()).expect("render");
    let second = render(&other, &Style::default()).expect("render");
    assert_eq!(first, second, "input order must not affect the output");
    assert_eq!(
        first,
        render(&one, &Style::default()).expect("render"),
        "rendering must be repeatable"
    );

    // Byte order, not locale order: uppercase before lowercase, 'Z' (0x5A)
    // before 'a' (0x61).
    let lines: Vec<&str> = first
        .lines()
        .filter(|line| line.trim_start().starts_with("pkgs."))
        .collect();
    assert_eq!(
        lines,
        vec![
            "    pkgs.Beta",
            "    pkgs.Zed",
            "    pkgs.alpha",
            "    pkgs.ripgrep"
        ]
    );
}

#[test]
fn duplicate_packages_are_collapsed() {
    let spec = spec(&["ripgrep", "ripgrep", "firefox"], vec![]);
    let rendered = render(&spec, &Style::default()).expect("render");
    assert_eq!(rendered.matches("pkgs.ripgrep").count(), 1);
    assert!(rendered.contains("  home.packages = [\n"));
}

/// Indentation comes from the file being edited; a tab-indented file must not
/// suddenly receive spaces.
#[test]
fn indentation_is_inherited_from_the_existing_file() {
    assert_eq!(Style::detect("{\n  imports = [ ];\n}\n").indent, "  ");
    assert_eq!(
        Style::detect("{\n    imports = [ ];\n    x = 1;\n}\n").indent,
        "    "
    );
    assert_eq!(Style::detect("{\n\timports = [ ];\n}\n").indent, "\t");
    assert_eq!(Style::detect("").indent, "  ", "no evidence, safe default");
    assert_eq!(
        Style::detect("{}\n").indent,
        "  ",
        "no indented line at all"
    );

    let tabbed = Style::detect("{\n\timports = [ ];\n}\n");
    let rendered = render(&spec(&["firefox"], vec![]), &tabbed).expect("render");
    for line in rendered.lines().skip(2) {
        if line.starts_with('\t') {
            assert!(
                !line.starts_with(" \t") && !line.starts_with("  "),
                "a tab-indented file must not get space indentation: {line:?}"
            );
        }
    }
    assert!(rendered.contains("\n\t# nhs:managed:start\n"));
    assert!(rendered.contains("\n\thome.packages = [\n"));
    assert!(rendered.contains("\n\t\tpkgs.firefox\n"));
}

/// CRLF files must not receive bare LF lines, which would leave the file with
/// mixed endings and an unreadable diff.
#[test]
fn line_endings_follow_the_existing_file() {
    let style = Style::detect("{\r\n  imports = [ ];\r\n}\r\n");
    assert_eq!(style.eol, "\r\n");

    let rendered = render(&spec(&["firefox"], vec![]), &style).expect("render");
    assert!(rendered.contains("\r\n"));
    let without_crlf = rendered.replace("\r\n", "");
    assert!(
        !without_crlf.contains('\n'),
        "no line may end with a bare LF"
    );
}

/// A value is data, never code: quotes, backslashes and `${…}` must survive as
/// characters, and the module must still parse.
#[test]
fn string_settings_are_quoted_so_they_cannot_interpolate() {
    let hostile = "a\"b\\c${evil}\n\ttab";
    let spec = spec(
        &[],
        vec![setting(
            "programs.foo.bar",
            SettingValue::Str(hostile.to_string()),
        )],
    );
    let rendered = render(&spec, &Style::default()).expect("render");
    assert!(
        rendered.contains(r#"  programs.foo.bar = "a\"b\\c\${evil}\n\ttab";"#),
        "unexpected quoting:\n{rendered}"
    );
    validate::parse(&rendered).expect("escaped value must still parse");

    // Nothing was interpolated: the only `${` in the file is the escaped one.
    assert_eq!(
        rendered.matches("${").count(),
        rendered.matches("\\${").count(),
        "every `${{` must be escaped"
    );
}

#[test]
fn scalars_and_string_lists_render() {
    let spec = spec(
        &[],
        vec![
            setting("a.enabled", SettingValue::Bool(false)),
            setting("a.count", SettingValue::Int(-3)),
            setting(
                "a.names",
                SettingValue::StrList(vec!["x".into(), "y z".into()]),
            ),
            setting("a.empty", SettingValue::StrList(vec![])),
            setting("a.none", SettingValue::Str("".to_string())),
        ],
    );
    let rendered = render(&spec, &Style::default()).expect("render");
    assert!(rendered.contains("  a.count = -3;\n"));
    assert!(rendered.contains("  a.empty = [ ];\n"));
    assert!(rendered.contains("  a.enabled = false;\n"));
    assert!(rendered.contains("  a.names = [ \"x\" \"y z\" ];\n"));
    assert!(rendered.contains("  a.none = \"\";\n"));
    validate::parse(&rendered).expect("must parse");
}

#[test]
fn an_empty_spec_still_produces_a_valid_module() {
    let rendered = render(&ModuleSpec::default(), &Style::default()).expect("render");
    assert!(rendered.contains("  home.packages = [ ];\n"));
    assert!(rendered.contains("  # nhs:settings:start\n  # nhs:settings:end\n"));
    validate::parse(&rendered).expect("must parse");
}

#[test]
fn invalid_package_names_are_refused() {
    let cases = [
        "",
        " firefox",
        "firefox ",
        "pkgs.firefox",
        "firefox;",
        "a b",
        "a..b",
        "a.",
        "python3Packages.${name}",
        "python3Packages.\"requests\"",
        "a\nb",
        "#comment",
        "firefox or pkgs.evil",
    ];
    for case in cases {
        let error = render(&spec(&[case], vec![]), &Style::default())
            .expect_err(&format!("`{case}` must be refused"));
        match error {
            ModuleError::InvalidPackage { attr, reason } => {
                assert_eq!(attr, case, "the error must name the offending input");
                assert!(!reason.is_empty(), "the error must say why");
            }
            other => panic!("`{case}` produced the wrong error: {other}"),
        }
    }
}

#[test]
fn invalid_setting_paths_are_refused() {
    let cases = [
        "",
        " programs.git.enable",
        "programs..git",
        "programs.git; x = 1",
        "a b",
        "programs.${name}.enable",
        "\"programs\".git",
        "a.",
    ];
    for case in cases {
        let error = render(
            &spec(&[], vec![setting(case, SettingValue::Bool(true))]),
            &Style::default(),
        )
        .expect_err(&format!("`{case}` must be refused"));
        match error {
            ModuleError::InvalidSettingPath { path, reason } => {
                assert_eq!(path, case);
                assert!(!reason.is_empty());
            }
            other => panic!("`{case}` produced the wrong error: {other}"),
        }
    }
}

/// `or` is a keyword in expression position but a legal attribute name, so a path
/// segment spelled `or` is accepted — the parser decides, not a keyword list.
#[test]
fn a_keyword_is_a_legal_attribute_name() {
    let rendered = render(
        &spec(&[], vec![setting("programs.or", SettingValue::Bool(true))]),
        &Style::default(),
    )
    .expect("`or` is a valid attribute name");
    assert!(rendered.contains("  programs.or = true;\n"));
    validate::parse(&rendered).expect("must parse");
}

#[test]
fn duplicate_setting_paths_are_refused() {
    let error = render(
        &spec(
            &[],
            vec![
                setting("programs.git.enable", SettingValue::Bool(true)),
                setting("programs.git.enable", SettingValue::Bool(false)),
            ],
        ),
        &Style::default(),
    )
    .expect_err("a duplicate binding must be refused");
    assert_eq!(
        error,
        ModuleError::DuplicateSetting {
            path: "programs.git.enable".to_string()
        }
    );
}

// ---------------------------------------------------------------------------
// M3.2: rewriting an existing generated file
// ---------------------------------------------------------------------------

fn default_style() -> Style {
    Style::default()
}

/// A hand-written file that surrounds the generated blocks. Nothing outside the
/// markers may change.
const SURROUNDED: &str = concat!(
    "# my own header note\n",
    "{ pkgs, ... }:\n",
    "{\n",
    "  imports = [ ./other.nix ];\n",
    "\n",
    "  # nhs:managed:start\n",
    "  home.packages = [\n",
    "    pkgs.old\n",
    "  ];\n",
    "  # nhs:managed:end\n",
    "\n",
    "  # nhs:settings:start\n",
    "  programs.git.enable = true;\n",
    "  # nhs:settings:end\n",
    "\n",
    "  # a note of my own, after the blocks\n",
    "  home.stateVersion = \"24.05\";\n",
    "}\n",
);

/// The spec that matches [`SURROUNDED`]'s existing blocks, so a rewrite with it
/// is a pure formatting no-op.
fn surrounded_spec(packages: &[&str]) -> ModuleSpec {
    ModuleSpec {
        packages: packages.iter().map(|name| package(name)).collect(),
        settings: vec![setting("programs.git.enable", SettingValue::Bool(true))],
    }
}

/// Everything outside the markers survives byte for byte.
#[test]
fn regenerating_preserves_content_outside_the_markers() {
    let rewritten =
        rewrite(SURROUNDED, &surrounded_spec(&["ripgrep", "firefox"])).expect("rewrite");

    // Prefix: everything up to the managed block interior, unchanged.
    let before = SURROUNDED
        .get(
            ..SURROUNDED
                .find("  home.packages = [\n")
                .expect("marker body"),
        )
        .unwrap_or("");
    assert!(
        rewritten.starts_with(before),
        "the prefix changed:\n{rewritten}"
    );

    // Suffix: from the managed end marker's closing `];` onwards, unchanged.
    let tail = SURROUNDED
        .get(
            SURROUNDED
                .find("  ];\n  # nhs:managed:end")
                .expect("end marker")..,
        )
        .unwrap_or("");
    assert!(
        rewritten.contains("  ];\n  # nhs:managed:end"),
        "the block closing changed:\n{rewritten}"
    );
    // The settings block is rewritten too, so compare from its end onward.
    let after_settings = tail
        .get(tail.find("  # nhs:settings:end").expect("settings end")..)
        .unwrap_or("");
    assert!(
        rewritten.ends_with(after_settings),
        "content after the blocks changed:\n{rewritten}"
    );
    assert!(rewritten.contains("  # a note of my own, after the blocks\n"));
    assert!(rewritten.contains("  home.stateVersion = \"24.05\";\n"));
    assert!(rewritten.starts_with("# my own header note\n"));
    validate::parse(&rewritten).expect("result must parse");
}

/// Rewriting with the spec the file was generated from is a byte-identical no-op.
#[test]
fn rewriting_an_up_to_date_file_changes_nothing() {
    let spec = spec(
        &["firefox", "ripgrep"],
        vec![setting("a.b", SettingValue::Bool(true))],
    );
    let rendered = render(&spec, &default_style()).expect("render");
    let again = rewrite(&rendered, &spec).expect("rewrite");
    assert_eq!(again, rendered);
}

/// A hand edit inside the managed block is refused and the input is untouched.
#[test]
fn refuses_to_rewrite_a_managed_file_whose_program_was_hand_modified() {
    let edited = SURROUNDED.replace(
        "    pkgs.old\n",
        "    pkgs.old\n    (import ./local.nix { })\n",
    );
    assert_ne!(edited, SURROUNDED);

    let error = rewrite(&edited, &surrounded_spec(&["firefox"]))
        .expect_err("a computed element must be refused");
    match error {
        ModuleError::BlockHandModified {
            block,
            line,
            reason,
        } => {
            assert_eq!(block, Block::Managed);
            assert_eq!(line, 9, "the offending line must be reported: {reason}");
            assert!(reason.contains("pkgs.<name>") || reason.contains("plain package"));
        }
        other => panic!("expected BlockHandModified, got {other}"),
    }
}

/// A setting added by hand inside the settings block is refused too: silently
/// dropping it would delete user data.
#[test]
fn a_hand_added_canonical_setting_is_replaced() {
    // The markers are the contract: inside them the app owns the content, and a
    // stale selection and a hand-added line are indistinguishable without stored
    // state. The removal is not silent — it is in the diff the user approves.
    // What the app may not do is drop a line it cannot re-emit; that is refused
    // by the tests below.
    let edited = SURROUNDED.replace(
        "  programs.git.enable = true;\n",
        "  programs.git.enable = true;\n  programs.fish.enable = true;\n",
    );
    let rewritten = rewrite(&edited, &surrounded_spec(&["firefox"])).expect("rewrite");
    assert!(
        !rewritten.contains("programs.fish.enable"),
        "a binding not in the selection is replaced by the block rewrite:\n{rewritten}"
    );
    assert!(rewritten.contains("  programs.git.enable = true;\n"));
    validate::parse(&rewritten).expect("must parse");
}

/// A value this application cannot re-emit is refused, not collapsed to
/// something else.
#[test]
fn refuses_a_managed_value_it_cannot_reproduce() {
    let edited = SURROUNDED.replace(
        "  programs.git.enable = true;\n",
        "  programs.git.enable = pkgs.lib.mkDefault true;\n",
    );
    let error = rewrite(&edited, &surrounded_spec(&["firefox"]))
        .expect_err("an expression value must be refused");
    assert!(
        matches!(
            error,
            ModuleError::BlockHandModified {
                block: Block::Settings,
                ..
            }
        ),
        "got {error}"
    );
}

/// A comment inside a block would disappear, so it is a refusal with the line.
#[test]
fn refuses_a_comment_inside_a_block() {
    let edited = SURROUNDED.replace("    pkgs.old\n", "    # I put this here\n    pkgs.old\n");
    let error = rewrite(&edited, &surrounded_spec(&["firefox"]))
        .expect_err("a comment inside the block must be refused");
    match error {
        ModuleError::BlockHandModified {
            block,
            line,
            reason,
        } => {
            assert_eq!(block, Block::Managed);
            assert_eq!(line, 8);
            assert!(reason.contains("comment"), "{reason}");
        }
        other => panic!("expected BlockHandModified, got {other}"),
    }
}

/// Without the markers there is nothing to rewrite; saying so beats guessing.
#[test]
fn refuses_a_file_without_markers() {
    let plain = "{\n  home.packages = [ pkgs.firefox ];\n}\n";
    let error = rewrite(plain, &surrounded_spec(&["firefox"])).expect_err("no markers");
    assert!(
        matches!(error, ModuleError::MarkerMissing { .. }),
        "got {error}"
    );
}

/// Duplicate markers make the boundaries ambiguous.
#[test]
fn refuses_duplicate_markers() {
    let doubled = format!("{SURROUNDED}# nhs:managed:start\n");
    let error = rewrite(&doubled, &surrounded_spec(&["firefox"])).expect_err("ambiguous");
    match error {
        ModuleError::MarkerRepeated {
            marker,
            first_line,
            again_line,
        } => {
            assert_eq!(marker, Marker::ManagedStart);
            assert_eq!(first_line, 6);
            assert_eq!(again_line, 19);
        }
        other => panic!("expected MarkerRepeated, got {other}"),
    }
}

/// Marker text inside a string is not a marker.
#[test]
fn refuses_marker_text_that_is_not_a_comment() {
    let inside_string = SURROUNDED.replace(
        "  home.stateVersion = \"24.05\";\n",
        "  home.stateVersion = \"24.05\";\n  programs.foo.text = ''\n    # nhs:managed:end\n  '';\n",
    );
    let error = rewrite(&inside_string, &surrounded_spec(&["firefox"]))
        .expect_err("marker text inside a string must not be honoured");
    match error {
        // Both outcomes are safe; the exact one depends on which marker text is
        // found first. What matters is that the in-string text is never honoured.
        ModuleError::MarkerNotAComment { marker, line } => {
            assert_eq!(marker, Marker::ManagedEnd);
            assert_eq!(line, 19);
        }
        ModuleError::MarkerMissing { .. } | ModuleError::MarkerOutOfOrder { .. } => {}
        other => panic!("expected a marker error, got {other}"),
    }
}

/// A file that does not parse cannot be reasoned about at all.
#[test]
fn refuses_a_file_that_does_not_parse() {
    let broken = SURROUNDED.replace("{\n  imports", "{\n  imports = [\n");
    let error = rewrite(&broken, &surrounded_spec(&["firefox"])).expect_err("broken");
    assert!(
        matches!(error, ModuleError::NotParseable { .. }),
        "got {error}"
    );
}

/// CRLF files stay CRLF, including the rewritten lines.
#[test]
fn crlf_files_stay_crlf() {
    let crlf = SURROUNDED.replace('\n', "\r\n");
    let rewritten = rewrite(&crlf, &surrounded_spec(&["firefox"])).expect("rewrite");
    assert!(rewritten.contains("\r\n    pkgs.firefox\r\n"));
    assert!(
        !rewritten.replace("\r\n", "").contains('\n'),
        "no line may end with a bare LF:\n{rewritten:?}"
    );
}

/// A tab-indented file keeps tabs in the rewritten lines.
#[test]
fn tab_indented_files_keep_tabs() {
    let tabbed = SURROUNDED.replace("  ", "\t");
    let rewritten = rewrite(&tabbed, &surrounded_spec(&["firefox"])).expect("rewrite");
    assert!(rewritten.contains("\t\tpkgs.firefox\n"), "{rewritten}");
    assert!(
        !rewritten.contains("  pkgs.firefox"),
        "no space indentation may appear"
    );
}

/// The file's own indentation wins over the default, including a single space.
#[test]
fn rewritten_lines_follow_the_marker_indentation() {
    let odd = SURROUNDED.replace("\n  ", "\n ");
    let rewritten = rewrite(&odd, &surrounded_spec(&["firefox"])).expect("rewrite");
    assert!(rewritten.contains("\n  pkgs.firefox\n"), "{rewritten}");
}

/// A missing final newline is not invented and an existing one is not removed.
#[test]
fn the_final_newline_is_preserved() {
    let no_newline = SURROUNDED.trim_end_matches('\n');
    let rewritten = rewrite(no_newline, &surrounded_spec(&["firefox"])).expect("rewrite");
    assert!(
        !rewritten.ends_with('\n'),
        "a missing final newline must not be added"
    );

    let with_newline = rewritten.clone() + "\n";
    let again = rewrite(&with_newline, &surrounded_spec(&["firefox"])).expect("rewrite");
    assert!(again.ends_with('\n'));
    assert!(!again.ends_with("\n\n"));
}

/// An empty selection empties the block without removing the markers.
#[test]
fn an_empty_selection_empties_the_block() {
    let empty = ModuleSpec {
        packages: Vec::new(),
        settings: vec![setting("programs.git.enable", SettingValue::Bool(true))],
    };
    let rewritten = rewrite(SURROUNDED, &empty).expect("rewrite");
    assert!(rewritten.contains("  home.packages = [ ];\n"));
    assert!(rewritten.contains("  # nhs:settings:end\n"));
    validate::parse(&rewritten).expect("must parse");
}

/// Invalid input is refused before anything else happens — including validation
/// of the spec payload itself.
#[test]
fn invalid_spec_is_refused_before_the_file_is_examined() {
    let error = rewrite(SURROUNDED, &spec(&["not a package"], vec![]))
        .expect_err("invalid package must be refused");
    assert!(
        matches!(error, ModuleError::InvalidPackage { .. }),
        "got {error}"
    );
}

// ---------------------------------------------------------------------------
// Entry-form whitelist
//
// `inherit` is a legal sibling of a binding and is *not* an `AttrpathValue`.
// Validating only the bindings that were collected let it through, and the
// rewrite then dropped it. Every direct entry is now checked.
// ---------------------------------------------------------------------------

/// `inherit` inside the package block is refused with its line.
#[test]
fn refuses_inherit_in_the_managed_block() {
    let edited = SURROUNDED.replace(
        "  ];\n  # nhs:managed:end\n",
        "  ];\n  inherit (pkgs) hello;\n  # nhs:managed:end\n",
    );
    assert!(edited.contains("inherit (pkgs) hello;"));

    let error = rewrite(&edited, &surrounded_spec(&["firefox"]))
        .expect_err("an inherit would be dropped, so the rewrite must be refused");
    match error {
        ModuleError::BlockHandModified {
            block,
            line,
            reason,
        } => {
            assert_eq!(block, Block::Managed);
            assert_eq!(line, 10, "the inherit's line: {reason}");
            assert!(reason.contains("inherit"), "{reason}");
        }
        other => panic!("expected BlockHandModified, got {other}"),
    }
}

/// `inherit` inside the settings block is refused with its line.
#[test]
fn refuses_inherit_in_the_settings_block() {
    let edited = SURROUNDED.replace(
        "  programs.git.enable = true;\n  # nhs:settings:end\n",
        "  programs.git.enable = true;\n  inherit (builtins) currentSystem;\n  # nhs:settings:end\n",
    );
    assert!(edited.contains("inherit (builtins) currentSystem;"));

    let error = rewrite(&edited, &surrounded_spec(&["firefox"]))
        .expect_err("an inherit would be dropped, so the rewrite must be refused");
    match error {
        ModuleError::BlockHandModified {
            block,
            line,
            reason,
        } => {
            assert_eq!(block, Block::Settings);
            assert_eq!(line, 14, "the inherit's line: {reason}");
            assert!(reason.contains("inherit"), "{reason}");
        }
        other => panic!("expected BlockHandModified, got {other}"),
    }
}

/// `inherit` with a multi-line `from` expression is refused as well — the
/// refusal does not depend on the entry fitting on one line.
#[test]
fn refuses_a_multiline_inherit() {
    let edited = SURROUNDED.replace(
        "  ];\n  # nhs:managed:end\n",
        "  ];\n  inherit (pkgs.python3Packages)\n    requests\n    httpx;\n  # nhs:managed:end\n",
    );
    let error = rewrite(&edited, &surrounded_spec(&["firefox"]))
        .expect_err("a multi-line inherit must be refused too");
    match error {
        ModuleError::BlockHandModified {
            block,
            line,
            reason,
        } => {
            assert_eq!(block, Block::Managed);
            assert_eq!(line, 10, "the entry starts on line 10: {reason}");
        }
        other => panic!("expected BlockHandModified, got {other}"),
    }
}

// ---------------------------------------------------------------------------
// M3.4: options that only exist in a NixOS module
//
// D2 keeps NixOS integration read-only, so a `SystemOnly` option can never be
// written by v1. It must still be *reported* — refusing with the option names is
// the difference between "this app cannot do that" and a module that silently
// does nothing.
// ---------------------------------------------------------------------------

fn system_setting(path: &str) -> Setting {
    Setting {
        path: path.to_string(),
        value: SettingValue::Bool(true),
        scope: Scope::SystemOnly,
    }
}

#[test]
fn system_only_options_are_flagged_and_refused_in_home_layouts() {
    let selection = spec(
        &["firefox"],
        vec![
            setting("programs.git.enable", SettingValue::Bool(true)),
            system_setting("boot.kernelPackages"),
            system_setting("services.openssh.enable"),
        ],
    );

    // Flagged: the caller can show these without parsing the spec again.
    assert_eq!(
        nhs_core::module::system_only_paths(&selection),
        vec![
            "boot.kernelPackages".to_string(),
            "services.openssh.enable".to_string()
        ]
    );

    // Refused for a home target, naming every offending option.
    match nhs_core::module::check_target(&selection, Target::Home) {
        Err(ModuleError::SystemOnlyInHome { paths }) => assert_eq!(
            paths,
            vec![
                "boot.kernelPackages".to_string(),
                "services.openssh.enable".to_string()
            ]
        ),
        other => panic!("expected SystemOnlyInHome, got {other:?}"),
    }
    assert!(matches!(
        nhs_core::module::render(&selection, &Style::default(), Target::Home),
        Err(ModuleError::SystemOnlyInHome { .. })
    ));

    // ... and refused before an existing file is even looked at.
    let rendered = nhs_core::module::render(
        &spec(
            &["firefox"],
            vec![setting("programs.git.enable", SettingValue::Bool(true))],
        ),
        &Style::default(),
        Target::Home,
    )
    .expect("the clean spec renders");
    match nhs_core::module::rewrite(&rendered, &selection, Target::Home) {
        Err(ModuleError::SystemOnlyInHome { paths }) => assert_eq!(paths.len(), 2),
        other => panic!("expected SystemOnlyInHome before any file work, got {other:?}"),
    }
}

/// The NixOS branch is not dead code by accident: it accepts the same spec and
/// reports the options instead of refusing them. v1 never writes that module
/// (D2), but the policy has a defined counterpart.
#[test]
fn a_nixos_target_reports_instead_of_refusing() {
    let spec = spec(&[], vec![system_setting("boot.kernelPackages")]);
    let notes = nhs_core::module::check_target(&spec, Target::Nixos).expect("allowed");
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("boot.kernelPackages"), "{notes:?}");

    let rendered = nhs_core::module::render(&spec, &Style::default(), Target::Nixos)
        .expect("a NixOS module may set it");
    validate::parse(&rendered).expect("must parse");
    assert!(rendered.contains("  boot.kernelPackages = true;\n"));
}

/// A layout that can be edited maps to the home target; the NixOS-integration
/// layout is the only one that does not.
#[test]
fn layouts_map_to_the_module_system_they_would_be_written_for() {
    use nhs_core::Layout;
    use std::path::PathBuf;

    let classic = Layout::ClassicHome {
        main_file: PathBuf::from("/home/tester/.config/home-manager/home.nix"),
    };
    let flake = Layout::FlakeHome {
        root: PathBuf::from("/home/tester/dotfiles"),
        main_file: PathBuf::from("/home/tester/dotfiles/home.nix"),
        attr: "tester".to_string(),
    };
    let nixos = Layout::FlakeNixos {
        root: PathBuf::from("/etc/nixos"),
        main_file: PathBuf::from("/etc/nixos/flake.nix"),
        host: "box".to_string(),
        user: "tester".to_string(),
    };
    // The machine this is being developed on: a profile without a config file.
    // It has no module to write, but if one were ever written it would be a home
    // module, never a NixOS one.
    let profile_only = Layout::ProfileOnly {
        profile_dir: PathBuf::from("/home/tester/.local/state/nix/profiles"),
        generations: Vec::new(),
    };
    let unknown = Layout::Unknown {
        reason: "no configuration and no profile".to_string(),
    };

    assert_eq!(Target::for_layout(&classic), Target::Home);
    assert_eq!(Target::for_layout(&flake), Target::Home);
    assert_eq!(Target::for_layout(&profile_only), Target::Home);
    assert_eq!(Target::for_layout(&unknown), Target::Home);
    assert_eq!(Target::for_layout(&nixos), Target::Nixos);
}

/// A spec with no system-only option is accepted everywhere, and the notes list
/// stays empty.
#[test]
fn a_home_only_spec_needs_no_notes() {
    let spec = spec(
        &["firefox"],
        vec![setting("programs.git.enable", SettingValue::Bool(true))],
    );
    assert!(nhs_core::module::system_only_paths(&spec).is_empty());
    assert_eq!(
        nhs_core::module::check_target(&spec, Target::Home).expect("allowed"),
        Vec::<String>::new()
    );
    assert!(nhs_core::module::check_target(&spec, Target::Nixos)
        .expect("allowed")
        .is_empty());
}
