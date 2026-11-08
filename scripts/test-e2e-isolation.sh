#!/bin/sh
# Automated isolation tests for scripts/e2e-home-manager.sh.
#
# These tests never run `home-manager switch` or rollback. Unit cases source the
# harness in library mode; the integration cases run `--preflight` only.
#
# Run from anywhere:
#
#   sh scripts/test-e2e-isolation.sh
#
# Integration cases are skipped when no /nix/store home-manager and nixpkgs
# source are available (for example on a Rust-only CI runner).

# The sourced harness uses several of these variables; shellcheck only sees the
# assignments here.
# shellcheck disable=SC2034
# Single-quoted child snippets expand inside the child, not in the parent.
# shellcheck disable=SC2016

set -u

REPO=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
LIB="$REPO/scripts/e2e-home-manager.sh"

PASS=0
FAILED=0
SKIPPED=0
TRACK_FILE=$(mktemp /var/tmp/nhs-isolation-track.XXXXXX) || exit 1

ok() {
    PASS=$((PASS + 1))
    printf 'ok - %s\n' "$1"
}

bad() {
    FAILED=$((FAILED + 1))
    printf 'not ok - %s\n' "$1"
}

skip() {
    SKIPPED=$((SKIPPED + 1))
    printf 'skip - %s\n' "$1"
}

track_root() {
    printf '%s\n' "$1" >>"$TRACK_FILE"
}

cleanup_tests() {
    if [ -f "$TRACK_FILE" ]; then
        while IFS= read -r tracked; do
            [ -n "$tracked" ] || continue
            rm -rf -- "$tracked"
        done <"$TRACK_FILE"
        rm -f "$TRACK_FILE"
    fi
}
trap cleanup_tests EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

expect_ok() {
    name=$1
    shift
    output=$( ( "$@" ) 2>&1 )
    status=$?
    if [ "$status" -eq 0 ]; then
        ok "$name"
    else
        bad "$name (status $status)"
        printf '%s\n' "$output" | sed 's/^/    /'
    fi
}

expect_fail() {
    name=$1
    pattern=$2
    shift 2
    output=$( ( "$@" ) 2>&1 )
    status=$?
    if [ "$status" -eq 0 ]; then
        bad "$name (expected a failure)"
        printf '%s\n' "$output" | sed 's/^/    /'
    elif [ -n "$pattern" ] && ! printf '%s' "$output" | grep -Fq -- "$pattern"; then
        bad "$name (failure did not mention '$pattern')"
        printf '%s\n' "$output" | sed 's/^/    /'
    else
        ok "$name"
    fi
}

expect_status() {
    name=$1
    expected=$2
    shift 2
    output=$( ( "$@" ) 2>&1 )
    status=$?
    if [ "$status" -eq "$expected" ]; then
        ok "$name"
    else
        bad "$name (status $status, expected $expected)"
        printf '%s\n' "$output" | sed 's/^/    /'
    fi
}

# --- fixtures -------------------------------------------------------------

sh -n "$LIB" || { echo "harness does not parse" >&2; exit 1; }

NHS_E2E_LIB_ONLY=1
export NHS_E2E_LIB_ONLY
# shellcheck disable=SC1090
. "$LIB"

# Tests capture statuses explicitly; `fail` exits only the case subshell.
set +e

KILL_GRACE=1

FAKE_HOME=$(mktemp -d /var/tmp/nhs-real-home.XXXXXX) || exit 1
chmod 700 "$FAKE_HOME"
track_root "$FAKE_HOME"
mkdir -p "$FAKE_HOME/.config/home-manager" "$FAKE_HOME/.local/state/nix-home-studio"
printf 'initial\n' >"$FAKE_HOME/.config/home-manager/home.nix"
printf 'state\n' >"$FAKE_HOME/.local/state/nix-home-studio/state.json"

make_root() {
    ROOT_TO_USE=$(mktemp -d /var/tmp/nhs-e2e.XXXXXX) || exit 1
    chmod 700 "$ROOT_TO_USE"
    track_root "$ROOT_TO_USE"
}

make_outside() {
    OUTSIDE=$(mktemp -d /var/tmp/nhs-outside.XXXXXX) || exit 1
    track_root "$OUTSIDE"
}

setup_case() {
    ROOT=$1
    REAL_HOME=$FAKE_HOME
    assign_layout
    prepare_directories
}

fresh_root() {
    make_root
    printf '%s\n' "$ROOT_TO_USE"
}

# Fixtures that only need to satisfy the presence/executable checks.
HM=/bin/sh
NP="$FAKE_HOME/nixpkgs-fixture"
mkdir -p "$NP"
: >"$NP/default.nix"

# --- store path validation ------------------------------------------------

case_bad_nixpkgs() { validate_nixpkgs /tmp; }
case_bad_home_manager() { validate_home_manager /tmp/not-a-home-manager; }
case_bad_home_manager_store() { validate_home_manager /nix/store/not-a-real-home-manager/bin/home-manager; }

expect_fail "nixpkgs outside /nix/store is rejected" "does not resolve into /nix/store" case_bad_nixpkgs
expect_fail "non-store home-manager is rejected" "/nix/store" case_bad_home_manager
expect_fail "missing store home-manager is rejected" "home-manager" case_bad_home_manager_store

# --- caller environment ---------------------------------------------------

case_caller_default() {
    unset NIX_PROFILES NIX_REMOTE NIX_CONFIG HOME_MANAGER_CONFIG || true
    NIX_REMOTE=ssh://blocked
    collect_caller_environment
    case "$CALLER_IGNORED" in
        *NIX_REMOTE*) return 0 ;;
        *) return 1 ;;
    esac
}

case_caller_strict() {
    unset NIX_PROFILES NIX_USER_PROFILE_DIR NIX_STATE_DIR NIX_CONFIG NIX_CONF_DIR \
        NIX_USER_CONF_FILES NIX_STORE_DIR NIX_STORE NIX_DAEMON_SOCKET_PATH \
        HOME_MANAGER_CONFIG HOME_MANAGER_BACKUP_EXT || true
    NIX_REMOTE=ssh://blocked
    NHS_E2E_STRICT_ENV=1
    collect_caller_environment
}

case_activation_missing() {
    unset NHS_E2E_ALLOW_ACTIVATION || true
    require_activation_authorization
}

case_activation_present() {
    NHS_E2E_ALLOW_ACTIVATION=1
    require_activation_authorization
}

expect_ok "caller NIX_REMOTE is cleared and reported by default" case_caller_default
expect_fail "strict mode rejects NIX_REMOTE" "NIX_REMOTE" case_caller_strict
expect_fail "activation requires explicit authorization" "not authorised" case_activation_missing
expect_ok "activation authorization accepts the explicit flag" case_activation_present

# --- sandbox layout and isolation ----------------------------------------

case_isolation_ok() { setup_case "$1"; check_isolation; check_service_isolation; }
case_root_outside() {
    make_outside
    ROOT=$OUTSIDE
    REAL_HOME=$FAKE_HOME
    assign_layout
    check_isolation
}
case_root_is_real_home() {
    ROOT=$1
    REAL_HOME=$ROOT
    assign_layout
    check_isolation
}
case_root_inside_real_home() {
    ROOT=$1
    REAL_HOME=/var/tmp
    assign_layout
    check_isolation
}
case_missing_profiles() {
    setup_case "$1"
    rmdir "$PROFILES"
    check_isolation
}
case_symlink_cache() {
    setup_case "$1"
    make_outside
    rmdir "$CACHE_DIR"
    ln -s "$OUTSIDE" "$CACHE_DIR"
    check_isolation
}
case_symlink_profiles() {
    setup_case "$1"
    make_outside
    rmdir "$PROFILES"
    ln -s "$OUTSIDE" "$PROFILES"
    check_isolation
}
case_bad_mode() {
    setup_case "$1"
    chmod 755 "$ROOT"
    check_isolation
}
case_bus_socket() {
    setup_case "$1"
    : >"$RUNTIME_DIR/bus"
    check_service_isolation
}
case_runtime_mode() {
    setup_case "$1"
    chmod 755 "$RUNTIME_DIR"
    check_service_isolation
}

make_root
ROOT_OK=$ROOT_TO_USE
expect_ok "valid sandbox passes isolation checks" case_isolation_ok "$ROOT_OK"

case_fallback_absent() {
    make_outside
    hm_fallback_absent "$OUTSIDE/missing"
}
case_fallback_present() {
    make_outside
    : >"$OUTSIDE/present"
    hm_fallback_absent "$OUTSIDE/present"
}
expect_ok "absent Home Manager fallback path is accepted" case_fallback_absent
expect_fail "existing Home Manager fallback path is rejected" "" case_fallback_present

# --- postcondition after a child command ---------------------------------

case_postcondition_detects() {
    setup_case "$1"
    fallback="$TMP_DIR/fake-fallback"
    run_with_timeout "$TMP_DIR/post.log" 5 sh -c 'mkdir -p "$1"' sh "$fallback"
    check_hm_fallback_postcondition "$fallback"
}
case_postcondition_blocks() {
    setup_case "$1"
    fallback="$TMP_DIR/fake-fallback"
    run_with_timeout "$TMP_DIR/post.log" 5 sh -c 'mkdir -p "$1"' sh "$fallback"
    if check_hm_fallback_postcondition "$fallback"; then
        : >"$ROOT/continued"
    fi
    [ ! -e "$ROOT/continued" ]
}
case_postcondition_preserves() {
    setup_case "$1"
    fallback="$TMP_DIR/fake-fallback"
    mkdir -p "$fallback"
    check_hm_fallback_postcondition "$fallback" || true
    cleanup 0
}

expect_fail "postcondition detects a fallback created during the child run" "not provably absent" case_postcondition_detects "$(fresh_root)"
expect_ok "postcondition prevents the next step from running" case_postcondition_blocks "$(fresh_root)"
make_root
ROOT_PRESERVED=$ROOT_TO_USE
expect_ok "postcondition keeps the sandbox instead of cleaning it" case_postcondition_preserves "$ROOT_PRESERVED" "$ROOT_PRESERVED"
if [ -d "$ROOT_PRESERVED" ]; then
    ok "preserved sandbox still exists after cleanup"
else
    bad "preserved sandbox was removed"
fi

# --- cleanup and real-HOME boundary --------------------------------------

case_removable_inside_home() {
    ROOT=$1
    REAL_HOME=/var/tmp
    root_is_removable
}
case_cleanup_inside_home() {
    ROOT=$1
    REAL_HOME=/var/tmp
    KEEP_E2E=0
    PRESERVE_SANDBOX=0
    cleanup 0
}
expect_fail "sandbox inside real HOME is not removable" "" case_removable_inside_home "$(fresh_root)"
make_root
ROOT_INSIDE_HOME=$ROOT_TO_USE
expect_fail "cleanup refuses a sandbox inside real HOME" "refusing to remove" case_cleanup_inside_home "$ROOT_INSIDE_HOME"
if [ -d "$ROOT_INSIDE_HOME" ]; then
    ok "cleanup left the sandbox inside real HOME untouched"
else
    bad "cleanup removed a sandbox inside real HOME"
fi
expect_fail "sandbox outside the temporary namespace is rejected" "not under" case_root_outside
expect_fail "sandbox equal to real HOME is rejected" "real HOME" case_root_is_real_home "$(fresh_root)"
expect_fail "sandbox inside real HOME is rejected" "inside the real HOME" case_root_inside_real_home "$(fresh_root)"
expect_fail "missing profile directory is rejected" "missing" case_missing_profiles "$(fresh_root)"
expect_fail "symlinked XDG cache directory is rejected" "symlink" case_symlink_cache "$(fresh_root)"
expect_fail "symlinked profile directory is rejected" "symlink" case_symlink_profiles "$(fresh_root)"
expect_fail "sandbox mode other than 700 is rejected" "mode is not 700" case_bad_mode "$(fresh_root)"
expect_fail "bus socket in the sandbox runtime is rejected" "bus socket" case_bus_socket "$(fresh_root)"
expect_fail "runtime directory mode other than 700 is rejected" "mode is not 700" case_runtime_mode "$(fresh_root)"

# --- child environment ----------------------------------------------------

case_child_env() {
    setup_case "$1"
    NP=/nix/store/fixture-source
    child=$(print_child_environment)
    case "$child" in
        *"child_home=$HOME_DIR"*) : ;;
        *) return 1 ;;
    esac
    case "$child" in
        *"child_xdg_data=$DATA_DIR"*) : ;;
        *) return 1 ;;
    esac
    case "$child" in
        *"child_xdg_state=$STATE_DIR"*) : ;;
        *) return 1 ;;
    esac
    case "$child" in
        *"child_tmpdir=$TMP_DIR"*) : ;;
        *) return 1 ;;
    esac
    case "$child" in
        *"child_runtime=$RUNTIME_DIR"*) : ;;
        *) return 1 ;;
    esac
    case "$child" in
        *"child_dbus=unix:path=$RUNTIME_DIR/bus"*) : ;;
        *) return 1 ;;
    esac
    case "$child" in
        *"child_nix_state=default"*) : ;;
        *) return 1 ;;
    esac
    case "$child" in
        *"child_nix_remote=daemon"*) : ;;
        *) return 1 ;;
    esac
    case "$child" in
        *"child_nix_path=nixpkgs=$NP"*) : ;;
        *) return 1 ;;
    esac
    return 0
}

expect_ok "child environment uses only sandbox paths" case_child_env "$ROOT_OK"

# --- cleanup safety -------------------------------------------------------

case_removable_ok() {
    ROOT=$1
    REAL_HOME=$FAKE_HOME
    root_is_removable
}
case_removable_symlink() {
    make_outside
    linkroot="$OUTSIDE/linkroot"
    rm -rf -- "$linkroot"
    ln -s "$OUTSIDE" "$linkroot"
    ROOT=$linkroot
    REAL_HOME=$FAKE_HOME
    root_is_removable
}
case_cleanup_symlink() {
    make_outside
    linkroot="$OUTSIDE/linkroot"
    rm -rf -- "$linkroot"
    ln -s "$OUTSIDE" "$linkroot"
    ROOT=$linkroot
    REAL_HOME=$FAKE_HOME
    KEEP_E2E=0
    cleanup 0
}
case_cleanup_ok() {
    ROOT=$1
    REAL_HOME=$FAKE_HOME
    KEEP_E2E=0
    cleanup 0
}
case_cleanup_child() {
    ROOT=$1
    REAL_HOME=$FAKE_HOME
    KEEP_E2E=0
    assign_layout
    prepare_directories
    start_process_group "$TMP_DIR/cleanup.log" sleep 300
    echo "$ACTIVE_GROUP" >"$2"
    sleep 0.2
    cleanup 0
}

make_root
ROOT_CLEANUP=$ROOT_TO_USE
expect_ok "valid sandbox is removable" case_removable_ok "$ROOT_CLEANUP"
expect_fail "symlinked sandbox is not removable" "" case_removable_symlink
expect_fail "cleanup refuses a symlinked sandbox" "refusing to remove" case_cleanup_symlink
expect_ok "cleanup removes a valid sandbox" case_cleanup_ok "$ROOT_CLEANUP"
if [ ! -e "$ROOT_CLEANUP" ]; then ok "cleanup actually removed the sandbox"; else bad "cleanup left the sandbox"; fi

make_root
ROOT_CHILD=$ROOT_TO_USE
CHILD_PID_FILE="$FAKE_HOME/child.pid"
expect_ok "cleanup stops the active process group before removing the tree" case_cleanup_child "$ROOT_CHILD" "$CHILD_PID_FILE"
child_pid=$(cat "$CHILD_PID_FILE" 2>/dev/null || true)
if [ -n "$child_pid" ] && ! kill -0 "$child_pid" 2>/dev/null; then
    ok "cleanup left no child process behind"
else
    bad "cleanup left child process $child_pid alive"
fi
if [ ! -e "$ROOT_CHILD" ]; then ok "cleanup removed the tree after stopping the child"; else bad "cleanup kept the tree with a stopped child"; fi

# --- timeout and process-group handling ----------------------------------

case_quick() {
    ROOT=$1
    REAL_HOME=$FAKE_HOME
    assign_layout
    prepare_directories
    run_with_timeout "$TMP_DIR/quick.log" 5 sh -c 'exit 0'
}

case_timeout() {
    ROOT=$1
    REAL_HOME=$FAKE_HOME
    assign_layout
    prepare_directories
    helper="$TMP_DIR/sleeper.sh"
    cat >"$helper" <<'EOF'
#!/bin/sh
trap '' TERM
sleep 300 &
echo "$!" >"$1"
while :; do sleep 1; done
EOF
    chmod 700 "$helper"
    run_with_timeout "$TMP_DIR/timeout.log" 1 sh "$helper" "$TMP_DIR/sleeper.pid"
}

case_bad_timeout() {
    ROOT=$1
    REAL_HOME=$FAKE_HOME
    assign_layout
    prepare_directories
    run_with_timeout "$TMP_DIR/bad.log" 0 sh -c 'exit 0'
}

make_root
ROOT_TIMEOUT=$ROOT_TO_USE
expect_ok "short command exits normally" case_quick "$ROOT_OK"
expect_status "timeout returns 124" 124 case_timeout "$ROOT_TIMEOUT"
sleep_pid=$(cat "$ROOT_TIMEOUT/tmp/sleeper.pid" 2>/dev/null || true)
if [ -n "$sleep_pid" ] && ! kill -0 "$sleep_pid" 2>/dev/null; then
    ok "timeout killed the TERM-ignoring process group"
else
    bad "timeout left process $sleep_pid alive"
fi
expect_fail "invalid timeout is rejected" "between 1 and 3600" case_bad_timeout "$(fresh_root)"

# --- interruption ---------------------------------------------------------

case_signal_runner() {
    ROOT=$1
    REAL_HOME=$FAKE_HOME
    KEEP_E2E=0
    assign_layout
    prepare_directories
    trap 'cleanup $?' EXIT
    trap 'cleanup 143' TERM
    helper="$TMP_DIR/sleeper.sh"
    cat >"$helper" <<'EOF'
#!/bin/sh
trap '' TERM
sleep 300 &
echo "$!" >"$1"
while :; do sleep 1; done
EOF
    chmod 700 "$helper"
    run_with_timeout "$TMP_DIR/signal.log" 300 sh "$helper" "$2"
}

make_root
ROOT_SIGNAL=$ROOT_TO_USE
SIGNAL_PID_FILE="$FAKE_HOME/signal.pid"
rm -f "$SIGNAL_PID_FILE"
( case_signal_runner "$ROOT_SIGNAL" "$SIGNAL_PID_FILE" ) &
runner=$!
attempt=0
while [ ! -s "$SIGNAL_PID_FILE" ] && [ "$attempt" -lt 50 ]; do
    sleep 0.1
    attempt=$((attempt + 1))
done
sleep 0.2
kill -TERM "$runner" 2>/dev/null || true
wait "$runner"
signal_status=$?
if [ "$signal_status" = 143 ]; then
    ok "SIGTERM exits with status 143"
else
    bad "SIGTERM exited with status $signal_status"
fi
if [ ! -e "$ROOT_SIGNAL" ]; then
    ok "SIGTERM removes the sandbox"
else
    bad "SIGTERM left the sandbox"
fi
signal_pid=$(cat "$SIGNAL_PID_FILE" 2>/dev/null || true)
if [ -n "$signal_pid" ] && ! kill -0 "$signal_pid" 2>/dev/null; then
    ok "SIGTERM stops the whole process group"
else
    bad "SIGTERM left process $signal_pid alive"
fi

# --- real environment snapshots ------------------------------------------

case_snapshot_stable() {
    REAL_HOME=$1
    before=$(snapshot_real_environment)
    after=$(snapshot_real_environment)
    [ "$before" = "$after" ]
}
case_snapshot_change() {
    REAL_HOME=$1
    before=$(snapshot_real_environment)
    printf 'changed\n' >"$REAL_HOME/.config/home-manager/home.nix"
    after=$(snapshot_real_environment)
    [ "$before" != "$after" ]
}

make_outside
SNAPSHOT_HOME="$OUTSIDE/home"
mkdir -p "$SNAPSHOT_HOME/.config/home-manager" "$SNAPSHOT_HOME/.local/state/nix-home-studio"
printf 'a\n' >"$SNAPSHOT_HOME/.config/home-manager/home.nix"
expect_ok "real environment snapshot is stable when nothing changes" case_snapshot_stable "$SNAPSHOT_HOME"
expect_ok "real environment snapshot notices a config change" case_snapshot_change "$SNAPSHOT_HOME"

# --- snapshot read failures fail closed -----------------------------------

case_digest_unreadable_dir() {
    setup_case "$1"
    target=$(mktemp -d "$FAKE_HOME/unreadable-dir.XXXXXX") || return 1
    mkdir -p "$target/child"
    : >"$target/child/file"
    chmod 000 "$target"
    tree_digest "$target"
    status=$?
    chmod 700 "$target"
    return "$status"
}
case_digest_unreadable_file() {
    setup_case "$1"
    target=$(mktemp -d "$FAKE_HOME/unreadable-file.XXXXXX") || return 1
    : >"$target/file"
    chmod 000 "$target/file"
    tree_digest "$target"
    status=$?
    chmod 600 "$target/file"
    return "$status"
}
case_print_real_digest_unreadable() {
    setup_case "$1"
    target=$(mktemp -d "$FAKE_HOME/unreadable-preflight.XXXXXX") || return 1
    mkdir -p "$target/child"
    : >"$target/child/file"
    chmod 000 "$target"
    ( print_real_digest test_path "$target" )
    status=$?
    chmod 700 "$target"
    return "$status"
}
case_snapshot_unreadable() {
    setup_case "$1"
    home="$2"
    mkdir -p "$home/.config/home-manager"
    : >"$home/.config/home-manager/home.nix"
    chmod 000 "$home/.config/home-manager"
    REAL_HOME=$home
    snapshot_real_environment
    status=$?
    chmod 700 "$home/.config/home-manager"
    return "$status"
}

expect_fail "unreadable directory digest fails closed" "unreadable" case_digest_unreadable_dir "$(fresh_root)"
expect_fail "unreadable file digest fails closed" "unreadable" case_digest_unreadable_file "$(fresh_root)"
expect_fail "preflight digest refuses an unreadable real path" "cannot read the real environment completely" case_print_real_digest_unreadable "$(fresh_root)"
make_outside
UNREADABLE_HOME="$OUTSIDE/unreadable-home"
mkdir -p "$UNREADABLE_HOME"
expect_fail "real-environment snapshot fails closed on an unreadable path" "unreadable" case_snapshot_unreadable "$(fresh_root)" "$UNREADABLE_HOME"

# --- inaccessible parent directories -------------------------------------

case_digest_unreadable_parent() {
    setup_case "$1"
    parent=$(mktemp -d "$FAKE_HOME/locked-parent.XXXXXX") || return 1
    chmod 000 "$parent"
    tree_digest "$parent/missing"
    status=$?
    chmod 700 "$parent"
    return "$status"
}
case_fallback_parent_unreadable_preflight() {
    setup_case "$1"
    parent=$(mktemp -d "$FAKE_HOME/locked-fallback.XXXXXX") || return 1
    chmod 000 "$parent"
    NHS_E2E_TEST_FALLBACK_PATH="$parent/nhs-e2e"
    ( check_isolation )
    status=$?
    chmod 700 "$parent"
    return "$status"
}
case_postcondition_unreadable_parent() {
    setup_case "$1"
    parent=$(mktemp -d "$FAKE_HOME/locked-postcondition.XXXXXX") || return 1
    chmod 000 "$parent"
    output=$(check_hm_fallback_postcondition "$parent/nhs-e2e" 2>&1)
    status=$?
    chmod 700 "$parent"
    case "$output" in
        *"state=unreadable"*) : ;;
        *) return 1 ;;
    esac
    case "$output" in
        *"state=absent"*) return 1 ;;
    esac
    [ "$status" -ne 0 ]
}
case_profile_label_unreadable() {
    home=$(mktemp -d "$FAKE_HOME/profile-home.XXXXXX") || return 1
    mkdir -p "$home/.local/state/nix/profiles"
    chmod 000 "$home/.local/state/nix"
    ( REAL_HOME=$home; real_profile_label )
    status=$?
    chmod 700 "$home/.local/state/nix"
    return "$status"
}
case_fallback_label_unreadable() {
    setup_case "$1"
    parent=$(mktemp -d "$FAKE_HOME/locked-label.XXXXXX") || return 1
    chmod 000 "$parent"
    NHS_E2E_TEST_FALLBACK_PATH="$parent/nhs-e2e"
    ( e2e_fallback_label )
    status=$?
    chmod 700 "$parent"
    return "$status"
}

expect_fail "snapshot target with an inaccessible parent fails closed" "unreadable" case_digest_unreadable_parent "$(fresh_root)"
expect_fail "preflight rejects an unreadable fallback parent" "not provably absent" case_fallback_parent_unreadable_preflight "$(fresh_root)"
expect_ok "postcondition reports an unreadable fallback parent as unreadable, not absent" case_postcondition_unreadable_parent "$(fresh_root)"
expect_fail "real profile label fails closed on an inaccessible parent" "unreadable" case_profile_label_unreadable "$(fresh_root)"
expect_fail "fallback label fails closed on an inaccessible parent" "unreadable" case_fallback_label_unreadable "$(fresh_root)"

# --- integration: preflight only -----------------------------------------

HM_TEST=${NHS_TEST_HM:-}
if [ -z "$HM_TEST" ]; then
    HM_TEST=$(command -v home-manager 2>/dev/null || true)
fi
if [ -z "$HM_TEST" ]; then
    for candidate in /nix/store/*-home-manager-*/bin/home-manager; do
        [ -x "$candidate" ] || continue
        HM_TEST=$candidate
        break
    done
fi
NP_TEST=${NHS_TEST_NIXPKGS:-}
if [ -z "$NP_TEST" ] && command -v nix >/dev/null 2>&1; then
    NP_TEST=$(cd "$REPO" && nix eval --raw --impure \
        --expr '(builtins.getFlake (toString ./.)).inputs.nixpkgs.outPath' 2>/dev/null || true)
fi
if [ -n "$NP_TEST" ] && [ -f "$NP_TEST/default.nix" ] && [ -n "$HM_TEST" ] && [ -x "$HM_TEST" ]; then
    INTEGRATION=1
else
    INTEGRATION=0
    skip "preflight integration (no /nix/store home-manager and nixpkgs source)"
fi

run_harness() {
    env -u NHS_E2E_LIB_ONLY \
        NHS_E2E_HM="$HM_TEST" \
        NHS_E2E_NIXPKGS="$NP_TEST" \
        "$@" sh "$LIB" --preflight
}

if [ "$INTEGRATION" = 1 ]; then
    if output=$(run_harness 2>&1); then
        case "$output" in
            *"preflight=ok"*) ok "preflight succeeds with explicit store paths" ;;
            *) bad "preflight output lacks the ok line" ;;
        esac
        case "$output" in
            *"e2e_fallback_profile=absent"*) ok "preflight reports the Home Manager fallback path absent" ;;
            *) bad "preflight did not report the fallback path state" ;;
        esac
    else
        bad "preflight failed unexpectedly"
        printf '%s\n' "$output" | sed 's/^/    /'
    fi

    first_output=$(run_harness 2>&1)
    first_status=$?
    second_output=$(run_harness 2>&1)
    second_status=$?
    first_root=$(printf '%s\n' "$first_output" | sed -n 's/^root=//p' | tail -1)
    second_root=$(printf '%s\n' "$second_output" | sed -n 's/^root=//p' | tail -1)
    if [ "$first_status" -eq 0 ] && [ "$second_status" -eq 0 ] &&
        [ -n "$first_root" ] && [ -n "$second_root" ] &&
        [ "$first_root" != "$second_root" ] &&
        [ ! -e "$first_root" ] && [ ! -e "$second_root" ]; then
        ok "repeated preflight uses a fresh tree and removes it"
    else
        bad "repeated preflight did not create/remove two distinct trees"
    fi

    case_config_parse() {
        setup_case "$1"
        write_config "pkgs.hello"
        nix-instantiate --parse "$CONFIG_FILE" >/dev/null
    }
    if command -v nix-instantiate >/dev/null 2>&1; then
        expect_ok "generated configuration parses" case_config_parse "$(fresh_root)"
    else
        skip "generated configuration parse (nix-instantiate not available)"
    fi

    case_nix_client() {
        setup_case "$1"
        nixbin=$(dirname "$(readlink -f "$(command -v nix)")")
        env -i \
            HOME="$HOME_DIR" \
            USER=nhs-e2e \
            LOGNAME=nhs-e2e \
            XDG_DATA_HOME="$DATA_DIR" \
            XDG_STATE_HOME="$STATE_DIR" \
            XDG_CONFIG_HOME="$CONFIG_DIR" \
            XDG_CACHE_HOME="$CACHE_DIR" \
            XDG_RUNTIME_DIR="$RUNTIME_DIR" \
            TMPDIR="$TMP_DIR" \
            TMP="$TMP_DIR" \
            TEMP="$TMP_DIR" \
            NIX_REMOTE=daemon \
            NIX_PATH="nixpkgs=$NP_TEST" \
            PATH="$nixbin:/usr/bin:/bin" \
            nix-build --quiet --expr '{}' --no-out-link
    }
    if command -v nix-build >/dev/null 2>&1; then
        expect_ok "Nix client works with the child environment and default state dir" case_nix_client "$(fresh_root)"
    else
        skip "Nix client compatibility (nix-build not available)"
    fi

    if output=$(NIX_REMOTE=ssh://blocked XDG_DATA_HOME="$FAKE_HOME/.local/share" \
        TMPDIR=/tmp DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/0/bus \
        run_harness 2>&1); then
        case "$output" in
            *"caller_env_ignored="*NIX_REMOTE*) : ;;
            *) bad "hostile caller env was not reported" ;;
        esac
        case "$output" in
            *"child_xdg_data=/var/tmp/nhs-e2e."*) : ;;
            *) bad "XDG_DATA_HOME was not replaced with the sandbox" ;;
        esac
        case "$output" in
            *"child_tmpdir=/var/tmp/nhs-e2e."*) : ;;
            *) bad "TMPDIR was not replaced with the sandbox" ;;
        esac
        case "$output" in
            *"child_dbus=unix:path=/var/tmp/nhs-e2e."*) : ;;
            *) bad "D-Bus address was not replaced with the sandbox" ;;
        esac
        case "$output" in
            *"child_xdg_data=$FAKE_HOME"*) bad "sandbox inherited the real XDG_DATA_HOME" ;;
            *) ok "hostile caller XDG/TMP/DBUS environment is cleared" ;;
        esac
    else
        bad "preflight failed with a hostile caller environment"
        printf '%s\n' "$output" | sed 's/^/    /'
    fi

    if output=$(KEEP_E2E=1 run_harness 2>&1); then
        kept_root=$(printf '%s\n' "$output" | sed -n 's/^root=//p' | tail -1)
        if [ -n "$kept_root" ] && [ -d "$kept_root" ]; then
            track_root "$kept_root"
            for directory in data state config cache tmp runtime; do
                if [ -d "$kept_root/$directory" ]; then :; else bad "kept sandbox lacks $directory"; fi
            done
            if [ -d "$kept_root/state/nix/profiles" ]; then
                ok "kept sandbox pre-creates the Home Manager profile directory"
            else
                bad "kept sandbox lacks state/nix/profiles"
            fi
            if [ "$(stat -c %a "$kept_root/runtime")" = 700 ]; then
                ok "kept sandbox runtime directory is 700"
            else
                bad "kept sandbox runtime directory mode is not 700"
            fi
            if [ ! -e "$kept_root/runtime/bus" ]; then
                ok "kept sandbox runtime has no bus socket"
            else
                bad "kept sandbox runtime has a bus socket"
            fi
            if [ -z "$(find "$kept_root/tmp" -maxdepth 1 -name 'home-manager-build.*' -print -quit 2>/dev/null)" ]; then
                ok "preflight creates no home-manager work directory"
            else
                bad "preflight left a home-manager work directory"
            fi
            if [ -z "$(find "$kept_root/home" -mindepth 1 -print -quit 2>/dev/null)" ]; then
                ok "preflight writes nothing into the sandbox home"
            else
                bad "preflight wrote into the sandbox home"
            fi
        else
            bad "preflight with KEEP_E2E=1 did not report a sandbox"
        fi
    else
        bad "kept preflight failed"
        printf '%s\n' "$output" | sed 's/^/    /'
    fi

    run_harness_strict() {
        unset NIX_PROFILES NIX_USER_PROFILE_DIR NIX_STATE_DIR NIX_CONFIG \
            NIX_CONF_DIR NIX_USER_CONF_FILES NIX_STORE_DIR NIX_STORE \
            NIX_DAEMON_SOCKET_PATH HOME_MANAGER_CONFIG HOME_MANAGER_BACKUP_EXT
        NHS_E2E_STRICT_ENV=1
        NIX_REMOTE=ssh://blocked
        export NHS_E2E_STRICT_ENV NIX_REMOTE
        run_harness
    }

    if output=$(run_harness_strict 2>&1); then
        bad "strict mode accepted NIX_REMOTE"
    else
        case "$output" in
            *"NIX_REMOTE"*) ok "strict mode rejects an inherited NIX_REMOTE" ;;
            *) bad "strict mode failed without naming NIX_REMOTE" ;;
        esac
    fi

    if output=$(run_harness NHS_E2E_NIXPKGS=/tmp 2>&1); then
        bad "preflight accepted a non-store nixpkgs"
    else
        case "$output" in
            *"/nix/store"*) ok "preflight rejects a non-store nixpkgs" ;;
            *) bad "non-store nixpkgs failure was not explained" ;;
        esac
    fi

    if output=$(run_harness NHS_E2E_HM=/tmp/not-a-home-manager 2>&1); then
        bad "preflight accepted a non-store home-manager"
    else
        case "$output" in
            *"/nix/store"*) ok "preflight rejects a non-store home-manager" ;;
            *) bad "non-store home-manager failure was not explained" ;;
        esac
    fi
fi

# --- CLI surface ----------------------------------------------------------

if env -u NHS_E2E_LIB_ONLY sh "$LIB" --help >/dev/null 2>&1; then
    ok "help exits 0"
else
    bad "help did not exit 0"
fi
if env -u NHS_E2E_LIB_ONLY sh "$LIB" --not-an-option >/dev/null 2>&1; then
    bad "unknown option was accepted"
else
    ok "unknown option is rejected"
fi

printf '\npassed=%s failed=%s skipped=%s\n' "$PASS" "$FAILED" "$SKIPPED"
[ "$FAILED" -eq 0 ]
