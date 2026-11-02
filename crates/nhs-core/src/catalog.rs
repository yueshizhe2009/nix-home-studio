//! The curated package catalogue: offline, opinionated, and honest about what it
//! does not know.
//!
//! Two design rules shape this module (architecture §5.1):
//!
//! * **unknown is shown, not hidden** — an entry with no platform list, or no
//!   licence, is included and labelled; only an entry that *declares* another
//!   platform is excluded;
//! * **the order is total** — ranking is `(tier, position, name length, name,
//!   attr)`, so paging twice cannot move a hit between pages.
//!
//! Nothing here reads a file or runs a command: the catalogue is compiled in and
//! searching is arithmetic over a `Vec`.

use serde::Deserialize;

use crate::donated::nix_search::rank::{ranking_term, relevance_fields, Relevance};

/// The catalogue format this build understands.
pub const FORMAT_VERSION: u32 = 1;

/// Categories an entry may claim. A typo here would silently orphan a package
/// from the category filter, so it is checked at load time.
pub const CATEGORIES: [&str; 12] = [
    "development",
    "editor",
    "games",
    "internet",
    "media",
    "nix",
    "office",
    "security",
    "shell",
    "system",
    "terminal",
    "virtualisation",
];

/// One package a beginner might want.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CatalogEntry {
    /// Attribute path under `pkgs`, e.g. `ripgrep` or `python3Packages.requests`.
    pub attr: String,
    /// Display name.
    pub name: String,
    pub description: String,
    pub category: String,
    /// Platforms this is known to build on. Empty means **unknown**, which is
    /// shown, never hidden.
    #[serde(default)]
    pub platforms: Vec<String>,
    /// `None` means unknown, which is displayed as `unknown` and is not a denial.
    #[serde(default)]
    pub license: Option<String>,
    /// Nix refuses this without an explicit opt-in.
    #[serde(default)]
    pub unfree: bool,
    /// Marked broken in nixpkgs at the revision this was written against.
    #[serde(default)]
    pub broken: bool,
    /// Only meaningful in a NixOS module.
    #[serde(default)]
    pub nixos_only: bool,
}

/// The on-disk shape, kept separate so `format_version` is not repeated per entry.
#[derive(Debug, Deserialize)]
struct CatalogFile {
    format_version: u32,
    entries: Vec<CatalogEntry>,
}

/// Why the catalogue could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    Parse { detail: String },
    UnsupportedFormat { version: u32 },
    Empty,
    MissingField { attr: String, field: &'static str },
    InvalidAttr { attr: String, reason: String },
    DuplicateAttr { attr: String },
    UnknownCategory { attr: String, category: String },
    InvalidPlatform { attr: String, tag: String },
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse { detail } => write!(f, "the catalogue is not valid TOML: {detail}"),
            Self::UnsupportedFormat { version } => write!(
                f,
                "catalogue format version {version} is not understood by this build \
                 (expected {FORMAT_VERSION})"
            ),
            Self::Empty => write!(f, "the catalogue has no entries"),
            Self::MissingField { attr, field } => {
                write!(f, "`{attr}`: `{field}` is empty")
            }
            Self::InvalidAttr { attr, reason } => {
                write!(f, "`{attr}` is not a usable package name: {reason}")
            }
            Self::DuplicateAttr { attr } => write!(f, "`{attr}` appears twice"),
            Self::UnknownCategory { attr, category } => write!(
                f,
                "`{attr}`: category `{category}` is not one of {}",
                CATEGORIES.join(", ")
            ),
            Self::InvalidPlatform { attr, tag } => {
                write!(
                    f,
                    "`{attr}`: platform tag `{tag}` is empty or has whitespace"
                )
            }
        }
    }
}

impl std::error::Error for CatalogError {}

/// A loaded, validated catalogue.
#[derive(Debug, Clone)]
pub struct Catalog {
    entries: Vec<CatalogEntry>,
}

impl Catalog {
    /// The catalogue that ships with the application.
    pub fn builtin() -> Result<Self, CatalogError> {
        Self::from_toml(include_str!("../data/catalog.toml"))
    }

    /// Parse and validate a catalogue.
    pub fn from_toml(text: &str) -> Result<Self, CatalogError> {
        let file: CatalogFile = toml::from_str(text).map_err(|error| CatalogError::Parse {
            detail: error.to_string(),
        })?;
        if file.format_version != FORMAT_VERSION {
            return Err(CatalogError::UnsupportedFormat {
                version: file.format_version,
            });
        }
        if file.entries.is_empty() {
            return Err(CatalogError::Empty);
        }

        let mut seen: Vec<&str> = Vec::with_capacity(file.entries.len());
        for entry in &file.entries {
            validate_entry(entry)?;
            if seen.contains(&entry.attr.as_str()) {
                return Err(CatalogError::DuplicateAttr {
                    attr: entry.attr.clone(),
                });
            }
            seen.push(&entry.attr);
        }
        Ok(Self {
            entries: file.entries,
        })
    }

    #[must_use]
    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn validate_entry(entry: &CatalogEntry) -> Result<(), CatalogError> {
    for (field, value) in [
        ("name", entry.name.as_str()),
        ("description", entry.description.as_str()),
        ("category", entry.category.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(CatalogError::MissingField {
                attr: entry.attr.clone(),
                field,
            });
        }
    }
    crate::module::check_attribute_path(&entry.attr).map_err(|reason| {
        CatalogError::InvalidAttr {
            attr: entry.attr.clone(),
            reason,
        }
    })?;
    if !CATEGORIES.contains(&entry.category.as_str()) {
        return Err(CatalogError::UnknownCategory {
            attr: entry.attr.clone(),
            category: entry.category.clone(),
        });
    }
    for tag in &entry.platforms {
        if tag.is_empty() || tag.chars().any(char::is_whitespace) {
            return Err(CatalogError::InvalidPlatform {
                attr: entry.attr.clone(),
                tag: tag.clone(),
            });
        }
    }
    if entry
        .license
        .as_deref()
        .is_some_and(|text| text.trim().is_empty())
    {
        return Err(CatalogError::MissingField {
            attr: entry.attr.clone(),
            field: "license",
        });
    }
    Ok(())
}

/// The machine a search is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPlatform {
    /// Concrete Nix system, e.g. `x86_64-linux`. Empty when only a family is known.
    pub system: String,
    /// `linux`, `darwin`, …
    pub family: String,
}

impl HostPlatform {
    /// Build a target from a concrete Nix system, a bare family, or `unix`.
    #[must_use]
    pub fn from_tag(tag: &str) -> Self {
        let family = tag.rsplit('-').next().unwrap_or(tag).to_string();
        let system = if tag.contains('-') {
            tag.to_string()
        } else {
            String::new()
        };
        Self { system, family }
    }

    /// Build a target from an OS and CPU name, as the CLI reads them from the
    /// build target's compile-time constants. This crate never inspects the
    /// process environment itself (gate G3).
    #[must_use]
    pub fn from_os_arch(os: &str, arch: &str) -> Self {
        let family = match os {
            "macos" => "darwin".to_string(),
            other => other.to_lowercase(),
        };
        let arch = arch.to_lowercase();
        Self {
            system: format!("{arch}-{family}"),
            family,
        }
    }
}

/// What the catalogue says about the target platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformSupport {
    Supported,
    /// The entry declares platforms, and none of them is this one.
    Unsupported,
    /// The entry does not say. Shown, because unknown is not "no".
    Unknown,
}

/// Decide whether an entry can run on `target` (architecture §5.1).
#[must_use]
pub fn platform_support(entry: &CatalogEntry, target: &HostPlatform) -> PlatformSupport {
    if entry.platforms.is_empty() {
        return PlatformSupport::Unknown;
    }
    for tag in &entry.platforms {
        if tag == &target.system && !target.system.is_empty() {
            return PlatformSupport::Supported;
        }
        if tag == &target.family {
            return PlatformSupport::Supported;
        }
        if tag == "unix" && matches!(target.family.as_str(), "linux" | "darwin") {
            return PlatformSupport::Supported;
        }
    }
    PlatformSupport::Unsupported
}

/// What to keep.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    /// Overrides the host platform for this search.
    pub platform: Option<String>,
    pub include_unfree: bool,
    pub include_broken: bool,
    pub include_nixos_only: bool,
    /// When non-empty, only these licences are accepted. An entry with no licence
    /// cannot satisfy it and is excluded — the user asked for a guarantee.
    pub license_allow: Vec<String>,
    /// Rejected licences. An entry with no licence is not rejected by name.
    pub license_deny: Vec<String>,
}

impl Filters {
    /// The platform this search is for: the `--platform` override when present,
    /// otherwise the host.
    #[must_use]
    pub fn resolve_target(&self, host: &HostPlatform) -> HostPlatform {
        self.platform
            .as_deref()
            .map_or_else(|| host.clone(), HostPlatform::from_tag)
    }

    /// Does this entry pass? `target` must already be resolved with
    /// [`Filters::resolve_target`], so the override is applied exactly once.
    #[must_use]
    pub fn accepts(&self, entry: &CatalogEntry, target: &HostPlatform) -> bool {
        if !self.include_unfree && entry.unfree {
            return false;
        }
        if !self.include_broken && entry.broken {
            return false;
        }
        if !self.include_nixos_only && entry.nixos_only {
            return false;
        }
        if platform_support(entry, target) == PlatformSupport::Unsupported {
            return false;
        }
        if !self.license_deny.is_empty() && license_denied(entry, &self.license_deny) {
            return false;
        }
        if !self.license_allow.is_empty() && !license_allowed(entry, &self.license_allow) {
            return false;
        }
        true
    }
}

/// A licence list is an exact, case-insensitive match against the whole string or
/// one of its `A OR B` alternatives. `GPL-3.0-or-later` is therefore not matched
/// by `GPL-3.0`; a filter is a promise, so it errs on the strict side.
fn license_alternatives(entry: &CatalogEntry) -> Vec<String> {
    let Some(license) = &entry.license else {
        return Vec::new();
    };
    license
        .split(" OR ")
        .map(|part| part.trim().to_lowercase())
        .collect()
}

fn license_allowed(entry: &CatalogEntry, allow: &[String]) -> bool {
    let alternatives = license_alternatives(entry);
    allow
        .iter()
        .any(|wanted| alternatives.contains(&wanted.trim().to_lowercase()))
}

fn license_denied(entry: &CatalogEntry, deny: &[String]) -> bool {
    let alternatives = license_alternatives(entry);
    deny.iter()
        .any(|unwanted| alternatives.contains(&unwanted.trim().to_lowercase()))
}

/// One result.
#[derive(Debug, Clone)]
pub struct Hit<'a> {
    pub entry: &'a CatalogEntry,
    /// Ranking tier from the donated scale (0 is the best match).
    pub tier: u8,
    pub platform: PlatformSupport,
}

/// One page of results.
#[derive(Debug, Clone)]
pub struct Page<'a> {
    pub hits: Vec<Hit<'a>>,
    /// 1-based page number actually used (clamped).
    pub page: usize,
    pub per_page: usize,
    /// Hits that passed the filters, across all pages.
    pub total: usize,
    pub pages: usize,
}

/// Largest page a caller may ask for.
pub const MAX_PER_PAGE: usize = 100;

/// Search the catalogue.
///
/// `page` is 1-based and clamped to a valid page; `per_page` is clamped to
/// `1..=MAX_PER_PAGE`. The ordering is total, so consecutive pages are stable and
/// disjoint.
#[must_use]
pub fn search<'a>(
    catalog: &'a Catalog,
    query: &str,
    filters: &Filters,
    host: &HostPlatform,
    page: usize,
    per_page: usize,
) -> Page<'a> {
    let term = ranking_term(query);
    let target = filters.resolve_target(host);
    let per_page = per_page.clamp(1, MAX_PER_PAGE);

    let mut ranked: Vec<(Relevance, &'a CatalogEntry, PlatformSupport)> = catalog
        .entries()
        .iter()
        .filter(|entry| filters.accepts(entry, &target))
        .map(|entry| {
            let relevance = relevance_fields(
                &entry.attr.to_lowercase(),
                &entry.name.to_lowercase(),
                &entry.description.to_lowercase(),
                &term,
            );
            (relevance, entry, platform_support(entry, &target))
        })
        // A real query must not return packages that do not mention it; an empty
        // query is "show me everything", which is how a UI browses the list.
        .filter(|(relevance, _, _)| term.is_empty() || relevance.tier < 6)
        .collect();

    ranked.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.attr.cmp(&right.1.attr))
    });

    let total = ranked.len();
    let pages = total.div_ceil(per_page);
    let page = page.max(1).min(pages.max(1));
    let start = (page - 1) * per_page;
    let hits: Vec<Hit<'a>> = ranked
        .iter()
        .skip(start)
        .take(per_page)
        .map(|(relevance, entry, platform)| Hit {
            entry,
            tier: relevance.tier,
            platform: *platform,
        })
        .collect();

    Page {
        hits,
        page,
        per_page,
        total,
        pages,
    }
}
