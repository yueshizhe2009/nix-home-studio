# Nix Home Studio — Agent Instructions

## Mission
Build a beginner-friendly Linux desktop application that unifies useful workflows from NixBox and nix-home-manager-gui: package discovery, Home Manager module generation, safe configuration editing, build validation, generation history, and rollback.

## Current assignment: audit only
Do not implement the application yet. Complete the upstream audit and design documents first:
1. `docs/upstream-audit.md`
2. `docs/architecture.md`
3. `docs/implementation-plan.md`

Study these repositories:
- `upstream/nixbox/` (https://github.com/SINGH-RAJVEER/nixbox)
- `upstream/nix-home-manager-gui/` (https://github.com/adhamsalama/nix-home-manager-gui)

## Safety constraints
- Treat upstream clones as read-only reference material. Do not edit their files or commits.
- Do not inspect, print, copy, or modify secrets, API keys, tokens, browser profiles, or unrelated personal files.
- Do not modify the user's existing Home Manager/NixOS configuration or any system configuration.
- Do not run `home-manager switch`, `nixos-rebuild switch`, package installation/removal, privileged commands, or destructive commands.
- Do not change global Git configuration, shell configuration, Orca/StudyBridge configuration, or the user's desktop.
- Do not claim a feature is supported without locating the relevant source or tests. Separate verified facts from assumptions.
- Do not merge code merely because both projects use Rust/Tauri. Check actual module boundaries, data models, dependencies, licenses, and testability.
- Preserve existing project files. Before editing a project file, inspect its current contents and Git status.

## Audit checklist
For each upstream, inspect README, license, manifests, directory structure, CI, tests, and the relevant implementation for:
- package search/install/remove and Nix invocation;
- Home Manager module generation and imports;
- parsing and editing existing Nix files;
- diff, backup, conflict detection, and recovery;
- generation listing and rollback;
- build/apply/error handling;
- packaging and Linux runtime dependencies.

## Deliverables
`docs/upstream-audit.md`:
- verified feature matrix with source paths and test references;
- license and dependency notes;
- reusable code vs design-only reference vs rewrite recommendation;
- unresolved questions and risk ranking.

`docs/architecture.md`:
- recommended stack and component boundaries;
- configuration ownership/data model;
- package catalog and module-generation pipeline;
- safe edit/validate/apply/rollback flow;
- failure recovery and portability boundaries.

`docs/implementation-plan.md`:
- small, ordered milestones;
- objective acceptance tests for each milestone;
- explicit non-goals and safety gates.

At the end, report what was inspected, which tests were run (if any), what remains unverified, and the recommended minimal viable version. Do not begin implementation until the user explicitly approves the audit/design.
