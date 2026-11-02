//! Which nixpkgs the catalogue is built from.
//!
//! Three sources, in priority order (architecture §5):
//!
//! 1. an explicit `--flake-ref`, used verbatim;
//! 2. the configuration's `flake.lock`, **following `follows` indirection**;
//! 3. the unqualified `nixpkgs` flake ref, resolved by Nix at run time.
//!
//! The third is a real fallback, not a pin: two runs a week apart can see
//! different packages. [`Resolution::pinned`] says which it is, and the snapshot
//! header records it, so the UI can be honest about reproducibility.
//!
//! NixBox bails out on the second case (`crates/nixbox-nix/src/search.rs`, the
//! `locked` lookup in `catalog_source_from_lock`): a lock entry that records an
//! input as `follows` has no `locked` of its own, so it is treated as broken.
//! Here the indirection is followed to the node it points at.
//!
//! This module is pure: the lock file's *text* is passed in, and nothing is read
//! or executed.

use crate::edit::hash::Sha256;

/// Where a resolution came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Passed by the user.
    Explicit,
    /// Read from the configuration's `flake.lock`.
    LockFile,
    /// The unqualified `nixpkgs` ref; not pinned.
    UnpinnedDefault,
}

impl SourceKind {
    /// Stable label for JSON output.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::LockFile => "flake-lock",
            Self::UnpinnedDefault => "unpinned-default",
        }
    }
}

/// A resolved catalogue source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub kind: SourceKind,
    /// The flake ref to hand to `nix search`.
    pub reference: String,
    /// The commit, when the reference names one.
    pub revision: Option<String>,
    /// False when the reference is a branch or the unqualified default: the same
    /// command can produce different results later.
    pub pinned: bool,
}

impl Resolution {
    /// Cache key for the full-tier snapshot.
    ///
    /// Binds the reference, the revision (it is part of the reference) and the
    /// snapshot format, so a changed lock, a changed flag or a bumped format all
    /// invalidate a cached catalogue (architecture §5.2).
    #[must_use]
    pub fn cache_key(&self, format_version: u32) -> String {
        let mut material = Vec::new();
        material.extend_from_slice(self.reference.as_bytes());
        material.push(0);
        material.extend_from_slice(format_version.to_string().as_bytes());
        Sha256::of_bytes(&material).to_hex()
    }
}

/// Why a source could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionError {
    /// The explicit ref is empty.
    EmptyExplicitRef,
    /// The lock file is not JSON, or lacks `nodes`/`root`.
    InvalidLock { detail: String },
    /// The root has no `nixpkgs` input, directly or through another input.
    NoNixpkgsInput,
    /// Following `follows` came back to a node it had already visited.
    FollowsCycle { path: String },
    /// A `follows` target does not exist in the lock.
    FollowsTargetMissing { target: String, node: String },
    /// The node was reached but its `locked` record is incomplete.
    LockedIncomplete { node: String, field: &'static str },
    /// The node is locked to something this application cannot name as a flake ref.
    UnsupportedLockedType { node: String, kind: String },
}

impl std::fmt::Display for RevisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyExplicitRef => write!(f, "`--flake-ref` was given but is empty"),
            Self::InvalidLock { detail } => write!(f, "flake.lock is not usable: {detail}"),
            Self::NoNixpkgsInput => write!(
                f,
                "flake.lock has no `nixpkgs` input, directly or through another input"
            ),
            Self::FollowsCycle { path } => {
                write!(f, "`follows` loops back on itself at `{path}`")
            }
            Self::FollowsTargetMissing { target, node } => {
                write!(f, "`{node}` follows `{target}`, which is not in flake.lock")
            }
            Self::LockedIncomplete { node, field } => {
                write!(f, "flake.lock node `{node}` has no `locked.{field}`")
            }
            Self::UnsupportedLockedType { node, kind } => write!(
                f,
                "flake.lock node `{node}` is locked to a `{kind}` input, which cannot be used \
                 as a nixpkgs flake reference here"
            ),
        }
    }
}

impl std::error::Error for RevisionError {}

/// Resolve the catalogue source.
///
/// `explicit` wins when present; otherwise `lock` (the text of `flake.lock`) is
/// consulted; otherwise the unqualified default is used.
pub fn resolve(explicit: Option<&str>, lock: Option<&str>) -> Result<Resolution, RevisionError> {
    if let Some(reference) = explicit {
        let reference = reference.trim();
        if reference.is_empty() {
            return Err(RevisionError::EmptyExplicitRef);
        }
        let revision = pinned_revision(reference);
        return Ok(Resolution {
            kind: SourceKind::Explicit,
            reference: reference.to_string(),
            pinned: revision.is_some(),
            revision,
        });
    }
    if let Some(text) = lock {
        return from_lock(text);
    }
    Ok(Resolution {
        kind: SourceKind::UnpinnedDefault,
        reference: "nixpkgs".to_string(),
        revision: None,
        pinned: false,
    })
}

/// The commit named by a ref, if it names one.
fn pinned_revision(reference: &str) -> Option<String> {
    let (path, query) = reference
        .split_once('?')
        .map_or((reference, None), |(path, query)| (path, Some(query)));

    if let Some(query) = query {
        // Split on `&` and compare the key exactly: `?notrev=…` is a different
        // parameter and must not be read as a pin.
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("rev=") {
                let candidate: String = value.chars().take_while(char::is_ascii_hexdigit).collect();
                if candidate.len() >= 7 {
                    return Some(candidate);
                }
            }
        }
    }

    // `github:owner/repo/<40 hex>` — the last path segment is the commit.
    let last = path.rsplit('/').next().unwrap_or("");
    if last.len() >= 40 && last.chars().all(|character| character.is_ascii_hexdigit()) {
        return Some(last.to_string());
    }
    None
}

/// Resolve from the text of a `flake.lock`.
fn from_lock(text: &str) -> Result<Resolution, RevisionError> {
    let lock: serde_json::Value =
        serde_json::from_str(text).map_err(|error| RevisionError::InvalidLock {
            detail: error.to_string(),
        })?;
    let nodes = lock
        .get("nodes")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| RevisionError::InvalidLock {
            detail: "no `nodes` object".to_string(),
        })?;
    let root = lock
        .get("root")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| RevisionError::InvalidLock {
            detail: "no `root` node id".to_string(),
        })?;

    let node_id = find_nixpkgs_node(nodes, root)?;
    let node = nodes
        .get(&node_id)
        .ok_or_else(|| RevisionError::FollowsTargetMissing {
            target: node_id.clone(),
            node: node_id.clone(),
        })?;
    let locked = node
        .get("locked")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| RevisionError::LockedIncomplete {
            node: node_id.clone(),
            field: "type",
        })?;

    let kind = locked
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| RevisionError::LockedIncomplete {
            node: node_id.clone(),
            field: "type",
        })?;
    let host = match kind {
        "github" => "github",
        "gitlab" => "gitlab",
        other => {
            return Err(RevisionError::UnsupportedLockedType {
                node: node_id,
                kind: other.to_string(),
            })
        }
    };
    let revision = locked
        .get("rev")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| RevisionError::LockedIncomplete {
            node: node_id.clone(),
            field: "rev",
        })?;
    let owner = locked
        .get("owner")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| RevisionError::LockedIncomplete {
            node: node_id.clone(),
            field: "owner",
        })?;
    let repo = locked
        .get("repo")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| RevisionError::LockedIncomplete {
            node: node_id.clone(),
            field: "repo",
        })?;
    Ok(Resolution {
        kind: SourceKind::LockFile,
        reference: format!("{host}:{owner}/{repo}/{revision}"),
        revision: Some(revision.to_string()),
        pinned: true,
    })
}

/// Find the node that holds nixpkgs.
///
/// The root's own `nixpkgs` input is authoritative — it is what the configuration
/// actually evaluates with. When that input is an indirection (an array or a
/// `follows` object) it is followed, relative to the root as Nix records it.
fn find_nixpkgs_node(
    nodes: &serde_json::Map<String, serde_json::Value>,
    root: &str,
) -> Result<String, RevisionError> {
    let root_node = nodes.get(root).ok_or_else(|| RevisionError::InvalidLock {
        detail: format!("root node `{root}` is missing"),
    })?;
    let value = root_node
        .get("inputs")
        .and_then(serde_json::Value::as_object)
        .and_then(|inputs| inputs.get("nixpkgs"))
        .ok_or(RevisionError::NoNixpkgsInput)?;
    follow(nodes, root, value)
}

/// Follow one input value to the node it designates.
///
/// Handles the three shapes a lock file uses for an input: a node id (string), a
/// path array (`["nixpkgs"]`), and an explicit `{"follows": "inputs.nixpkgs"}`
/// object. Paths are relative to the root, as Nix records them.
fn follow(
    nodes: &serde_json::Map<String, serde_json::Value>,
    root: &str,
    value: &serde_json::Value,
) -> Result<String, RevisionError> {
    let mut value = value.clone();
    let mut steps: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..32 {
        let fingerprint = value.to_string();
        if seen.contains(&fingerprint) {
            return Err(RevisionError::FollowsCycle {
                path: steps.join("."),
            });
        }
        seen.push(fingerprint);
        match value {
            serde_json::Value::String(node) => {
                if !nodes.contains_key(&node) {
                    return Err(RevisionError::FollowsTargetMissing {
                        target: node,
                        node: root.to_string(),
                    });
                }
                return Ok(node);
            }
            serde_json::Value::Array(items) => {
                let path = path_from(&items)?;
                steps.extend(path.iter().cloned());
                if steps.len() > 32 {
                    return Err(RevisionError::FollowsCycle {
                        path: steps.join("."),
                    });
                }
                value = lookup_path(nodes, root, &path)?;
            }
            serde_json::Value::Object(object) => {
                let target = object
                    .get("follows")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| RevisionError::InvalidLock {
                        detail: "an input is an object without `follows`".to_string(),
                    })?;
                let path: Vec<String> = target
                    .split(['.', '/'])
                    .filter(|part| !part.is_empty() && *part != "inputs")
                    .map(str::to_string)
                    .collect();
                steps.extend(path.iter().cloned());
                if steps.len() > 32 {
                    return Err(RevisionError::FollowsCycle {
                        path: steps.join("."),
                    });
                }
                value = lookup_path(nodes, root, &path)?;
            }
            _ => {
                return Err(RevisionError::InvalidLock {
                    detail: "an input is neither a node id, a path nor a `follows` object"
                        .to_string(),
                })
            }
        }
    }
    Err(RevisionError::FollowsCycle {
        path: steps.join("."),
    })
}

/// Read a `follows` path array.
///
/// Every element must be a string. Dropping a non-string would silently change
/// which node the path designates, and a lock file that is damaged must fail
/// closed rather than resolve to something plausible.
fn path_from(items: &[serde_json::Value]) -> Result<Vec<String>, RevisionError> {
    let mut path = Vec::with_capacity(items.len());
    for item in items {
        let Some(part) = item.as_str() else {
            return Err(RevisionError::InvalidLock {
                detail: "a `follows` path contains a value that is not a string".to_string(),
            });
        };
        path.push(part.to_string());
    }
    Ok(path)
}

/// Walk `path` from `root` through the `inputs` maps.
fn lookup_path(
    nodes: &serde_json::Map<String, serde_json::Value>,
    root: &str,
    path: &[String],
) -> Result<serde_json::Value, RevisionError> {
    if path.is_empty() {
        return Err(RevisionError::InvalidLock {
            detail: "an empty `follows` path".to_string(),
        });
    }
    let mut current = root.to_string();
    for (index, step) in path.iter().enumerate() {
        let next = nodes
            .get(&current)
            .and_then(|node| node.get("inputs"))
            .and_then(serde_json::Value::as_object)
            .and_then(|inputs| inputs.get(step))
            .cloned()
            .ok_or_else(|| RevisionError::FollowsTargetMissing {
                target: path.join("."),
                node: current.clone(),
            })?;
        let last = index + 1 == path.len();
        match next {
            serde_json::Value::String(node) => {
                if last {
                    return Ok(serde_json::Value::String(node));
                }
                if !nodes.contains_key(&node) {
                    return Err(RevisionError::FollowsTargetMissing {
                        target: path.join("."),
                        node,
                    });
                }
                current = node;
            }
            // A path or `follows` object in the middle of the walk: hand it back
            // to `follow`, which will resolve it from the root.
            other => return Ok(other),
        }
    }
    Ok(serde_json::Value::String(current))
}
