# M7 WP2 — E2E isolation hardening

Date: 2026-11-08. Scope: fix the isolation findings from the independent WP2
review of `scripts/e2e-home-manager.sh`. This change does **not** make the
product an activation tool: the product still never runs `home-manager switch`.
A single test-only `--execute` run was authorized on 2026-11-08 and is recorded
below; Gate 8 now has test-only evidence. No push, tag move, Nix GC or real-HOME
activation was performed.

## 1. Reviewed implementation

The harness was reviewed against the installed Home Manager
`0-unstable-2026-08-06` (`/nix/store/86nwi02lp209rc0ibnisvs8qivv0h5cv-home-manager-0-unstable-2026-08-06/bin/home-manager`)
and its generated activation script
(`/nix/store/2ybaklxr64475fy55i457pxrxqg71sjy-home-manager-generation/activate`).
Store paths are host- and build-specific; the line references are evidence for
this audit, not a stable API.

Relevant behaviour found in those files:

| Location | Behaviour |
| --- | --- |
| `home-manager` `setHomeManagerPathVariables` | `HM_PROFILE_DIR` is `$XDG_STATE_HOME/nix/profiles` only if that directory exists; otherwise it falls back to `$NIX_STATE_DIR/profiles/per-user/$USER` (default `/nix/var/nix/...`). |
| `home-manager` `setWorkDir` | `WORK_DIR="$(mktemp --tmpdir -d home-manager-build.XXX)"`; the activation generation is `$WORK_DIR/generation`, a GC root. `mktemp --tmpdir` honours `TMPDIR`. |
| `home-manager` `doSwitch` | `nix-env --profile "$HM_PROFILE_DIR/home-manager" --set "$generation"`; rollback uses `nix-env --rollback`. |
| activation `setupVars` | `HM_DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}/home-manager"`, state from `XDG_STATE_HOME`, global roots from `NIX_STATE_DIR`. |
| activation `migrateProfile` | If `$NIX_STATE_DIR/profiles/per-user/$USER/home-manager` exists it is treated as the old profile and **removed** after migration into the sandbox state directory. The harness therefore pre-creates the sandbox profile directory, uses a fake `USER`, and verifies that the fake user's fallback path does not exist. |
| activation `reloadSystemd` | Calls `systemctl --user is-system-running`, then `sd-switch --new-units`. It uses `${XDG_RUNTIME_DIR:-/run/user/$(id -u)}`. |
| activation end | `nix-store --realise "$newGenPath" --add-root "$currentGenGcPath"`; `currentGenGcPath=$XDG_STATE_HOME/home-manager/gcroots/current-home`. |
| `modules/systemd.nix` | `systemd.user.startServices` selects `sd-switch` vs the suggest script; `home.activation.reloadSystemd` is the DAG entry that performs the systemctl/reload block. |

## 2. P1 fixes

### 2.1 Every XDG directory and TMPDIR are sandbox paths

The child environment is built with `env -i` and sets:

| Variable | Value |
| --- | --- |
| `HOME` | `$ROOT/home` |
| `XDG_DATA_HOME` | `$ROOT/data` |
| `XDG_STATE_HOME` | `$ROOT/state` |
| `XDG_CONFIG_HOME` | `$ROOT/config` |
| `XDG_CACHE_HOME` | `$ROOT/cache` |
| `TMPDIR`, `TMP`, `TEMP` | `$ROOT/tmp` |
| `XDG_RUNTIME_DIR` | `$ROOT/runtime` (mode 0700) |
| `DBUS_SESSION_BUS_ADDRESS` | `unix:path=$ROOT/runtime/bus` (does not exist) |
| `NIX_REMOTE` | `daemon` (explicit) |
| `NIX_PATH` | `nixpkgs=<NHS_E2E_NIXPKGS>` (explicit) |

`NIX_STATE_DIR` is deliberately **not** set in the child. The Nix client
derives the daemon socket from it, and a manual check showed that setting it to
the sandbox makes `nix-build`/`nix-store` fail with
`cannot connect to socket at .../nix-state/daemon-socket/socket`. It is still
cleared from the caller environment and rejected in strict mode.

No caller `HOME`, XDG, `TMPDIR`, `DBUS_*`, `NIX_*` or `HOME_MANAGER_*` value is
inherited. The preflight prints each child value and lists caller variables that
were replaced (`caller_env_ignored=`). `NHS_E2E_STRICT_ENV=1` makes an inherited
Nix-state/home-manager variable a refusal instead of a silent replacement.

### 2.2 Home Manager profile and work directory

- `$ROOT/state/nix/profiles` is created **before** Home Manager runs and is
  verified with `readlink -f` to be canonical and inside the sandbox. This
  removes the fallback to `/nix/var/nix/profiles/per-user/$USER`.
- The fake `USER=nhs-e2e` makes the fallback path
  `/nix/var/nix/profiles/per-user/nhs-e2e`. `check_isolation` refuses to run if
  that path exists (file or symlink), so Home Manager cannot observe or migrate
  a per-user profile. Because `NIX_STATE_DIR` is not redirected, the Nix client
  still connects to the real daemon socket; the manual check above proves this
  combination works.
- **Postcondition:** every `home-manager build`, `switch` and
  `switch --rollback` return (success **or** failure) is followed immediately by
  `check_hm_fallback_postcondition`. If the fallback appeared, the harness sets
  `PRESERVE_SANDBOX=1`, reports the path, stops the sequence (no later build or
  rollback), and cleanup keeps the sandbox for inspection instead of removing
  it.
- `TMPDIR=$ROOT/tmp` makes `home-manager`'s `WORK_DIR` and its
  `$WORK_DIR/generation` GC root land in the sandbox. `run_with_timeout` is
  followed by `check_gc_roots`, which verifies the locations of `result-1`,
  `result-2`, the profile link, `$XDG_STATE_HOME/home-manager/gcroots`, and any
  `$TMPDIR/home-manager-build.*` directory.

### 2.3 User systemd / D-Bus

- The generated configuration sets `systemd.user.startServices = false;` and
  `home.activation.reloadSystemd = lib.mkForce "";`, removing the generated
  systemctl/sd-switch block from this configuration.
- Independently of that configuration change, the child has no reachable user
  manager: `XDG_RUNTIME_DIR` is an empty 0700 sandbox directory, there is no
  `bus` or `systemd` entry in it, and `DBUS_SESSION_BUS_ADDRESS` points to a
  non-existent socket inside that directory. `check_service_isolation` asserts
  these properties before every `home-manager` invocation.
- This means the E2E does **not** exercise Home Manager's systemd service
  activation. That is intentional: the harness has no safe private user
  systemd, so the alternative would be a false claim of service coverage.

### 2.4 Nix environment

- `NHS_E2E_HM` and `NHS_E2E_NIXPKGS` are mandatory. Both are resolved with
  `readlink -f`; the home-manager path must be
  `/nix/store/*/bin/home-manager`, and nixpkgs must be a `/nix/store` directory
  containing `default.nix`. The harness no longer searches `PATH` or `NIX_PATH`
  for them.
- The Nix tool directory is derived from the resolved `nix` executable and
  prepended to a fixed `PATH`; no caller `PATH` is inherited.
- `NIX_REMOTE=daemon` is explicit and the child has no caller `NIX_CONFIG`,
  `NIX_CONF_DIR`, `NIX_USER_CONF_FILES`, `NIX_STORE*`,
  `NIX_DAEMON_SOCKET_PATH`, `NIX_PROFILES`, `NIX_USER_PROFILE_DIR` or
  `NIX_STATE_DIR`.

## 3. P2/P3 fixes

- **Process groups:** `setsid -f -w` starts the command in a new session; the
  child writes its pid before `exec`, so the harness tracks the real
  process-group id even when `setsid` forks. Timeout and signal handling send
  TERM then KILL to that group. The watchdog is also its own session, so
  stopping it cannot leave a `sleep` child behind.
- **No removal under a live child:** `cleanup` stops the watchdog, then the
  active process group, and only then considers removing the tree. If a group
  cannot be stopped, it keeps the tree and exits non-zero.
- **Postcondition failure preserves the tree:** a fallback path that appears
  after a Home Manager command sets `PRESERVE_SANDBOX=1`; `cleanup` then keeps
  the sandbox and exits with the failure status.
- **Cleanup re-validation:** `root_is_removable` re-checks that the tree is
  directly under `/var/tmp/nhs-e2e.??????`, is not a symlink, resolves to
  itself, is owned by the current user, is not the real HOME, is not **inside**
  the real HOME, and is not an ancestor of it. `rm -rf` is only called after
  that check.
- **Signal status:** HUP/INT/TERM map to 129/130/143; the test suite verifies
  SIGTERM removes the tree and stops the whole process group.
- **Real environment snapshot:** before/after comparison now covers the real
  profile link, `~/.local/state/nix/profiles`, `~/.config/home-manager`, the app
  state directory, `~/.local/share/home-manager`, `~/.cache/home-manager`, and
  `/nix/var/nix/profiles/per-user/<user>`. The digest reads file contents but
  never prints them and does not follow symlinks out of the tree. A shared
  `path_state()` distinguishes **present**, **absent** and **unreadable** by
  walking the parent chain and checking search permission on every existing
  ancestor; `[ ! -e ]` alone is never treated as proof of absence. An
  unreadable file/directory, inaccessible parent, failed traversal, failed hash
  or failed temporary-manifest write makes `tree_digest` and the label helpers
  return non-zero. Preflight refuses to continue, and `--execute` refuses to
  take or compare a snapshot. Two unreadable snapshots are never accepted as
  "unchanged".
- **Repeated runs:** each run uses a fresh `mktemp` tree; the integration test
  runs preflight twice and asserts two distinct roots, both removed.
- **SIGKILL** cannot run a trap. The tree remains under `/var/tmp`; no real
  profile, config or app state is touched. Manual cleanup is required.

## 4. Automated tests

`scripts/test-e2e-isolation.sh` sources the harness in `NHS_E2E_LIB_ONLY=1` mode
and runs 69 cases. The integration cases run `--preflight` only; no
`home-manager switch` or rollback is executed. The suite is also called from
`crates/nhs-gui/scripts/check-all.sh`.

Coverage by requirement:

| Requirement | Cases |
| --- | --- |
| Store-path validation | non-store nixpkgs; non-store and missing home-manager |
| Caller Nix environment | default clearing/reporting; strict rejection of `NIX_REMOTE`; activation authorization required/accepted |
| Sandbox path checks | valid tree; outside `/var/tmp`; root equal to HOME; root inside HOME; missing profile dir; mode not 700 |
| Fallback postcondition | fallback created during a child run detected; next step blocked; sandbox preserved on detection |
| Symlink escape | symlinked XDG cache; symlinked profile dir; symlinked root refused by cleanup |
| Cleanup real-HOME boundary | sandbox inside real HOME not removable; cleanup refuses and leaves it intact |
| Service isolation | bus socket rejected; runtime mode not 700 rejected |
| Child environment | every path value and the dead D-Bus address checked |
| Cleanup | valid tree removed; symlinked tree refused; sandbox inside real HOME refused; active process group stopped before removal; no child left |
| Timeout | short command; TERM-ignoring helper returns 124 and its group dies; invalid timeout rejected |
| Interruption | SIGTERM returns 143, removes the tree and kills the group |
| Real-environment guard | snapshot stable; snapshot detects a config change; unreadable directory/file fails closed; preflight refuses an unreadable path; snapshot refuses an unreadable tree |
| Path state / inaccessible parent | target under an inaccessible parent fails closed; preflight rejects an unreadable fallback parent; postcondition reports `state=unreadable`, not `absent`; profile/fallback labels fail closed |
| Preflight integration | clean run; hostile XDG/TMP/DBUS caller env cleared; profile dir/runtime/workdir/home inspected; strict mode; bad store paths; repeated runs; generated config parses; Nix client works without a redirected `NIX_STATE_DIR`; help/unknown option |

Final evidence for the automated suite: `passed=69 failed=0 skipped=0`; no
`sleep`, `home-manager`, or sandbox process remained. The separately authorized
`--execute` run kept `/var/tmp/nhs-e2e.d45fGg` for review.

## 4.1 Authorized isolated activation (2026-11-08)

After explicit authorization, one `--execute` run was made with:

```sh
KEEP_E2E=1 NHS_E2E_ALLOW_ACTIVATION=1 \
NHS_E2E_TIMEOUT=600 NHS_E2E_KILL_GRACE=5 \
NHS_E2E_HM=/nix/store/.../bin/home-manager \
NHS_E2E_NIXPKGS=/nix/store/...-source \
sh scripts/e2e-home-manager.sh --execute
```

Result: `e2e=done`, `real_environment_unchanged=yes`, sandbox
`/var/tmp/nhs-e2e.d45fGg` kept for review.

| Evidence | Observed |
| --- | --- |
| Build/switch generation 1 | `build-1.log`, `switch-1.log`; `home-manager-1-link` |
| Build/switch generation 2 with `pkgs.hello` | `build-2.log`, `switch-2.log`; `hello` present in the generation-2 `home-path` |
| Rollback | `rollback.log`; profile is `home-manager-1-link` |
| Rollback removed `hello` | `home-manager-1-link/home-path/bin/hello` absent; generation-2 store still contains it |
| Profile/GC roots | `state/nix/profiles/home-manager -> home-manager-1-link`; `state/home-manager/gcroots/current-home` points at generation 1 |
| Build result link | `home/result` points at the generation-2 store output; inside the sandbox |
| Systemd/D-Bus commands | no `systemctl`, `sd-switch` or D-Bus command in any log; only the empty `Activating reloadSystemd` label remains |
| Real environment | real profile still `home-manager-3-link`; real config/app state absent; HM data digest unchanged; `nhs-e2e` fallback absent |
| Processes | none left after the run |

The first attempt failed before any switch because `home-manager build` does not
accept `nix-build`'s `-o` option in this version. The harness now runs
`home-manager build` with `HOME` as its working directory and checks the default
`$HOME/result` link; this was a harness bug, not a product or isolation failure.
No real Home Manager command was run in that failed attempt.

This is test-only evidence. The application itself still has no activation
path, and the release notes continue to say that it never runs
`home-manager switch`.

## 5. Residual risks and boundaries

- The authorized `--execute` is test-only: it ran build → switch → package
  install → switch → rollback in the sandbox. The real environment was
  unchanged. The product itself still has no activation path.
- The generated configuration was exercised by that run; the sandbox systemd
  reload entry was empty and no `systemctl`/`sd-switch` command appeared in the
  logs. This is not the same as testing service activation, which remains
  disabled.
- `NIX_REMOTE=daemon` is a host assumption. A single-user Nix installation may
  need a different explicit remote; that would be a reviewed change, not a
  fallback to the caller's environment.
- If a real account literally named `nhs-e2e` has a Home Manager per-user
  profile, preflight refuses to run. The fake user is fixed; changing it is a
  reviewed code change, not an environment override.
- The Nix client still uses its default state directory for the daemon socket.
  The harness does not write there, but it also cannot prove a Nix daemon
  implementation will never write client state outside the sandbox.
- The harness does not implement a private user systemd/bus. Service activation
  is disabled rather than tested.
- Same-user concurrent replacement of sandbox paths remains a TOCTOU window.
  The 0700 fresh root and canonical-path checks reduce it; this is not a
  hostile-multi-user boundary.
- SIGKILL leaves the sandbox behind; `KEEP_E2E=1` also keeps it intentionally.
- The real-environment snapshot reads (hashes) the user's Home Manager config
  and data. It does not print contents and does not write them.

## 6. Gate status

| Gate | Status |
| --- | --- |
| E2E isolation preflight | **passes** (`preflight=ok`) |
| Isolation negative tests | **69 passed** |
| Real E2E activation/rollback (gate 8) | **PASS (test-only, 2026-11-08)** — build/switch/install/rollback in `/var/tmp/nhs-e2e.d45fGg`; real environment unchanged |
| Push / tag move / release | not performed |
| Nix GC | not performed |
| Real Home Manager profile/config/app state | unchanged in this phase |
