# M7.2 — generation rollback in isolation

Date: 2026-11-05. Host: CachyOS, niri, one 1× output. Home Manager from the
store (`0-unstable-2026-08-06`), nixpkgs source from the store. **No command was
run against the real user profile.** The product still has no rollback button.

## 1. Isolation preflight (before any switch)

`/var/tmp/nhs-m72/preflight.sh` created a sandbox with all four XDG roots and
`HOME` inside one temporary directory, then refused to continue unless every
write target resolved inside it:

- `HOME`, `XDG_STATE_HOME`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` under the sandbox;
- the Home Manager profile directory (`$XDG_STATE_HOME/nix/profiles`) under it;
- `NIX_USER_PROFILE_DIR`, `NIX_STATE_DIR` and `NIX_PROFILES` **unset** — the first
  run failed here, because the shell had
  `NIX_PROFILES=/nix/var/nix/profiles/default /home/ysz/.nix-profile`, i.e. a
  pointer at real state. The experiment environment clears all three;
- the toolchain is the absolute store path
  `/nix/store/86nwi…-home-manager-0-unstable-2026-08-06/bin/home-manager`, and if
  a `home-manager` happens to be on `PATH` it must resolve to the same file;
- the nixpkgs source is the store tree that matches the installed Home Manager.

The preflight is re-run before each experiment rather than assumed.

## 2. Real generations in the sandbox

Three `home-manager switch` runs, each with a changed `home.nix`:

| Generation | Content | Time |
| --- | --- | --- |
| 1 | base user/state only | 21 s |
| 2 | + `pkgs.jq` | 56 s |
| 3 | + `pkgs.jq pkgs.ripgrep`, session variable | 109 s |

Profile: `/var/tmp/nhs-m72/run-01/state/nix/profiles/home-manager → home-manager-3-link`.
The first attempt failed activation with `USER is "yueshizhe", expected "m72"`;
the sandbox now sets `USER`/`LOGNAME` to the sandbox user, not just `HOME`.

## 3. What the installed Home Manager can do

`home-manager switch --rollback` only. The script implements it as
`nix-env --profile … --rollback` followed by the generation's `activate`. There
is **no** `--switch-generation <n>`, so the product cannot honestly offer
"roll back to generation 1".

## 4. Experiments

| Scenario | Command | Result |
| --- | --- | --- |
| Official rollback | `home-manager switch --rollback` | exit 0, `home-manager` 3 → 2, activation ran in the sandbox |
| Arbitrary target (not an HM interface) | `nix-env --profile … --switch-generation 3` + `home-manager-3-link/activate` | exit 0, 2 → 3; proves the mechanism, but `nix-env` is outside this product's allowlist and is **not** proposed for the product |
| Failure | `nix-env … --switch-generation 99` | exit 1, `profile version 99 does not exist`, current unchanged (3) |
| Timeout | `timeout 0.3 home-manager switch --rollback` | exit 124 — **profile already switched 3 → 2**, activation was interrupted: a real inconsistency window |
| Cancel | SIGTERM to the process group after 0.15 s | exit 143, current unchanged (2): the signal arrived before the profile switch |
| Recovery | run the current generation's `activate --driver-version 1` | exit 0; consistent again |

The timeout case is the important finding: rollback is **not atomic**. Between
`nix-env --set/--rollback` and a completed activation there is a window where the
profile points at the target but the home directory does not yet match it.
Cancelling early avoids it; cancelling late does not.

## 5. Product-side work (no execution)

`nhs-core/src/rollback.rs` adds a read-only plan:

- `RollbackPlan::Previous { program: HomeManager, args: ["switch", "--rollback"] }`
  when the request is the previous generation (implicit or explicit);
- `RollbackPlan::Unsupported { reason }` for the current generation, an unknown
  id, an older-than-previous id, a single-generation profile, or no current
  generation.

Seven unit tests cover those cases. Nothing in the product spawns the command;
there is no UI change, and history remains read-only with no rollback control.

## 6. Isolation and cleanup

- The real profile still points at `home-manager-3-link`; the real
  `~/.config/home-manager` and `~/.local/state/nix-home-studio` do not exist.
- All sandbox output was under `/var/tmp/nhs-m72/run-01` (112 KiB of links/logs).
  After the experiments, `pgrep` showed no sandbox `home-manager`/`nix-build`
  process or process group, so the tree was removed. The preflight and
  generation scripts are kept at `/var/tmp/nhs-m72/` for reproduction.
- The sandbox's Nix store closure (jq/ripgrep activation) stays in `/nix`; free
  space fell from ~5 GiB to ~3 GiB. No garbage collection was run: it can remove
  unrelated user paths and is out of scope.

## 7. Real-host read-only check

The normal app binary was launched against the real `HOME` (no sandbox) and its
History screen was opened through AT-SPI:

- discovery reports `profile-only`, 3 generations, read-only, `switch.available=false`;
- the screen shows the banner *"Rolling back is not available in this version.
  These lists are read-only."*, `Home Manager generations (3)`, generation 3
  marked `current`, and zero application snapshots;
- the only buttons are the language toggle and the seven step buttons — there is
  no rollback control;
- after closing the app, `~/.local/state/nix/profiles/home-manager` still points
  at `home-manager-3-link` and the app state directory is still absent.

Screenshot: `/var/tmp/nhs-m72/real-history-full.png`.

## 8. Conclusion

A one-step rollback works and is the only form this toolchain supports. It is
**not** atomic under interruption, and arbitrary-generation rollback would
require `nix-env`, which the product's process allowlist deliberately excludes.
M7.2 therefore does **not** authorise a GUI rollback feature; any future proposal
must first define recovery for an interrupted activation and stay inside the
allowlist (or extend it deliberately).

## Not verified

- Rollback on a real user configuration (not authorised).
- Windows of interruption between the profile switch and each activation step.
- Nix garbage collection and its effect on kept generations.
