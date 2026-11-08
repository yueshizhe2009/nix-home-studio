#!/bin/sh
# Isolated Home Manager E2E harness.
#
# Default mode is PREFLIGHT ONLY: it creates a temporary HOME/XDG tree, checks
# that every write target, profile and GC root is inside it, prints the plan,
# and exits. It does not run `home-manager switch`.
#
# Activation is opt-in and must be explicitly authorised:
#
#   NHS_E2E_HM=/nix/store/.../bin/home-manager \
#   NHS_E2E_NIXPKGS=/nix/store/...-source \
#   NHS_E2E_ALLOW_ACTIVATION=1 sh scripts/e2e-home-manager.sh --execute
#
# The harness never inherits the caller's NIX_*, HOME, XDG, TMPDIR or D-Bus
# environment. Nix-state variables are cleared for the child by default and
# rejected with NHS_E2E_STRICT_ENV=1. It starts home-manager in its own process
# group so a timeout or signal stops the whole tree before the sandbox is
# removed.

# Intentionally single-quoted: `$1`, `$2` are expanded by the child shell.
# shellcheck disable=SC2016

set -eu

TMP_ROOT_BASE=/var/tmp
E2E_USER=nhs-e2e
E2E_TIMEOUT=${NHS_E2E_TIMEOUT:-600}
KILL_GRACE=${NHS_E2E_KILL_GRACE:-5}

# Variables that change where Nix writes state, profiles, the store or config.
# They are never passed to the child (`env -i`). With NHS_E2E_STRICT_ENV=1 they
# are also rejected in the caller; the default is to clear them explicitly and
# report them so a normal Nix shell (which exports NIX_PROFILES) can run.
DENY_ENV_VARS="
NIX_PROFILES
NIX_USER_PROFILE_DIR
NIX_STATE_DIR
NIX_REMOTE
NIX_CONFIG
NIX_CONF_DIR
NIX_USER_CONF_FILES
NIX_STORE_DIR
NIX_STORE
NIX_DAEMON_SOCKET_PATH
HOME_MANAGER_CONFIG
HOME_MANAGER_BACKUP_EXT
"

# Variables that are always replaced in the child, and reported if the caller
# had set them. HOME is included because the child gets the sandbox home.
OVERRIDE_ENV_VARS="
HOME
XDG_DATA_HOME
XDG_STATE_HOME
XDG_CONFIG_HOME
XDG_CACHE_HOME
XDG_RUNTIME_DIR
TMPDIR
TMP
TEMP
DBUS_SESSION_BUS_ADDRESS
"

CALLER_IGNORED=

ROOT=
REAL_HOME=
HM=
NP=
NIX_BIN=
E2E_PATH=
HOME_DIR=
STATE_DIR=
CONFIG_DIR=
CACHE_DIR=
DATA_DIR=
TMP_DIR=
RUNTIME_DIR=
PROFILES=
CONFIG_FILE=
RESULT_1=
RESULT_2=
ACTIVE_CHILD=
ACTIVE_GROUP=
ACTIVE_PIDFILE=
WATCHDOG=
WATCHDOG_GROUP=
WATCHDOG_PIDFILE=
PRESERVE_SANDBOX=0

usage() {
    cat <<'EOF'
usage: scripts/e2e-home-manager.sh [--preflight|--execute]

  --preflight  (default) create the sandbox and verify isolation; no activation
  --execute    run build/switch/rollback; requires NHS_E2E_ALLOW_ACTIVATION=1

Both modes require explicit store paths; the harness does not search PATH or
NIX_PATH for them:

  NHS_E2E_HM=/nix/store/.../bin/home-manager
  NHS_E2E_NIXPKGS=/nix/store/...-source

The child runs with `env -i`: no caller HOME, XDG_*, TMPDIR, DBUS_* or NIX_*
variable is inherited. Service activation is disabled in the generated
configuration because this harness does not provide a private user systemd.

Set NHS_E2E_STRICT_ENV=1 to turn an inherited NIX_*/HOME_MANAGER_* variable
into an error instead of clearing and reporting it.
EOF
}

fail() {
    echo "E2E FAIL: $*" >&2
    exit 1
}

is_under() {
    case "$1" in
        "$2"/*) return 0 ;;
        *) return 1 ;;
    esac
}

hm_fallback_path() {
    # Test-only seam: the harness itself never sets NHS_E2E_LIB_ONLY.
    if [ "${NHS_E2E_LIB_ONLY:-0}" = 1 ] && [ -n "${NHS_E2E_TEST_FALLBACK_PATH:-}" ]; then
        printf '%s' "$NHS_E2E_TEST_FALLBACK_PATH"
        return 0
    fi
    printf '/nix/var/nix/profiles/per-user/%s' "$E2E_USER"
}

# Distinguish a path that is present, one that can be proved absent, and one
# whose parent chain cannot be searched. `[ ! -e path ]` is not enough: it also
# succeeds when an ancestor is unreadable.
path_state() {
    path=$1
    case "$path" in
        /*) : ;;
        *) path=${PWD:-/}/$path ;;
    esac
    while [ "$path" != "/" ] && [ "${path%/}" != "$path" ]; do
        path=${path%/}
    done
    if [ "$path" = "/" ]; then
        if [ -d / ] && [ -x / ]; then
            printf present
            return 0
        fi
        printf unreadable
        return 1
    fi
    if [ ! -d / ] || [ ! -x / ]; then
        printf unreadable
        return 1
    fi

    remaining=${path#/}
    current=
    while [ -n "$remaining" ]; do
        component=${remaining%%/*}
        case "$remaining" in
            */*) remaining=${remaining#*/} ;;
            *) remaining= ;;
        esac
        [ -n "$component" ] || continue
        if [ -z "$current" ]; then
            current="/$component"
        else
            current="$current/$component"
        fi

        if [ -n "$remaining" ]; then
            # Ancestor: it must be a searchable directory.
            if [ -L "$current" ]; then
                resolved=$(readlink -f "$current" 2>/dev/null) || {
                    printf unreadable
                    return 1
                }
                if [ ! -d "$resolved" ]; then
                    printf absent
                    return 0
                fi
                if [ ! -x "$resolved" ]; then
                    printf unreadable
                    return 1
                fi
            elif [ ! -e "$current" ]; then
                printf absent
                return 0
            elif [ ! -d "$current" ]; then
                printf absent
                return 0
            elif [ ! -x "$current" ]; then
                printf unreadable
                return 1
            fi
        else
            # Target: the parent chain was proved searchable above.
            if [ -e "$current" ] || [ -L "$current" ]; then
                printf present
                return 0
            fi
            printf absent
            return 0
        fi
    done
    printf absent
    return 0
}

hm_fallback_absent() {
    fallback=${1:-$(hm_fallback_path)}
    state=$(path_state "$fallback") || return 1
    [ "$state" = absent ]
}

# Postcondition for every home-manager invocation, success or failure. If the
# fallback is present or cannot be checked, keep the sandbox and stop before any
# later build/switch.
check_hm_fallback_postcondition() {
    fallback=${1:-$(hm_fallback_path)}
    state=$(path_state "$fallback") || state=unreadable
    if [ "$state" = absent ]; then
        return 0
    fi
    PRESERVE_SANDBOX=1
    echo "E2E FAIL: Home Manager fallback is not provably absent: $fallback (state=$state)" >&2
    echo "E2E: keeping sandbox $ROOT; stopping before the next home-manager command" >&2
    return 1
}

require_integer() {
    value=$1
    name=$2
    min=$3
    max=$4
    case "$value" in
        ''|*[!0-9]*) fail "$name must be an integer: $value" ;;
    esac
    [ "$value" -ge "$min" ] && [ "$value" -le "$max" ] ||
        fail "$name must be between $min and $max: $value"
}

require_tool() {
    command -v "$1" >/dev/null 2>&1 || fail "required tool not found: $1"
}

# Resolve and validate an absolute /nix/store home-manager executable.
validate_home_manager() {
    candidate=${1:-}
    [ -n "$candidate" ] || fail "NHS_E2E_HM is required (absolute /nix/store path)"
    resolved=$(readlink -f "$candidate" 2>/dev/null) || fail "cannot resolve home-manager: $candidate"
    case "$resolved" in
        /nix/store/*/bin/home-manager) : ;;
        *) fail "home-manager does not resolve to /nix/store/*/bin/home-manager: $resolved" ;;
    esac
    [ -f "$resolved" ] && [ -x "$resolved" ] ||
        fail "home-manager is not an executable file: $resolved"
    HM=$resolved
}

# Resolve and validate an absolute nixpkgs source tree in the Nix store.
validate_nixpkgs() {
    candidate=${1:-}
    [ -n "$candidate" ] || fail "NHS_E2E_NIXPKGS is required (absolute /nix/store path)"
    resolved=$(readlink -f "$candidate" 2>/dev/null) || fail "cannot resolve nixpkgs: $candidate"
    case "$resolved" in
        /nix/store/*) : ;;
        *) fail "nixpkgs does not resolve into /nix/store: $resolved" ;;
    esac
    [ -d "$resolved" ] || fail "nixpkgs is not a directory: $resolved"
    [ -f "$resolved/default.nix" ] || fail "nixpkgs has no default.nix: $resolved"
    NP=$resolved
}

# Record caller variables that are cleared for the child. Strict mode turns the
# contract into a refusal for the Nix-state variables.
collect_caller_environment() {
    CALLER_IGNORED=
    for variable in $DENY_ENV_VARS; do
        eval "value=\${$variable-}"
        [ -n "$value" ] || continue
        if [ "${NHS_E2E_STRICT_ENV:-0}" = 1 ]; then
            fail "$variable is set ($value); refusing to run with a redirected Nix environment"
        fi
        CALLER_IGNORED="$CALLER_IGNORED $variable"
    done
    for variable in $OVERRIDE_ENV_VARS; do
        eval "value=\${$variable-}"
        [ -n "$value" ] || continue
        CALLER_IGNORED="$CALLER_IGNORED $variable"
    done
    CALLER_IGNORED=${CALLER_IGNORED# }
    [ -n "$CALLER_IGNORED" ] || CALLER_IGNORED=none
}

require_activation_authorization() {
    [ "${NHS_E2E_ALLOW_ACTIVATION:-0}" = 1 ] ||
        fail "activation is not authorised; set NHS_E2E_ALLOW_ACTIVATION=1 explicitly"
}

resolve_real_home() {
    [ -n "${HOME:-}" ] || fail "HOME is not set"
    REAL_HOME=$(readlink -f "$HOME") || fail "cannot resolve HOME: $HOME"
    [ -d "$REAL_HOME" ] || fail "HOME is not a directory: $REAL_HOME"
}

check_root_base() {
    [ -d "$TMP_ROOT_BASE" ] || fail "temporary root base is missing: $TMP_ROOT_BASE"
    [ ! -L "$TMP_ROOT_BASE" ] || fail "temporary root base is a symlink: $TMP_ROOT_BASE"
    resolved=$(readlink -f "$TMP_ROOT_BASE") || fail "cannot resolve $TMP_ROOT_BASE"
    [ "$resolved" = "$TMP_ROOT_BASE" ] ||
        fail "temporary root base is not canonical: $TMP_ROOT_BASE -> $resolved"
}

prepare_root() {
    check_root_base
    ROOT=$(mktemp -d "$TMP_ROOT_BASE/nhs-e2e.XXXXXX") || fail "cannot create sandbox"
    chmod 700 "$ROOT" || fail "cannot set sandbox mode"
    ROOT=$(readlink -f "$ROOT") || fail "cannot resolve sandbox"
    case "$ROOT" in
        "$TMP_ROOT_BASE"/nhs-e2e.??????) : ;;
        *) fail "sandbox has an unexpected path: $ROOT" ;;
    esac
}

assign_layout() {
    HOME_DIR="$ROOT/home"
    STATE_DIR="$ROOT/state"
    CONFIG_DIR="$ROOT/config"
    CACHE_DIR="$ROOT/cache"
    DATA_DIR="$ROOT/data"
    TMP_DIR="$ROOT/tmp"
    RUNTIME_DIR="$ROOT/runtime"
    PROFILES="$STATE_DIR/nix/profiles"
    CONFIG_FILE="$CONFIG_DIR/home-manager/home.nix"
    # `home-manager build` creates ./result in its working directory; run it
    # with HOME as the working directory and check that link for sandboxing.
    RESULT_1="$HOME_DIR/result"
    RESULT_2="$HOME_DIR/result"
}

make_managed_dir() {
    dir=$1
    [ -n "$dir" ] || fail "empty managed directory"
    parent=$(dirname "$dir")
    [ -d "$parent" ] || fail "parent directory is missing: $parent"
    [ ! -L "$dir" ] || fail "refusing symlinked managed directory: $dir"
    if [ -e "$dir" ]; then
        [ -d "$dir" ] || fail "managed path is not a directory: $dir"
    else
        mkdir "$dir" || fail "cannot create managed directory: $dir"
    fi
    resolved=$(readlink -f "$dir") || fail "cannot resolve managed directory: $dir"
    [ "$resolved" = "$dir" ] || fail "managed directory is not canonical: $dir -> $resolved"
    is_under "$resolved" "$ROOT" || fail "managed directory is outside the sandbox: $dir"
}

prepare_directories() {
    make_managed_dir "$HOME_DIR"
    make_managed_dir "$STATE_DIR"
    make_managed_dir "$CONFIG_DIR"
    make_managed_dir "$CACHE_DIR"
    make_managed_dir "$DATA_DIR"
    make_managed_dir "$TMP_DIR"
    make_managed_dir "$RUNTIME_DIR"
    chmod 700 "$RUNTIME_DIR" || fail "cannot set runtime directory mode"
    make_managed_dir "$STATE_DIR/nix"
    # This directory must exist before home-manager runs. Otherwise Home Manager
    # falls back to /nix/var/nix/profiles/per-user/$USER.
    make_managed_dir "$PROFILES"
    make_managed_dir "$CONFIG_DIR/home-manager"
}

check_isolation() {
    [ -n "$ROOT" ] || fail "sandbox is not initialised"
    [ -d "$ROOT" ] || fail "sandbox disappeared: $ROOT"
    [ ! -L "$ROOT" ] || fail "sandbox is a symlink: $ROOT"
    resolved=$(readlink -f "$ROOT") || fail "cannot resolve sandbox: $ROOT"
    [ "$resolved" = "$ROOT" ] || fail "sandbox is not canonical: $ROOT -> $resolved"
    case "$ROOT" in
        "$TMP_ROOT_BASE"/nhs-e2e.??????) : ;;
        *) fail "sandbox is not under $TMP_ROOT_BASE/nhs-e2e.*: $ROOT" ;;
    esac
    [ "$(stat -c %u "$ROOT")" = "$(id -u)" ] || fail "sandbox is not owned by the current user"
    [ "$(stat -c %a "$ROOT")" = 700 ] || fail "sandbox mode is not 700: $ROOT"

    [ -n "$REAL_HOME" ] && [ -d "$REAL_HOME" ] || fail "real HOME is not available"
    [ "$(readlink -f "$REAL_HOME")" = "$REAL_HOME" ] ||
        fail "real HOME is not canonical: $REAL_HOME"
    [ "$ROOT" != "$REAL_HOME" ] || fail "sandbox is the real HOME"
    case "$ROOT" in
        "$REAL_HOME"/*) fail "sandbox is inside the real HOME" ;;
    esac
    case "$REAL_HOME" in
        "$ROOT"/*) fail "sandbox is an ancestor of the real HOME" ;;
    esac

    for directory in \
        "$HOME_DIR" \
        "$STATE_DIR" \
        "$CONFIG_DIR" \
        "$CACHE_DIR" \
        "$DATA_DIR" \
        "$TMP_DIR" \
        "$RUNTIME_DIR" \
        "$STATE_DIR/nix" \
        "$PROFILES" \
        "$CONFIG_DIR/home-manager"
    do
        [ -d "$directory" ] || fail "managed directory is missing: $directory"
        [ ! -L "$directory" ] || fail "managed directory is a symlink: $directory"
        resolved=$(readlink -f "$directory") || fail "cannot resolve managed directory: $directory"
        [ "$resolved" = "$directory" ] ||
            fail "managed directory is not canonical: $directory -> $resolved"
        is_under "$resolved" "$ROOT" ||
            fail "managed directory is outside the sandbox: $directory"
    done

    [ -x "$HM" ] || fail "home-manager is not executable: $HM"
    [ -f "$NP/default.nix" ] || fail "nixpkgs source is not usable: $NP"

    # NIX_STATE_DIR cannot be redirected here: the Nix client derives the
    # daemon socket from it. Instead, the sandbox profile directory exists and
    # the per-user fallback for the fake E2E user must be provably absent.
    fallback_state=$(path_state "$(hm_fallback_path)") || fallback_state=unreadable
    [ "$fallback_state" = absent ] ||
        fail "Home Manager fallback is not provably absent: $(hm_fallback_path) (state=$fallback_state)"
}

# The generated configuration disables service starting and removes the reload
# activation entry. As defence in depth the runtime directory is empty and the
# only D-Bus address points at a non-existent socket inside it, so even a
# generated script that still called systemctl could not reach the real user
# manager.
check_service_isolation() {
    [ -d "$RUNTIME_DIR" ] || fail "runtime directory is missing: $RUNTIME_DIR"
    [ ! -L "$RUNTIME_DIR" ] || fail "runtime directory is a symlink: $RUNTIME_DIR"
    [ "$(stat -c %a "$RUNTIME_DIR")" = 700 ] ||
        fail "runtime directory mode is not 700: $RUNTIME_DIR"
    [ ! -e "$RUNTIME_DIR/bus" ] ||
        fail "sandbox runtime directory contains a bus socket: $RUNTIME_DIR/bus"
    [ ! -e "$RUNTIME_DIR/systemd" ] ||
        fail "sandbox runtime directory contains a systemd manager: $RUNTIME_DIR/systemd"
}

# Read-only digest of a real path. Never follows symlinks out of the tree and
# never prints file contents. Returns non-zero when any part cannot be read, so
# callers can fail closed instead of comparing two incomplete snapshots.
tree_digest() {
    target=$1
    state=$(path_state "$target") || state=unreadable
    case "$state" in
        absent)
            printf 'absent'
            return 0
            ;;
        unreadable)
            printf 'unreadable'
            return 1
            ;;
    esac
    if [ -L "$target" ]; then
        printf 'link:%s' "$(readlink "$target")"
        return 0
    fi
    if [ -f "$target" ]; then
        if digest=$(sha256sum "$target" 2>/dev/null); then
            printf 'file:%s' "$(printf '%s' "$digest" | awk '{print $1}')"
            return 0
        fi
        printf 'unreadable'
        return 1
    fi
    [ -d "$target" ] || {
        printf 'other'
        return 1
    }

    digest_base=${TMP_DIR:-${TMPDIR:-/var/tmp}}
    [ -d "$digest_base" ] || {
        printf 'unreadable'
        return 1
    }
    work=$(mktemp -d "$digest_base/nhs-digest.XXXXXX") || {
        printf 'unreadable'
        return 1
    }
    if digest=$(
        trap 'rm -rf "$work"' EXIT
        cd "$target" || exit 1
        find . -xdev -type f -exec sha256sum {} + >"$work/files" 2>/dev/null || exit 1
        LC_ALL=C sort "$work/files" >"$work/files.sorted" 2>/dev/null || exit 1
        find . -xdev -type l -printf 'link %p -> %l\n' >"$work/links" 2>/dev/null || exit 1
        LC_ALL=C sort "$work/links" >"$work/links.sorted" 2>/dev/null || exit 1
        find . -xdev -type d -printf 'dir %p\n' >"$work/dirs" 2>/dev/null || exit 1
        LC_ALL=C sort "$work/dirs" >"$work/dirs.sorted" 2>/dev/null || exit 1
        cat "$work/files.sorted" "$work/links.sorted" "$work/dirs.sorted" >"$work/all" 2>/dev/null || exit 1
        sha256sum "$work/all" 2>/dev/null | awk '{print $1}'
    ); then
        if [ -n "$digest" ]; then
            printf 'tree:%s' "$digest"
            return 0
        fi
    fi
    printf 'unreadable'
    return 1
}

real_profile_label() {
    profile="$REAL_HOME/.local/state/nix/profiles/home-manager"
    state=$(path_state "$profile") || state=unreadable
    case "$state" in
        absent) printf absent ;;
        present)
            if [ -L "$profile" ]; then
                printf 'link:%s' "$(readlink "$profile")"
            else
                printf present
            fi
            ;;
        *)
            printf unreadable
            return 1
            ;;
    esac
}

e2e_fallback_label() {
    fallback=$(hm_fallback_path)
    state=$(path_state "$fallback") || state=unreadable
    case "$state" in
        absent) printf absent ;;
        present)
            if [ -L "$fallback" ]; then
                printf 'link:%s' "$(readlink "$fallback")"
            else
                printf present
            fi
            ;;
        *)
            printf unreadable
            return 1
            ;;
    esac
}

# Emit one snapshot line. Returns non-zero if the path could not be read.
snapshot_tree_line() {
    label=$1
    path=$2
    if value=$(tree_digest "$path"); then
        printf '%s=%s\n' "$label" "$value"
    else
        printf '%s=unreadable\n' "$label"
        printf 'E2E: cannot read %s completely: %s\n' "$label" "$path" >&2
        return 1
    fi
}

# Everything the harness must not change. Compared before and after activation.
# Fails closed: an unreadable path is not treated as "unchanged".
snapshot_real_environment() {
    status=0
    if profile=$(real_profile_label); then
        printf 'profile=%s\n' "$profile"
    else
        printf 'profile=unreadable\n'
        printf 'E2E: cannot read profile completely: %s\n' "$REAL_HOME/.local/state/nix/profiles/home-manager" >&2
        status=1
    fi
    snapshot_tree_line profiles_dir "$REAL_HOME/.local/state/nix/profiles" || status=1
    snapshot_tree_line config "$REAL_HOME/.config/home-manager" || status=1
    snapshot_tree_line app_state "$REAL_HOME/.local/state/nix-home-studio" || status=1
    snapshot_tree_line data "$REAL_HOME/.local/share/home-manager" || status=1
    snapshot_tree_line cache "$REAL_HOME/.cache/home-manager" || status=1
    snapshot_tree_line per_user "/nix/var/nix/profiles/per-user/$(id -un)" || status=1
    if fallback=$(e2e_fallback_label); then
        printf 'e2e_fallback=%s\n' "$fallback"
    else
        printf 'e2e_fallback=unreadable\n'
        printf 'E2E: cannot read fallback path completely: %s\n' "$(hm_fallback_path)" >&2
        status=1
    fi
    return "$status"
}

# Preflight version of the same check: report the line, then fail immediately.
print_real_digest() {
    label=$1
    path=$2
    if ! snapshot_tree_line "$label" "$path"; then
        fail "cannot read the real environment completely: $path"
    fi
}

print_real_label() {
    label=$1
    producer=$2
    if value=$("$producer"); then
        printf '%s=%s\n' "$label" "$value"
    else
        printf '%s=unreadable\n' "$label"
        fail "cannot read the real environment completely: $label"
    fi
}

print_child_environment() {
    printf 'child_home=%s\n' "$HOME_DIR"
    printf 'child_user=%s\n' "$E2E_USER"
    printf 'child_xdg_data=%s\n' "$DATA_DIR"
    printf 'child_xdg_state=%s\n' "$STATE_DIR"
    printf 'child_xdg_config=%s\n' "$CONFIG_DIR"
    printf 'child_xdg_cache=%s\n' "$CACHE_DIR"
    printf 'child_tmpdir=%s\n' "$TMP_DIR"
    printf 'child_runtime=%s\n' "$RUNTIME_DIR"
    printf 'child_dbus=unix:path=%s/bus\n' "$RUNTIME_DIR"
    printf 'child_nix_state=default (/nix/var/nix; fallback %s checked absent)\n' "/nix/var/nix/profiles/per-user/$E2E_USER"
    printf 'child_nix_remote=daemon\n'
    printf 'child_nix_path=nixpkgs=%s\n' "$NP"
}

# Remove the sandbox only after re-proving it is the exact directory this
# process created under /var/tmp, owned by this user, and not the real HOME.
root_is_removable() {
    [ -n "$ROOT" ] || return 1
    [ -d "$ROOT" ] || return 1
    [ ! -L "$ROOT" ] || return 1
    resolved=$(readlink -f "$ROOT" 2>/dev/null) || return 1
    [ "$resolved" = "$ROOT" ] || return 1
    case "$resolved" in
        "$TMP_ROOT_BASE"/nhs-e2e.??????) : ;;
        *) return 1 ;;
    esac
    [ "$(stat -c %u "$resolved" 2>/dev/null)" = "$(id -u)" ] || return 1
    if [ -n "$REAL_HOME" ]; then
        [ "$resolved" != "$REAL_HOME" ] || return 1
        case "$resolved" in
            "$REAL_HOME"/*) return 1 ;;
        esac
        case "$REAL_HOME" in
            "$resolved"/*) return 1 ;;
        esac
    fi
    return 0
}

# Signal TERM, then KILL, the whole process group. Returns 1 if members remain.
terminate_group() {
    group=$1
    [ -n "$group" ] || return 0
    kill -TERM -"$group" 2>/dev/null || true
    attempt=0
    while kill -0 -"$group" 2>/dev/null && [ "$attempt" -lt $((KILL_GRACE * 10)) ]; do
        sleep 0.1
        attempt=$((attempt + 1))
    done
    if kill -0 -"$group" 2>/dev/null; then
        kill -KILL -"$group" 2>/dev/null || true
        attempt=0
        while kill -0 -"$group" 2>/dev/null && [ "$attempt" -lt 50 ]; do
            sleep 0.1
            attempt=$((attempt + 1))
        done
    fi
    ! kill -0 -"$group" 2>/dev/null
}

terminate_active_child() {
    # If a signal arrives before the child wrote its pid file, give it a short
    # window. Refusing to delete is safer than deleting under an untracked child.
    if [ -z "$ACTIVE_GROUP" ] && [ -n "$ACTIVE_CHILD" ] && [ -n "$ACTIVE_PIDFILE" ]; then
        attempt=0
        while [ ! -s "$ACTIVE_PIDFILE" ] && kill -0 "$ACTIVE_CHILD" 2>/dev/null && [ "$attempt" -lt 100 ]; do
            sleep 0.01
            attempt=$((attempt + 1))
        done
        if [ -s "$ACTIVE_PIDFILE" ]; then
            ACTIVE_GROUP=$(cat "$ACTIVE_PIDFILE")
        fi
    fi
    if [ -z "$ACTIVE_GROUP" ] && [ -n "$ACTIVE_CHILD" ]; then
        kill -TERM "$ACTIVE_CHILD" 2>/dev/null || true
        wait "$ACTIVE_CHILD" 2>/dev/null || true
        ACTIVE_CHILD=
        return 1
    fi
    result=0
    if [ -n "$ACTIVE_GROUP" ]; then
        terminate_group "$ACTIVE_GROUP" || result=1
    fi
    if [ -n "$ACTIVE_CHILD" ]; then
        kill -TERM "$ACTIVE_CHILD" 2>/dev/null || true
        wait "$ACTIVE_CHILD" 2>/dev/null || true
    fi
    ACTIVE_CHILD=
    ACTIVE_GROUP=
    ACTIVE_PIDFILE=
    return "$result"
}

stop_watchdog() {
    [ -n "$WATCHDOG" ] || return 0
    if [ -n "$WATCHDOG_GROUP" ]; then
        terminate_group "$WATCHDOG_GROUP" || true
    fi
    # Fallback for the short window before the watchdog wrote its pid file.
    children=$(ps -o pid= --ppid "$WATCHDOG" 2>/dev/null | tr -d ' ')
    kill "$WATCHDOG" 2>/dev/null || true
    if [ -n "$children" ]; then
        for child in $children; do
            kill "$child" 2>/dev/null || true
        done
    fi
    wait "$WATCHDOG" 2>/dev/null || true
    WATCHDOG=
    WATCHDOG_GROUP=
    WATCHDOG_PIDFILE=
}

# The watchdog is a separate session too, so stopping it cannot leave a `sleep`
# child behind (which would otherwise keep a workspace open).
start_watchdog() {
    log=$1
    timeout_seconds=$2
    timeout_flag=$3
    group=$4
    WATCHDOG_PIDFILE="$log.watchdog.pid"
    rm -f "$WATCHDOG_PIDFILE"
    setsid -f -w sh -c 'printf "%s\n" "$$" > "$1"; shift; exec "$@"' sh \
        "$WATCHDOG_PIDFILE" sh -c \
        'sleep "$1"; : > "$2"; kill -TERM -"$3" 2>/dev/null || true; sleep "$4"; kill -KILL -"$3" 2>/dev/null || true' \
        sh "$timeout_seconds" "$timeout_flag" "$group" "$KILL_GRACE" \
        >"$log.watchdog" 2>&1 &
    WATCHDOG=$!
    WATCHDOG_GROUP=
    attempt=0
    while [ ! -s "$WATCHDOG_PIDFILE" ] && [ "$attempt" -lt 100 ]; do
        if ! kill -0 "$WATCHDOG" 2>/dev/null; then
            break
        fi
        sleep 0.01
        attempt=$((attempt + 1))
    done
    if [ -s "$WATCHDOG_PIDFILE" ]; then
        WATCHDOG_GROUP=$(cat "$WATCHDOG_PIDFILE")
    else
        stop_watchdog
        fail "could not start the timeout watchdog"
    fi
}

# Start a command as the leader of a new session/process group. `setsid -f`
# always forks, so the child writes its own pid before exec; that pid is the
# process-group id used for termination.
start_process_group() {
    log=$1
    shift
    ACTIVE_PIDFILE="$log.pid"
    rm -f "$ACTIVE_PIDFILE"
    setsid -f -w sh -c 'printf "%s\n" "$$" > "$1"; shift; exec "$@"' \
        sh "$ACTIVE_PIDFILE" "$@" >"$log" 2>&1 &
    ACTIVE_CHILD=$!
    ACTIVE_GROUP=
    attempt=0
    while [ ! -s "$ACTIVE_PIDFILE" ] && [ "$attempt" -lt 500 ]; do
        if ! kill -0 "$ACTIVE_CHILD" 2>/dev/null; then
            break
        fi
        sleep 0.01
        attempt=$((attempt + 1))
    done
    [ -s "$ACTIVE_PIDFILE" ] ||
        fail "could not determine the process group for $log"
    ACTIVE_GROUP=$(cat "$ACTIVE_PIDFILE")
    pgid=$(ps -o pgid= -p "$ACTIVE_GROUP" 2>/dev/null | tr -d ' ')
    if [ "$pgid" != "$ACTIVE_GROUP" ]; then
        # A very short command can be gone before ps runs. Only accept that
        # when the whole group is gone as well.
        if kill -0 -"$ACTIVE_GROUP" 2>/dev/null; then
            fail "child is not a process-group leader (pid=$ACTIVE_GROUP pgid=${pgid:-none})"
        fi
    fi
}

# Run a command in a new session/process group. On timeout or signal, the whole
# group is stopped before the caller removes the workspace.
run_with_timeout() {
    log=$1
    timeout_seconds=$2
    shift 2
    require_integer "$timeout_seconds" "timeout" 1 3600
    timeout_flag="$log.timeout"
    rm -f "$timeout_flag"
    start_process_group "$log" "$@"
    start_watchdog "$log" "$timeout_seconds" "$timeout_flag" "$ACTIVE_GROUP"
    status=0
    wait "$ACTIVE_CHILD" || status=$?
    stop_watchdog
    # A command can exit while leaving children behind. Stop the remainder and
    # treat that as a failure instead of deleting a directory still in use.
    if kill -0 -"$ACTIVE_GROUP" 2>/dev/null; then
        printf '%s\n' "E2E: process group $ACTIVE_GROUP still had members after exit" >>"$log"
        if ! terminate_group "$ACTIVE_GROUP"; then
            fail "could not stop leftover process group $ACTIVE_GROUP"
        fi
        status=1
    fi
    rm -f "$ACTIVE_PIDFILE"
    ACTIVE_CHILD=
    ACTIVE_GROUP=
    ACTIVE_PIDFILE=
    if [ -e "$timeout_flag" ]; then
        status=124
    fi
    return "$status"
}

cleanup() {
    status=$1
    trap - EXIT HUP INT TERM
    stop_watchdog
    if ! terminate_active_child; then
        echo "E2E FAIL: process group still alive; keeping $ROOT for inspection" >&2
        exit 1
    fi
    if [ "${KEEP_E2E:-0}" = 1 ] || [ "${PRESERVE_SANDBOX:-0}" = 1 ]; then
        echo "E2E kept: $ROOT" >&2
        exit "$status"
    fi
    if [ -n "$ROOT" ] && [ -d "$ROOT" ]; then
        if ! root_is_removable; then
            echo "E2E FAIL: refusing to remove a sandbox that no longer matches the created path: $ROOT" >&2
            exit 1
        fi
        rm -rf -- "$ROOT"
    fi
    exit "$status"
}

write_config() {
    packages=$1
    mkdir -p "$CONFIG_DIR/home-manager"
    cat >"$CONFIG_FILE" <<EOF
{ pkgs, lib, ... }:
{
  home.username = "$E2E_USER";
  home.homeDirectory = "$HOME_DIR";
  home.stateVersion = "24.05";
  home.packages = [ $packages ];

  # No private user systemd is available in this sandbox. Disable service
  # starting and remove the reload activation entry so activation cannot reach
  # the real user manager even if a bus were present by accident.
  systemd.user.startServices = false;
  home.activation.reloadSystemd = lib.mkForce "";
}
EOF
}

run_hm() {
    log_name=$1
    shift
    check_isolation
    check_service_isolation
    status=0
    run_with_timeout "$ROOT/$log_name.log" "$E2E_TIMEOUT" \
        sh -c 'cd "$1"; shift; exec "$@"' sh "$HOME_DIR" \
        env -i \
        HOME="$HOME_DIR" \
        USER="$E2E_USER" \
        LOGNAME="$E2E_USER" \
        XDG_DATA_HOME="$DATA_DIR" \
        XDG_STATE_HOME="$STATE_DIR" \
        XDG_CONFIG_HOME="$CONFIG_DIR" \
        XDG_CACHE_HOME="$CACHE_DIR" \
        XDG_RUNTIME_DIR="$RUNTIME_DIR" \
        DBUS_SESSION_BUS_ADDRESS="unix:path=$RUNTIME_DIR/bus" \
        TMPDIR="$TMP_DIR" \
        TMP="$TMP_DIR" \
        TEMP="$TMP_DIR" \
        NIX_REMOTE=daemon \
        NIX_PATH="nixpkgs=$NP" \
        PATH="$E2E_PATH" \
        "$HM" "$@" || status=$?
    # This must run for both success and failure. A fallback directory means
    # Home Manager may have observed real per-user state; stop the sequence,
    # keep the sandbox, and do not attempt a later rollback.
    if ! check_hm_fallback_postcondition; then
        fail "Home Manager fallback directory appeared after $log_name; sequence stopped"
    fi
    if [ "$status" -ne 0 ]; then
        echo "E2E FAIL: home-manager $log_name failed with status $status" >&2
        if [ -f "$ROOT/$log_name.log" ]; then
            echo "--- $ROOT/$log_name.log (tail) ---" >&2
            tail -40 "$ROOT/$log_name.log" >&2 || true
        fi
        return "$status"
    fi
}

# Every generation/GC root must be addressed through the sandbox. A root is a
# symlink whose *location* is inside the sandbox and whose target is normally a
# /nix/store path, so only the link's parent is resolved here.
check_gc_roots() {
    for path in \
        "$RESULT_1" \
        "$RESULT_2" \
        "$PROFILES/home-manager" \
        "$STATE_DIR/home-manager/gcroots"
    do
        [ -e "$path" ] || [ -L "$path" ] || continue
        parent=$(dirname "$path")
        resolved_parent=$(readlink -f "$parent") || fail "cannot resolve GC root parent: $parent"
        [ "$resolved_parent" = "$parent" ] ||
            fail "GC root parent is not canonical: $parent -> $resolved_parent"
        is_under "$resolved_parent" "$ROOT" ||
            fail "GC root is outside the sandbox: $path"
        if [ -d "$path" ] && [ ! -L "$path" ]; then
            resolved=$(readlink -f "$path") || fail "cannot resolve GC root directory: $path"
            [ "$resolved" = "$path" ] ||
                fail "GC root directory is not canonical: $path -> $resolved"
            is_under "$resolved" "$ROOT" ||
                fail "GC root directory is outside the sandbox: $path"
        fi
    done
    for work in "$TMP_DIR"/home-manager-build.*; do
        [ -e "$work" ] || continue
        resolved=$(readlink -f "$work") || fail "cannot resolve work directory: $work"
        is_under "$resolved" "$ROOT" ||
            fail "home-manager work directory is outside the sandbox: $work -> $resolved"
    done
    # The profile directory itself must be a real sandbox directory, not a
    # symlink that Home Manager or nix-env could follow elsewhere.
    [ ! -L "$PROFILES" ] || fail "profile directory became a symlink: $PROFILES"
    [ "$(readlink -f "$PROFILES")" = "$PROFILES" ] ||
        fail "profile directory is not canonical: $PROFILES"
}

require_tools() {
    for tool in setsid ps mktemp readlink stat chmod mkdir rm kill sleep find sort cat sha256sum awk tail id dirname tr; do
        require_tool "$tool"
    done
    setsid -f -w sh -c 'exit 0' >/dev/null 2>&1 ||
        fail "setsid does not support -f and -w"
    command -v nix >/dev/null 2>&1 || fail "nix is not available"
    for tool in nix nix-build nix-env nix-store; do
        path=$(command -v "$tool" 2>/dev/null) || fail "required Nix tool not found: $tool"
        resolved=$(readlink -f "$path") || fail "cannot resolve $tool: $path"
        [ -x "$resolved" ] || fail "$tool is not executable: $resolved"
    done
    NIX_BIN=$(dirname "$(readlink -f "$(command -v nix)")")
    E2E_PATH="$NIX_BIN:/usr/bin:/bin"
}

main() {
    MODE=preflight
    for argument in "$@"; do
        case "$argument" in
            --preflight|--check) MODE=preflight ;;
            --execute) MODE=execute ;;
            --help|-h) usage; exit 0 ;;
            *) usage >&2; fail "unknown argument: $argument" ;;
        esac
    done

    require_integer "$E2E_TIMEOUT" "NHS_E2E_TIMEOUT" 1 3600
    require_integer "$KILL_GRACE" "NHS_E2E_KILL_GRACE" 1 60
    require_tools
    resolve_real_home
    collect_caller_environment
    validate_home_manager "${NHS_E2E_HM:-}"
    validate_nixpkgs "${NHS_E2E_NIXPKGS:-}"

    prepare_root
    # shellcheck disable=SC2064
    trap 'cleanup $?' EXIT
    trap 'cleanup 129' HUP
    trap 'cleanup 130' INT
    trap 'cleanup 143' TERM

    assign_layout
    prepare_directories
    check_isolation
    check_service_isolation

    echo "mode=$MODE"
    echo "root=$ROOT"
    echo "home=$HOME_DIR"
    echo "state=$STATE_DIR"
    echo "profiles=$PROFILES"
    echo "home_manager=$HM"
    echo "nixpkgs=$NP"
    echo "real_home=$REAL_HOME"
    echo "caller_env_ignored=$CALLER_IGNORED"
    print_child_environment
    print_real_label real_profile_before real_profile_label
    print_real_digest real_config_before "$REAL_HOME/.config/home-manager"
    print_real_digest real_app_state_before "$REAL_HOME/.local/state/nix-home-studio"
    print_real_digest real_hm_data_before "$REAL_HOME/.local/share/home-manager"
    print_real_digest real_hm_cache_before "$REAL_HOME/.cache/home-manager"
    print_real_label e2e_fallback_profile e2e_fallback_label
    echo "service_activation=disabled"
    check_isolation
    check_service_isolation

    if [ "$MODE" = preflight ]; then
        echo "preflight=ok; no home-manager switch or rollback was run"
        exit 0
    fi

    require_activation_authorization
    if ! snapshot_before=$(snapshot_real_environment); then
        fail "cannot snapshot the real environment before activation; refusing to continue"
    fi

    # Generation 1: baseline.
    write_config ""
    run_hm build-1 build -f "$CONFIG_FILE"
    run_hm switch-1 switch -f "$CONFIG_FILE"
    check_gc_roots

    # Generation 2: install hello.
    write_config "pkgs.hello"
    run_hm build-2 build -f "$CONFIG_FILE"
    run_hm switch-2 switch -f "$CONFIG_FILE"
    profile="$PROFILES/home-manager"
    [ -L "$profile" ] || fail "profile link not created: $profile"
    # shellcheck disable=SC2012
    [ "$(readlink -f "$profile")" != "$(readlink -f "$REAL_HOME/.local/state/nix/profiles/home-manager" 2>/dev/null || true)" ] ||
        fail "sandbox profile resolves to the real profile"
    test -x "$profile/home-path/bin/hello" || fail "hello is not installed in generation 2"
    check_gc_roots

    # Roll back to generation 1 and verify removal.
    run_hm rollback switch --rollback
    test ! -e "$profile/home-path/bin/hello" || fail "hello survived the rollback"
    check_gc_roots

    current=$(readlink "$profile")
    echo "rollback_current=$current"

    if ! snapshot_after=$(snapshot_real_environment); then
        fail "cannot snapshot the real environment after activation; refusing to compare"
    fi
    [ "$snapshot_before" = "$snapshot_after" ] ||
        fail "real Home Manager environment changed during the E2E"
    echo "real_environment_unchanged=yes"
    echo "e2e=done"
}

# Sourced by scripts/test-e2e-isolation.sh; the harness itself never sets this.
if [ "${NHS_E2E_LIB_ONLY:-0}" != 1 ]; then
    main "$@"
fi
