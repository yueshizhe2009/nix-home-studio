//! M5.1 acceptance tests: the curated catalogue loads offline, ranks, filters and
//! pages — with no network, no Nix, and no filesystem beyond the binary itself.

#![allow(
    clippy::pedantic,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use nhs_core::catalog::{
    platform_support, search, Catalog, CatalogEntry, Filters, HostPlatform, PlatformSupport,
    CATEGORIES, FORMAT_VERSION,
};

fn linux() -> HostPlatform {
    HostPlatform::from_os_arch("linux", "x86_64")
}

fn darwin() -> HostPlatform {
    HostPlatform::from_os_arch("macos", "aarch64")
}

fn attrs<'a>(page: &'a nhs_core::catalog::Page<'a>) -> Vec<&'a str> {
    page.hits
        .iter()
        .map(|hit| hit.entry.attr.as_str())
        .collect()
}

fn catalogue(entries: &str) -> Catalog {
    Catalog::from_toml(&format!("format_version = {FORMAT_VERSION}\n{entries}")).expect("catalogue")
}

/// The shipped catalogue parses, has the documented size, and puts an exact name
/// match first.
#[test]
fn curated_catalog_loads_offline_and_ranks_exact_names_first() {
    let catalog = Catalog::builtin().expect("the shipped catalogue must parse");
    assert!(
        catalog.len() >= 100,
        "the curated list was meant to be ~120 entries, found {}",
        catalog.len()
    );

    for (query, expected) in [("firefox", "firefox"), ("ripgrep", "ripgrep"), ("jq", "jq")] {
        let page = search(&catalog, query, &Filters::default(), &linux(), 1, 10);
        assert!(!page.hits.is_empty(), "`{query}` found nothing");
        let first = &page.hits[0];
        assert_eq!(
            first.entry.attr, expected,
            "`{query}` did not rank the exact name first"
        );
        assert_eq!(first.tier, 0, "an exact name match is tier 0");
    }

    // Prefix beats token, token beats substring: the tiers never increase within
    // a page.
    let page = search(&catalog, "git", &Filters::default(), &linux(), 1, 100);
    assert_eq!(page.hits[0].entry.attr, "git");
    for pair in page.hits.windows(2) {
        assert!(
            pair[0].tier <= pair[1].tier,
            "tiers must be non-decreasing: {} ({}) then {} ({})",
            pair[0].entry.attr,
            pair[0].tier,
            pair[1].entry.attr,
            pair[1].tier
        );
    }
}

/// Unfree entries are hidden until asked for, and a declared platform that is not
/// ours is excluded — while "unknown" stays visible.
#[test]
fn filters_exclude_unfree_and_wrong_platform() {
    let catalog = Catalog::builtin().expect("catalogue");

    let default = Filters::default();
    let page = search(&catalog, "vscode", &default, &linux(), 1, 10);
    assert!(
        attrs(&page).iter().all(|attr| *attr != "vscode"),
        "unfree must be excluded by default: {:?}",
        attrs(&page)
    );

    let permissive = Filters {
        include_unfree: true,
        ..Filters::default()
    };
    let page = search(&catalog, "vscode", &permissive, &linux(), 1, 10);
    assert!(
        attrs(&page).contains(&"vscode"),
        "`--include-unfree` must reveal it: {:?}",
        attrs(&page)
    );

    // A Linux-only desktop package disappears on macOS.
    let page = search(&catalog, "kitty", &default, &darwin(), 1, 10);
    assert!(
        attrs(&page).contains(&"kitty"),
        "kitty declares darwin support: {:?}",
        attrs(&page)
    );
    let page = search(&catalog, "virt-manager", &default, &darwin(), 1, 10);
    assert!(
        attrs(&page).is_empty(),
        "virt-manager is Linux-only: {:?}",
        attrs(&page)
    );

    // Synthetic edge cases the shipped data must not have to carry.
    let synthetic = catalogue(
        r#"
[[entries]]
attr = "unknown-platform"
name = "unknown-platform"
description = "no platform list"
category = "system"

[[entries]]
attr = "arm-only"
name = "arm-only"
description = "declares only another system"
category = "system"
platforms = ["aarch64-darwin"]

[[entries]]
attr = "no-license"
name = "no-license"
description = "licence unknown"
category = "system"
platforms = ["linux"]

[[entries]]
attr = "broken-thing"
name = "broken-thing"
description = "marked broken"
category = "system"
platforms = ["linux"]
broken = true
"#,
    );

    // Unknown platform ⇒ shown, and reported as unknown.
    let page = search(&synthetic, "unknown-platform", &default, &linux(), 1, 10);
    assert_eq!(attrs(&page), vec!["unknown-platform"]);
    assert_eq!(page.hits[0].platform, PlatformSupport::Unknown);

    // A declared foreign platform ⇒ excluded on this host, kept on that one.
    let page = search(&synthetic, "arm-only", &default, &linux(), 1, 10);
    assert!(page.hits.is_empty(), "{:?}", attrs(&page));
    let page = search(&synthetic, "arm-only", &default, &darwin(), 1, 10);
    assert_eq!(attrs(&page), vec!["arm-only"]);

    // Broken ⇒ excluded unless asked for.
    let page = search(&synthetic, "broken-thing", &default, &linux(), 1, 10);
    assert!(page.hits.is_empty());
    let page = search(
        &synthetic,
        "broken-thing",
        &Filters {
            include_broken: true,
            ..Filters::default()
        },
        &linux(),
        1,
        10,
    );
    assert_eq!(attrs(&page), vec!["broken-thing"]);

    // Unknown licence is shown by default, but cannot satisfy an allow-list.
    let page = search(&synthetic, "no-license", &default, &linux(), 1, 10);
    assert_eq!(attrs(&page), vec!["no-license"]);
    let page = search(
        &synthetic,
        "no-license",
        &Filters {
            license_allow: vec!["MIT".to_string()],
            ..Filters::default()
        },
        &linux(),
        1,
        10,
    );
    assert!(
        page.hits.is_empty(),
        "an unknown licence cannot be verified"
    );

    // A deny-list entry is removed by name.
    let page = search(
        &catalog,
        "git",
        &Filters {
            license_deny: vec!["GPL-2.0".to_string()],
            ..Filters::default()
        },
        &linux(),
        1,
        10,
    );
    assert!(
        attrs(&page).iter().all(|attr| *attr != "git"),
        "git is GPL-2.0 and was denied: {:?}",
        attrs(&page)
    );
}

/// Platform matching follows architecture §5.1, including the family aliases.
#[test]
fn platform_matching_understands_families_and_systems() {
    let entry = |platforms: &[&str]| CatalogEntry {
        attr: "x".to_string(),
        name: "x".to_string(),
        description: String::new(),
        category: "system".to_string(),
        platforms: platforms.iter().map(|tag| (*tag).to_string()).collect(),
        license: None,
        unfree: false,
        broken: false,
        nixos_only: false,
    };

    assert_eq!(
        platform_support(&entry(&[]), &linux()),
        PlatformSupport::Unknown
    );
    assert_eq!(
        platform_support(&entry(&["x86_64-linux"]), &linux()),
        PlatformSupport::Supported
    );
    assert_eq!(
        platform_support(&entry(&["linux"]), &linux()),
        PlatformSupport::Supported
    );
    assert_eq!(
        platform_support(&entry(&["unix"]), &darwin()),
        PlatformSupport::Supported
    );
    assert_eq!(
        platform_support(&entry(&["aarch64-darwin"]), &linux()),
        PlatformSupport::Unsupported
    );
    assert_eq!(
        platform_support(&entry(&["nonsense-tag"]), &linux()),
        PlatformSupport::Unsupported,
        "an unrecognised tag is a non-match, not an error"
    );

    // `--platform` replaces the host for the run.
    let filters = Filters {
        platform: Some("aarch64-darwin".to_string()),
        ..Filters::default()
    };
    let target = filters.resolve_target(&linux());
    assert!(filters.accepts(&entry(&["aarch64-darwin"]), &target));
    assert!(!filters.accepts(&entry(&["x86_64-linux"]), &target));
}

/// Consecutive pages are stable and disjoint, and the union is the whole result.
#[test]
fn pagination_returns_stable_disjoint_pages() {
    let catalog = Catalog::builtin().expect("catalogue");
    let filters = Filters::default();
    let first_run = search(&catalog, "", &filters, &linux(), 1, 25);
    assert!(first_run.pages >= 2, "expected several pages");

    let mut seen: Vec<String> = Vec::new();
    for page_number in 1..=first_run.pages {
        let page = search(&catalog, "", &filters, &linux(), page_number, 25);
        assert_eq!(page.page, page_number);
        assert_eq!(page.total, first_run.total);
        for hit in &page.hits {
            let attr = hit.entry.attr.clone();
            assert!(
                !seen.contains(&attr),
                "{attr} appeared on more than one page"
            );
            seen.push(attr);
        }
        // The same query twice gives the same order.
        let again = search(&catalog, "", &filters, &linux(), page_number, 25);
        assert_eq!(
            attrs(&page),
            attrs(&again),
            "page {page_number} is not stable between runs"
        );
    }
    assert_eq!(
        seen.len(),
        first_run.total,
        "the pages must cover every hit"
    );

    // Out-of-range pages are clamped instead of panicking.
    let beyond = search(&catalog, "", &filters, &linux(), 9_999, 25);
    assert_eq!(beyond.page, first_run.pages);
    let zero = search(&catalog, "", &filters, &linux(), 0, 25);
    assert_eq!(zero.page, 1);
    let huge = search(&catalog, "", &filters, &linux(), 1, 10_000);
    assert_eq!(huge.per_page, nhs_core::catalog::MAX_PER_PAGE);
}

/// The shipped data is structurally sound on its own terms: unique attrs, known
/// categories, sane descriptions, every category represented.
#[test]
fn the_shipped_data_is_well_formed() {
    let catalog = Catalog::builtin().expect("catalogue");
    let mut categories: Vec<&str> = Vec::new();
    for entry in catalog.entries() {
        assert!(!entry.name.trim().is_empty(), "{}", entry.attr);
        assert!(!entry.description.trim().is_empty(), "{}", entry.attr);
        assert!(
            entry.description.len() <= 90,
            "{}: description is a sentence, not a paragraph",
            entry.attr
        );
        assert!(
            CATEGORIES.contains(&entry.category.as_str()),
            "{}: unknown category {}",
            entry.attr,
            entry.category
        );
        assert!(
            !entry.platforms.iter().any(|tag| tag != &tag.to_lowercase()),
            "{}: platform tags are lowercase",
            entry.attr
        );
        categories.push(&entry.category);
    }
    for category in CATEGORIES {
        assert!(
            categories.contains(&category),
            "category `{category}` has no entries, so the filter would look broken"
        );
    }
    // Unfree packages are declared, never smuggled in without the flag.
    assert!(
        catalog.entries().iter().any(|entry| entry.unfree),
        "the data should exercise the unfree flag"
    );
    assert!(
        catalog.entries().iter().all(|entry| !entry.nixos_only),
        "a Home Manager catalogue should not list NixOS-only packages yet"
    );
}

/// A malformed catalogue is refused with a reason, not half-loaded.
#[test]
fn a_malformed_catalogue_is_refused() {
    let cases: [(&str, &str); 7] = [
        ("format_version = 99\nentries = []\n", "version"),
        ("format_version = 1\nentries = []\n", "no entries"),
        (
            "format_version = 1\n[[entries]]\nattr = \"a b\"\nname = \"x\"\ndescription = \"d\"\ncategory = \"system\"\n",
            "not a usable package name",
        ),
        (
            "format_version = 1\n[[entries]]\nattr = \"ok\"\nname = \"\"\ndescription = \"d\"\ncategory = \"system\"\n",
            "is empty",
        ),
        (
            "format_version = 1\n[[entries]]\nattr = \"ok\"\nname = \"x\"\ndescription = \"d\"\ncategory = \"nonsense\"\n",
            "not one of",
        ),
        (
            "format_version = 1\n[[entries]]\nattr = \"ok\"\nname = \"x\"\ndescription = \"d\"\ncategory = \"system\"\nplatforms = [\"has space\"]\n",
            "platform tag",
        ),
        (
            "format_version = 1\n[[entries]]\nattr = \"dup\"\nname = \"x\"\ndescription = \"d\"\ncategory = \"system\"\n[[entries]]\nattr = \"dup\"\nname = \"y\"\ndescription = \"d\"\ncategory = \"system\"\n",
            "appears twice",
        ),
    ];
    for (text, needle) in cases {
        let error = Catalog::from_toml(text).expect_err("must be refused");
        let message = error.to_string();
        assert!(
            message.contains(needle),
            "expected `{needle}` in `{message}`"
        );
    }
}

/// Searching for something absent is empty, not an error, and reports zero pages.
#[test]
fn an_empty_result_is_not_an_error() {
    let catalog = Catalog::builtin().expect("catalogue");
    let page = search(
        &catalog,
        "zzzznotapackage",
        &Filters::default(),
        &linux(),
        1,
        10,
    );
    assert!(page.hits.is_empty());
    assert_eq!(page.total, 0);
    assert_eq!(page.pages, 0);
    assert_eq!(page.page, 1, "an empty result still reports page 1");
}
