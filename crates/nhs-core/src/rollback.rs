//! What this application can say about rolling back, without doing it.
//!
//! The installed Home Manager (the store build on this host) supports exactly one
//! rollback: `home-manager switch --rollback` moves to the generation before the
//! current one. It has no `--switch-generation`. The plan below reflects that
//! capability instead of inventing a command the tool would reject.
//!
//! Nothing here runs a process. A front end may display the plan; M7.2 verifies
//! execution only inside a disposable sandbox.

use crate::discovery::GenerationLink;
use crate::proc::Program;

/// A rollback that the toolchain can actually perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackPlan {
    /// `home-manager switch --rollback`, which moves one generation back.
    Previous { program: Program, args: Vec<String> },
    /// A request this toolchain cannot satisfy, with the reason shown to the user.
    Unsupported { reason: &'static str },
}

impl RollbackPlan {
    /// The command as `program arg…`, when there is one.
    #[must_use]
    pub fn command(&self) -> Option<String> {
        match self {
            Self::Previous { program, args } => {
                Some(format!("{} {}", program.name(), args.join(" ")))
            }
            Self::Unsupported { .. } => None,
        }
    }
}

/// Decide what rolling back to `target` (or to the previous generation when
/// `None`) would mean. Read-only: no path is touched and no process starts.
#[must_use]
pub fn rollback_plan(
    generations: &[GenerationLink],
    current: Option<u32>,
    target: Option<u32>,
) -> RollbackPlan {
    let Some(current) = current else {
        return RollbackPlan::Unsupported {
            reason: "there is no current generation to roll back from",
        };
    };
    let previous = generations
        .iter()
        .filter_map(|generation| generation.id)
        .filter(|id| *id < current)
        .max();

    match target {
        None => match previous {
            Some(_) => previous_plan(),
            None => RollbackPlan::Unsupported {
                reason: "there is no earlier generation to roll back to",
            },
        },
        Some(target) => {
            if target == current {
                return RollbackPlan::Unsupported {
                    reason: "that is the current generation",
                };
            }
            if generations
                .iter()
                .all(|generation| generation.id != Some(target))
            {
                return RollbackPlan::Unsupported {
                    reason: "no generation with that id exists",
                };
            }
            if Some(target) == previous {
                previous_plan()
            } else {
                RollbackPlan::Unsupported {
                    reason: "this home-manager version can only roll back one generation",
                }
            }
        }
    }
}

fn previous_plan() -> RollbackPlan {
    RollbackPlan::Previous {
        program: Program::HomeManager,
        args: vec!["switch".to_string(), "--rollback".to_string()],
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::panic,
        clippy::match_wildcard_for_single_variants,
        clippy::unwrap_used
    )]

    use super::{rollback_plan, RollbackPlan};
    use crate::discovery::GenerationLink;
    use std::path::PathBuf;

    fn links(ids: &[u32]) -> Vec<GenerationLink> {
        ids.iter()
            .map(|id| GenerationLink {
                id: Some(*id),
                link: PathBuf::from(format!("/p/home-manager-{id}-link")),
                target: Some(PathBuf::from(format!("/nix/store/gen-{id}"))),
            })
            .collect()
    }

    #[test]
    fn no_current_generation_is_refused() {
        assert!(matches!(
            rollback_plan(&links(&[1, 2]), None, None),
            RollbackPlan::Unsupported { .. }
        ));
    }

    #[test]
    fn the_previous_generation_is_the_one_the_tool_supports() {
        match rollback_plan(&links(&[1, 2, 3]), Some(3), None) {
            plan @ RollbackPlan::Previous { .. } => {
                assert_eq!(
                    plan.command().as_deref(),
                    Some("home-manager switch --rollback")
                );
            }
            other => panic!("expected Previous, got {other:?}"),
        }
    }

    #[test]
    fn naming_the_previous_generation_matches_the_implicit_rollback() {
        match rollback_plan(&links(&[1, 2, 3]), Some(3), Some(2)) {
            RollbackPlan::Previous { args, .. } => assert_eq!(args, ["switch", "--rollback"]),
            other => panic!("expected Previous, got {other:?}"),
        }
    }

    #[test]
    fn rolling_back_to_the_current_generation_is_refused() {
        match rollback_plan(&links(&[1, 2, 3]), Some(3), Some(3)) {
            RollbackPlan::Unsupported { reason } => assert!(reason.contains("current")),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn an_older_generation_is_refused_by_this_home_manager() {
        match rollback_plan(&links(&[1, 2, 3]), Some(3), Some(1)) {
            RollbackPlan::Unsupported { reason } => {
                assert!(reason.contains("one generation"), "{reason}");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_id_is_refused() {
        match rollback_plan(&links(&[1, 2, 3]), Some(3), Some(99)) {
            RollbackPlan::Unsupported { reason } => assert!(reason.contains("no generation")),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn a_single_generation_cannot_roll_back() {
        match rollback_plan(&links(&[1]), Some(1), None) {
            RollbackPlan::Unsupported { reason } => {
                assert!(reason.contains("no earlier generation"), "{reason}");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
