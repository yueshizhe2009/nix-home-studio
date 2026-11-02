//! M3.3 acceptance tests: inserting the sidecar import.
//!
//! Every positive fixture asserts the same invariant from two directions: the
//! result parses, and exactly one line differs from the input — either one line
//! added, or one line rewritten in place. Everything else must be byte-identical,
//! which is what makes the reviewed diff trustworthy.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use nhs_core::edit::validate;
use nhs_core::import::{inject_sidecar, ImportError, DEFAULT_SIDECAR};

const MULTI_LINE: &str = include_str!("fixtures/imports/multi_line.nix");
const NO_IMPORTS: &str = include_str!("fixtures/imports/no_imports.nix");
const EMPTY_LIST: &str = include_str!("fixtures/imports/empty_list.nix");
const SINGLE_LINE: &str = include_str!("fixtures/imports/classic_single_line.nix");
const LET_IN: &str = include_str!("fixtures/imports/let_in.nix");
const WITH_SCOPE: &str = include_str!("fixtures/imports/with_scope.nix");
const ALREADY: &str = include_str!("fixtures/imports/already_imported.nix");
const STRING_IMPORT: &str = include_str!("fixtures/imports/string_import.nix");
const COMPUTED_IMPORT: &str = include_str!("fixtures/imports/computed_import.nix");
const COMPUTED_ELEMENT: &str = include_str!("fixtures/imports/computed_element.nix");
const DUPLICATE_IMPORTS: &str = include_str!("fixtures/imports/duplicate_imports.nix");
const CONCAT_IMPORTS: &str = include_str!("fixtures/imports/concat_imports.nix");
const NESTED_CONFIG: &str = include_str!("fixtures/imports/nested_config.nix");
const NOT_A_MODULE: &str = include_str!("fixtures/imports/not_a_module.nix");

/// The single line that differs between input and output, if exactly one does.
fn only_one_line_changed(old: &str, new: &str) -> Option<String> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    if new_lines.len() == old_lines.len() + 1 {
        for index in 0..new_lines.len() {
            let mut without = new_lines.clone();
            let removed = without.remove(index);
            if without == old_lines {
                return Some(removed.to_string());
            }
        }
        return None;
    }
    if new_lines.len() == old_lines.len() {
        let differing: Vec<usize> = (0..old_lines.len())
            .filter(|index| old_lines[*index] != new_lines[*index])
            .collect();
        if let [only] = differing.as_slice() {
            return Some(new_lines[*only].to_string());
        }
    }
    None
}

/// Every positive fixture: one line changes, it holds the import, and the result
/// parses. `added` is recorded so the two shapes are distinguishable.
fn check_positive(name: &str, old: &str, sidecar: &str) -> String {
    let new = inject_sidecar(old, sidecar).unwrap_or_else(|error| panic!("{name}: {error}"));
    validate::parse(&new)
        .unwrap_or_else(|error| panic!("{name} produced invalid Nix: {}", error.detail));

    let changed = only_one_line_changed(old, &new)
        .unwrap_or_else(|| panic!("{name}: more than one line changed:\n{old}\n---\n{new}"));
    assert!(
        changed.contains(sidecar),
        "{name}: the changed line does not carry the import: {changed:?}"
    );
    assert_eq!(
        new.matches(sidecar).count(),
        1,
        "{name}: the import must appear exactly once"
    );
    new
}

#[test]
fn inserting_the_import_touches_only_the_imports_line() {
    for (name, fixture) in [
        ("multi_line", MULTI_LINE),
        ("no_imports", NO_IMPORTS),
        ("empty_list", EMPTY_LIST),
        ("single_line", SINGLE_LINE),
        ("let_in", LET_IN),
        ("with_scope", WITH_SCOPE),
    ] {
        check_positive(name, fixture, DEFAULT_SIDECAR);
    }
}

/// The inserted element uses the file's own indentation.
#[test]
fn the_inserted_line_follows_the_existing_indentation() {
    let new = check_positive("multi_line", MULTI_LINE, DEFAULT_SIDECAR);
    assert!(
        new.contains("\n    ./modules/nhs-packages.nix\n  ];\n"),
        "the new element must line up with its siblings:\n{new}"
    );

    // A tab-indented file gets a tab.
    let tabbed = MULTI_LINE.replace("    ", "\t\t").replace("  ", "\t");
    let new = check_positive("tabbed", &tabbed, DEFAULT_SIDECAR);
    assert!(new.contains("\n\t\t./modules/nhs-packages.nix\n"), "{new}");
}

/// CRLF files stay CRLF and gain no bare LF.
#[test]
fn crlf_files_stay_crlf() {
    let crlf = MULTI_LINE.replace('\n', "\r\n");
    let new = check_positive("crlf", &crlf, DEFAULT_SIDECAR);
    assert!(new.contains("./modules/nhs-packages.nix\r\n"));
    assert!(
        !new.replace("\r\n", "").contains('\n'),
        "no line may end with a bare LF: {new:?}"
    );
}

/// Applying the same import twice changes nothing the second time.
#[test]
fn import_insertion_is_idempotent() {
    for (name, fixture) in [
        ("multi_line", MULTI_LINE),
        ("no_imports", NO_IMPORTS),
        ("empty_list", EMPTY_LIST),
        ("single_line", SINGLE_LINE),
    ] {
        let once = inject_sidecar(fixture, DEFAULT_SIDECAR)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let twice = inject_sidecar(&once, DEFAULT_SIDECAR)
            .unwrap_or_else(|error| panic!("{name} second pass: {error}"));
        assert_eq!(twice, once, "{name}: the second injection must be a no-op");
    }

    // An existing import, however it is spelled, is recognised.
    let unchanged = inject_sidecar(ALREADY, DEFAULT_SIDECAR).expect("already imported");
    assert_eq!(
        unchanged, ALREADY,
        "an existing import must not be duplicated"
    );

    let odd_spelling =
        ALREADY.replace("./modules/nhs-packages.nix", "./modules/./nhs-packages.nix");
    let unchanged = inject_sidecar(&odd_spelling, DEFAULT_SIDECAR).expect("normalised");
    assert_eq!(
        unchanged, odd_spelling,
        "a differently spelled path must be recognised, and the file left as it is"
    );
}

/// `imports` that is not a plain list is refused, with the line.
#[test]
fn refuses_imports_that_is_not_a_list() {
    for (fixture, expected_line) in [
        (COMPUTED_IMPORT, 3),
        (CONCAT_IMPORTS, 3),
        // `imports = ./a.nix;` — a path, not a list.
        ("{ ... }:\n{\n  imports = ./a.nix;\n}\n", 3),
    ] {
        match inject_sidecar(fixture, DEFAULT_SIDECAR) {
            Err(ImportError::ImportsNotAList { line, detail }) => {
                assert_eq!(line, expected_line, "{fixture}");
                assert!(!detail.is_empty());
            }
            other => panic!("expected ImportsNotAList, got {other:?}"),
        }
    }
}

/// A computed element inside the list is refused, with its line.
#[test]
fn refuses_a_computed_element() {
    match inject_sidecar(COMPUTED_ELEMENT, DEFAULT_SIDECAR) {
        Err(ImportError::ComputedImport { line, text }) => {
            assert_eq!(line, 4);
            assert_eq!(text, "(./a.nix)");
        }
        other => panic!("expected ComputedImport, got {other:?}"),
    }
}

/// A string in the list is refused: Home Manager needs a path.
#[test]
fn refuses_a_string_element() {
    match inject_sidecar(STRING_IMPORT, DEFAULT_SIDECAR) {
        Err(ImportError::StringImport { line, text }) => {
            assert_eq!(line, 3);
            assert_eq!(text, "\"./a.nix\"");
        }
        other => panic!("expected StringImport, got {other:?}"),
    }
}

/// Two `imports` bindings make the target ambiguous.
#[test]
fn refuses_duplicate_imports() {
    match inject_sidecar(DUPLICATE_IMPORTS, DEFAULT_SIDECAR) {
        Err(ImportError::DuplicateImports {
            first_line,
            again_line,
        }) => {
            assert_eq!(first_line, 3);
            assert_eq!(again_line, 4);
        }
        other => panic!("expected DuplicateImports, got {other:?}"),
    }
}

/// `imports` inside a nested attribute set is not silently duplicated.
#[test]
fn refuses_a_nested_imports_attribute() {
    match inject_sidecar(NESTED_CONFIG, DEFAULT_SIDECAR) {
        Err(ImportError::NestedImports { line }) => assert_eq!(line, 4),
        other => panic!("expected NestedImports, got {other:?}"),
    }
}

/// A file that is not a module has no attribute set to edit.
#[test]
fn refuses_a_file_that_is_not_a_module() {
    match inject_sidecar(NOT_A_MODULE, DEFAULT_SIDECAR) {
        Err(ImportError::NoModuleAttrset { detail }) => assert!(!detail.is_empty()),
        other => panic!("expected NoModuleAttrset, got {other:?}"),
    }

    // A function whose body is not an attribute set is refused too.
    let function = "{ x }: x\n";
    assert!(matches!(
        inject_sidecar(function, DEFAULT_SIDECAR),
        Err(ImportError::NoModuleAttrset { .. })
    ));
}

/// A file that does not parse cannot be analysed.
#[test]
fn refuses_a_file_that_does_not_parse() {
    let broken = "{ pkgs, ... }:\n{\n  imports = [ ./a.nix\n";
    assert!(matches!(
        inject_sidecar(broken, DEFAULT_SIDECAR),
        Err(ImportError::NotParseable { .. })
    ));
}

/// The injected text must be a plain path, never an expression.
#[test]
fn refuses_a_sidecar_path_that_is_not_plain() {
    for bad in [
        "",
        "modules/x.nix",
        "./x y.nix",
        "./${./x}.nix",
        "\"./x.nix\"",
        "./x.nix; rm -rf /",
        "../..",
    ] {
        match inject_sidecar(MULTI_LINE, bad) {
            Err(ImportError::InvalidSidecar { path, reason }) => {
                assert_eq!(path, bad);
                assert!(!reason.is_empty());
            }
            other => panic!("`{bad}` should be refused, got {other:?}"),
        }
    }
}

/// An empty attribute set gets one binding, not a reflowed file.
#[test]
fn adds_imports_to_an_empty_attribute_set() {
    for fixture in ["{ }\n", "{\n}\n", "{ pkgs, ... }:\n{\n}\n"] {
        let new = inject_sidecar(fixture, DEFAULT_SIDECAR)
            .unwrap_or_else(|error| panic!("{fixture:?}: {error}"));
        validate::parse(&new).expect("must parse");
        assert_eq!(new.matches(DEFAULT_SIDECAR).count(), 1, "{new:?}");
        assert!(
            only_one_line_changed(fixture, &new).is_some(),
            "only one line may change: {fixture:?} -> {new:?}"
        );
    }
}

/// A one-line attribute set keeps its one-line shape.
#[test]
fn keeps_a_one_line_attribute_set_on_one_line() {
    let fixture = "{ pkgs, ... }: { home.stateVersion = \"24.05\"; }\n";
    let new = inject_sidecar(fixture, DEFAULT_SIDECAR).expect("inject");
    assert_eq!(
        new.lines().count(),
        1,
        "the line count must not grow: {new}"
    );
    validate::parse(&new).expect("must parse");
    assert!(new.contains(DEFAULT_SIDECAR));
}

/// The result is asserted byte for byte, so "one line changed" cannot hide a
/// reformatted neighbour.
#[test]
fn the_rendered_result_is_exactly_expected() {
    assert_eq!(
        inject_sidecar(MULTI_LINE, DEFAULT_SIDECAR).expect("inject"),
        concat!(
            "{ pkgs, ... }:\n",
            "{\n",
            "  imports = [\n",
            "    ./a.nix\n",
            "    ./b.nix\n",
            "    ./modules/nhs-packages.nix\n",
            "  ];\n",
            "\n",
            "  home.stateVersion = \"24.05\";\n",
            "}\n",
        )
    );

    assert_eq!(
        inject_sidecar(NO_IMPORTS, DEFAULT_SIDECAR).expect("inject"),
        concat!(
            "{ pkgs, ... }:\n",
            "{\n",
            "  imports = [ ./modules/nhs-packages.nix ];\n",
            "  home.stateVersion = \"24.05\";\n",
            "}\n",
        )
    );

    assert_eq!(
        inject_sidecar(EMPTY_LIST, DEFAULT_SIDECAR).expect("inject"),
        concat!(
            "{ pkgs, ... }:\n",
            "{\n",
            "  imports = [ ./modules/nhs-packages.nix ];\n",
            "}\n",
        )
    );

    assert_eq!(
        inject_sidecar(SINGLE_LINE, DEFAULT_SIDECAR).expect("inject"),
        "{ pkgs, ... }: { imports = [ ./existing.nix ./modules/nhs-packages.nix ]; }\n"
    );

    assert_eq!(
        inject_sidecar(LET_IN, DEFAULT_SIDECAR).expect("inject"),
        concat!(
            "{ pkgs, ... }:\n",
            "let\n",
            "  helper = 1;\n",
            "in\n",
            "{\n",
            "  imports = [\n",
            "    ./a.nix\n",
            "    ./modules/nhs-packages.nix\n",
            "  ];\n",
            "}\n",
        )
    );
}

// ---------------------------------------------------------------------------
// `imports` can also arrive through `inherit`
//
// `inherit imports;` is not an `AttrpathValue`, so a scan that only looks at
// bindings concludes "no imports here" and then adds a second one — which makes
// the module fail to evaluate. Any form this application cannot extend is
// refused, including an inherit inside a nested attribute set.
// ---------------------------------------------------------------------------

const INHERITED_TOP: &str = concat!(
    "{ pkgs, imports, ... }:\n",
    "{\n",
    "  inherit imports;\n",
    "  home.stateVersion = \"24.05\";\n",
    "}\n",
);

const INHERITED_NESTED: &str = concat!(
    "{ pkgs, ... }:\n",
    "{\n",
    "  config = {\n",
    "    inherit (pkgs) imports;\n",
    "  };\n",
    "}\n",
);

#[test]
fn refuses_imports_brought_in_by_inherit() {
    match inject_sidecar(INHERITED_TOP, DEFAULT_SIDECAR) {
        Err(ImportError::InheritedImports { line }) => assert_eq!(line, 3),
        other => panic!("expected InheritedImports, got {other:?}"),
    }

    // The same shape written with the opening brace on the first line: the
    // reported line must follow the file, not a fixed position.
    let compact = concat!(
        "{ pkgs, imports, ... }: {\n",
        "  inherit imports;\n",
        "  home.stateVersion = \"24.05\";\n",
        "}\n",
    );
    match inject_sidecar(compact, DEFAULT_SIDECAR) {
        Err(ImportError::InheritedImports { line }) => assert_eq!(line, 2),
        other => panic!("expected InheritedImports, got {other:?}"),
    }
}

#[test]
fn refuses_an_inherited_imports_in_a_nested_attribute_set() {
    match inject_sidecar(INHERITED_NESTED, DEFAULT_SIDECAR) {
        Err(ImportError::NestedImports { line }) => assert_eq!(line, 4),
        other => panic!("expected NestedImports, got {other:?}"),
    }
}

/// The `inherit (source) …` form is refused as well, whether the source is a
/// simple name or a selection.
#[test]
fn refuses_an_inherit_with_a_source() {
    for source in ["pkgs", "pkgs.lib", "builtins"] {
        let fixture = format!("{{ pkgs, ... }}:\n{{\n  inherit ({source}) imports;\n}}\n");
        match inject_sidecar(&fixture, DEFAULT_SIDECAR) {
            Err(ImportError::InheritedImports { line }) => assert_eq!(line, 3, "{source}"),
            other => panic!("`inherit ({source}) imports;` → {other:?}"),
        }
    }
}

/// A name list is checked as a whole: `imports` may not be the first or the only
/// entry.
#[test]
fn refuses_imports_in_a_multi_name_inherit() {
    let fixture = "{ pkgs, ... }:\n{\n  inherit (pkgs) hello imports;\n}\n";
    match inject_sidecar(fixture, DEFAULT_SIDECAR) {
        Err(ImportError::InheritedImports { line }) => assert_eq!(line, 3),
        other => panic!("expected InheritedImports, got {other:?}"),
    }
}

/// Only the inherited *names* count: an identifier called `imports` inside the
/// source expression is somebody else's, so adding the import is still allowed.
#[test]
fn an_identifier_named_imports_inside_the_source_is_not_a_declaration() {
    let fixture = "{ pkgs, ... }:\n{\n  inherit (lib.imports) hello;\n}\n";
    let new = inject_sidecar(fixture, DEFAULT_SIDECAR).expect("this does not define `imports`");
    validate::parse(&new).expect("must parse");
    assert_eq!(new.matches(DEFAULT_SIDECAR).count(), 1);
    assert!(
        new.contains("inherit (lib.imports) hello;"),
        "the inherit must survive:\n{new}"
    );
}
