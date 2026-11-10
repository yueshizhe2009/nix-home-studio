# Nix Home Studio — Agent Instructions

## Mission
Build a beginner-friendly Linux desktop application that unifies useful workflows from NixBox and nix-home-manager-gui: package discovery, Home Manager module generation, safe configuration editing, build validation, generation history, and rollback.

## Current assignment: product phase — follow `docs/product-roadmap.md`

The upstream audit is finished (`docs/upstream-audit.md`) and the application is
implemented through `M7-gens` (the generations/rollback stage closed on
2026-11-08). Do not re-run the audit.

The authoritative route is **`docs/product-roadmap.md`** (effective 2026-11-08):
`M6.6` interaction/accessibility (closed out 2026-11-09: skip link and
1100×700 default window; environment verification gaps remain), `M7` books the
Home Manager daily experience, then
`M8` read-only Flake Foundation, `M9` Nix Doctor, `M10` Machine Profile,
`M11` Migration, `M12` Lock & Export.

Rules that override any local instinct:

- Do not start a milestone that the roadmap has not reached; each new domain
  (roadmap §5, B–E) needs its own design and boundary review first.
- `M8` is read-only understanding of flakes — no flake editor, no input updates,
  no migration work.
- Do not expand a closed milestone's verification scope. The closed generations
  stage is cited as `M7-gens`; the roadmap's `M7` is Home Manager experience.
- Keep the core/adapter/GUI layering and the single write path; no new
  abstraction framework in advance.
- Report what was inspected, which tests ran, and what remains unverified. Do
  not present fixture, container or Xvfb results as real-machine acceptance.

## Safety constraints
- Treat upstream clones as read-only reference material. Do not edit their files or commits.
- Do not inspect, print, copy, or modify secrets, API keys, tokens, browser profiles, or unrelated personal files.
- Do not modify the user's existing Home Manager/NixOS configuration or any system configuration.
- Do not run `home-manager switch`, `nixos-rebuild switch`, package installation/removal, privileged commands, or destructive commands.
- Do not change global Git configuration, shell configuration, Orca/StudyBridge configuration, or the user's desktop.
- Do not claim a feature is supported without locating the relevant source or tests. Separate verified facts from assumptions.
- Do not merge code merely because both projects use Rust/Tauri. Check actual module boundaries, data models, dependencies, licenses, and testability.
- Preserve existing project files. Before editing a project file, inspect its current contents and Git status.

## Repository map

| 路径 | 内容 |
|---|---|
| `crates/nhs-core/` | 业务规则：包目录、模块生成、diff、校验、应用、历史（不依赖 GUI） |
| `crates/nhs-adapter/` | Nix / Home Manager / 外部进程适配层 |
| `crates/nhs-cli/` | 命令行入口 |
| `crates/nhs-gui/` | Tauri 2 + WebKitGTK 主 GUI |
| `crates/nhs-gui-spike-*/` | 早期 iced / Tauri 原型，**不是产品路径** |
| `crates/nhs-app/` | 应用装配层 |
| `docs/` | 路线图、架构、逐里程碑验收记录 |
| `upstream/` | 上游只读参考，禁止修改 |
| `nix/`、`flake.nix` | 打包与开发环境 |

单一写入路径：GUI → app → core → adapter。不要为了让某个界面“直接改文件”而新建旁路。

## Commands

```bash
CARGO_TARGET_DIR=/var/tmp/nhs-target cargo test --workspace
CARGO_TARGET_DIR=/var/tmp/nhs-gui-target cargo test --manifest-path crates/nhs-gui/Cargo.toml
CARGO_TARGET_DIR=/var/tmp/nhs-gui-target cargo run  --manifest-path crates/nhs-gui/Cargo.toml
```

`CARGO_TARGET_DIR` 指向 `/var/tmp` 是刻意的（避免仓库内 target 膨胀），沿用即可。

E2E 隔离脚本：`scripts/test-e2e-isolation.sh`、`scripts/e2e-home-manager.sh`。
它们只在隔离环境跑，**不等于真机验收**。

## Current state (2026-11-10)

- 0.1.0 技术预览。产品路线以 `docs/product-roadmap.md` 为准；`M7 (hm)` 的工作进度以
  `docs/m7-hm-task-cards.md` 为准。
- 进行中的改动以 `git status` 为准，不要根据本文缓存判断工作区；不要 revert 进行中的内容。

## Audit checklist (completed — historical reference only)
For each upstream, inspect README, license, manifests, directory structure, CI, tests, and the relevant implementation for:
- package search/install/remove and Nix invocation;
- Home Manager module generation and imports;
- parsing and editing existing Nix files;
- diff, backup, conflict detection, and recovery;
- generation listing and rollback;
- build/apply/error handling;
- packaging and Linux runtime dependencies.

## Audit deliverables (completed — historical reference only)
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

At the end of that (completed) audit the report covered what was inspected,
which tests were run, what remained unverified, and the recommended minimal
viable version. That gate has been passed; current work follows
`docs/product-roadmap.md`.
