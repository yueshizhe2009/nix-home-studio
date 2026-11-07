#!/bin/sh
# Isolated Home Manager E2E harness.
#
# Default mode is PREFLIGHT ONLY: it creates a temporary HOME/XDG tree, checks
# that every write target is inside it, prints the plan, and exits. It does not
# run `home-manager switch`.
#
# Activation is opt-in and must be explicitly authorised:
#
#   NHS_E2E_ALLOW_ACTIVATION=1 sh scripts/e2e-home-manager.sh --execute
#
# Even then the script re-checks isolation before every switch and never touches
# the caller's HOME, profile, or configuration.
set -eu

usage() {
    cat <<'EOF'
usage: scripts/e2e-home-manager.sh [--preflight|--execute]

  --preflight  (default) create the sandbox and verify isolation; no activation
  --execute    run build/switch/rollback; requires NHS_E2E_ALLOW_ACTIVATION=1

Set NHS_E2E_HM and NHS_E2E_NIXPKGS when the store paths are not discoverable
from the caller's PATH/NIX_PATH. The script never falls back to a bare PATH
executable for activation.
EOF
}

fail() {
    echo "E2E FAIL: $*" >&2
    exit 1
}

MODE=preflight
for argument in "$@"; do
    case "$argument" in
        --preflight|--check) MODE=preflight ;;
        --execute) MODE=execute ;;
        --help|-h) usage; exit 0 ;;
        *) usage >&2; fail "unknown argument: $argument" ;;
    esac
done

REAL_HOME=$(readlink -f "${HOME:?HOME is not set}")

# Resolve an absolute store binary; never trust a bare PATH lookup for execution.
HM=${NHS_E2E_HM:-$(command -v home-manager 2>/dev/null || true)}
[ -n "$HM" ] || fail "home-manager not found; set NHS_E2E_HM to the absolute store path"
HM=$(readlink -f "$HM")
case "$HM" in
    /nix/store/*/bin/home-manager) : ;;
    *) fail "home-manager does not resolve to an absolute store path: $HM" ;;
esac
[ -x "$HM" ] || fail "home-manager is not executable: $HM"

NP=${NHS_E2E_NIXPKGS:-}
if [ -z "$NP" ]; then
    NP=$(nix-instantiate --find-file nixpkgs 2>/dev/null || true)
fi
[ -n "$NP" ] && [ -d "$NP" ] || fail "nixpkgs source not found; set NHS_E2E_NIXPKGS"

ROOT=$(mktemp -d "${TMPDIR:-/var/tmp}/nhs-e2e.XXXXXX")
ROOT=$(readlink -f "$ROOT")
HOME_DIR="$ROOT/home"
STATE_DIR="$ROOT/state"
CONFIG_DIR="$ROOT/config"
CACHE_DIR="$ROOT/cache"
PROFILES="$STATE_DIR/nix/profiles"
CONFIG_FILE="$CONFIG_DIR/home-manager/home.nix"
E2E_USER=nhs-e2e
mkdir -p "$HOME_DIR" "$STATE_DIR" "$CONFIG_DIR" "$CACHE_DIR"

cleanup() {
    status=$?
    if [ "${KEEP_E2E:-0}" = 1 ]; then
        echo "E2E kept: $ROOT" >&2
    else
        rm -rf "$ROOT"
    fi
    exit "$status"
}
trap cleanup EXIT HUP INT TERM

check_isolation() {
    [ -d "$ROOT" ] || fail "sandbox disappeared: $ROOT"
    case "$ROOT" in
        /var/tmp/*|/tmp/*) : ;;
        *) fail "sandbox is not under a temporary directory: $ROOT" ;;
    esac
    [ "$ROOT" != "$REAL_HOME" ] || fail "sandbox is the real HOME"
    case "$ROOT" in
        "$REAL_HOME"/*) fail "sandbox is inside the real HOME" ;;
    esac

    for directory in "$HOME_DIR" "$STATE_DIR" "$CONFIG_DIR" "$CACHE_DIR"; do
        resolved=$(readlink -f "$directory")
        case "$resolved" in
            "$ROOT"/*) : ;;
            *) fail "$directory resolves outside the sandbox: $resolved" ;;
        esac
    done
    case "$PROFILES" in
        "$ROOT"/*) : ;;
        *) fail "profile directory is outside the sandbox: $PROFILES" ;;
    esac

    for variable in NIX_PROFILES NIX_USER_PROFILE_DIR NIX_STATE_DIR; do
        eval "value=\${$variable-}"
        [ -z "$value" ] || fail "$variable is set ($value); refusing to continue"
    done

    [ -x "$HM" ] || fail "home-manager is not executable: $HM"
}

write_config() {
    packages=$1
    mkdir -p "$CONFIG_DIR/home-manager"
    cat > "$CONFIG_FILE" <<EOF
{ pkgs, ... }:
{
  home.username = "$E2E_USER";
  home.homeDirectory = "$HOME_DIR";
  home.stateVersion = "24.05";
  home.packages = [ $packages ];
}
EOF
}

run_hm() {
    log=$1
    shift
    check_isolation
    env -u NIX_PROFILES -u NIX_USER_PROFILE_DIR -u NIX_STATE_DIR \
        HOME="$HOME_DIR" USER="$E2E_USER" LOGNAME="$E2E_USER" \
        XDG_STATE_HOME="$STATE_DIR" \
        XDG_CONFIG_HOME="$CONFIG_DIR" \
        XDG_CACHE_HOME="$CACHE_DIR" \
        NIX_PATH="nixpkgs=$NP" \
        PATH=/usr/bin:/bin \
        timeout 600 "$HM" "$@" > "$ROOT/$log.log" 2>&1
}

echo "mode=$MODE"
echo "root=$ROOT"
echo "home=$HOME_DIR"
echo "state=$STATE_DIR"
echo "profiles=$PROFILES"
echo "home_manager=$HM"
echo "nixpkgs=$NP"
echo "real_home=$REAL_HOME"
check_isolation

if [ "$MODE" = preflight ]; then
    echo "preflight=ok; no home-manager switch or rollback was run"
    exit 0
fi

[ "${NHS_E2E_ALLOW_ACTIVATION:-0}" = 1 ] || fail \
    "activation is not authorised; set NHS_E2E_ALLOW_ACTIVATION=1 explicitly"

real_profile_before=$(readlink "$REAL_HOME/.local/state/nix/profiles/home-manager" 2>/dev/null || true)

# Generation 1: baseline.
write_config ""
run_hm build-1 build -f "$CONFIG_FILE" -o "$ROOT/result-1"
run_hm switch-1 switch -f "$CONFIG_FILE"

# Generation 2: install hello.
write_config "pkgs.hello"
run_hm build-2 build -f "$CONFIG_FILE" -o "$ROOT/result-2"
run_hm switch-2 switch -f "$CONFIG_FILE"
profile="$PROFILES/home-manager"
[ -L "$profile" ] || fail "profile link not created: $profile"
test -x "$profile/home-path/bin/hello" || fail "hello is not installed in generation 2"

# Roll back to generation 1 and verify removal.
run_hm rollback switch --rollback
test ! -e "$profile/home-path/bin/hello" || fail "hello survived the rollback"

current=$(readlink "$profile")
echo "rollback_current=$current"

real_profile_after=$(readlink "$REAL_HOME/.local/state/nix/profiles/home-manager" 2>/dev/null || true)
[ "$real_profile_before" = "$real_profile_after" ] || fail "real Home Manager profile changed"
echo "real_profile_unchanged=yes"
echo "e2e=done"
