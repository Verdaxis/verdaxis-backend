#!/usr/bin/env bash
# Disposable corpus for the deploy-start guard. Every case builds a private
# temporary runtime root, runs the real guard file against it, and contacts no
# host, database or network.
set -euo pipefail
export LC_ALL=C
umask 077

repository=$(git rev-parse --show-toplevel)
cd "$repository"
guard=deploy/scripts/verify-deploy-start-guard.sh
reviewed=deploy/tests/shellcheck-files.txt
[[ -f "$guard" && ! -L "$guard" ]] || {
    printf 'missing guard: %s\n' "$guard" >&2
    exit 1
}
bash -n "$guard"
[[ -f "$reviewed" ]] || {
    printf 'missing reviewed list: %s\n' "$reviewed" >&2
    exit 1
}

work=$(mktemp -d "${TMPDIR:-/tmp}/verdaxis-guard-contract.XXXXXX")
held_pids=()
cleanup() {
    local pid
    for pid in "${held_pids[@]}"; do
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done
    rm -rf -- "$work"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

sha_a=1111111111111111111111111111111111111111
sha_b=2222222222222222222222222222222222222222
failures=0
record() {
    printf 'FAIL %s\n' "$1" >&2
    failures=$((failures + 1))
}

new_fixture() {
    local fixture=$work/fixture-$1
    mkdir -p -- "$fixture/scripts"
    cp -- "$guard" "$fixture/scripts/verify-deploy-start-guard.sh"
    printf '%s\n' "$fixture"
}

run_guard() {
    local fixture=$1
    shift
    bash "$fixture/scripts/verify-deploy-start-guard.sh" "$@" \
        >"$work/stdout" 2>"$work/stderr"
}

check_outcome() {
    local label=$1 status=$2
    if [[ "$status" -eq 0 ]]; then
        [[ ! -s "$work/stdout" ]] || record "$label: stdout is not empty"
        [[ ! -s "$work/stderr" ]] || record "$label: stderr is not empty"
    elif [[ "$status" -ne 1 ]]; then
        record "$label: exit $status, expected 1"
    elif [[ ! -s "$work/stderr" ]]; then
        record "$label: denial was silent"
    fi
}

expect_allowed() {
    local label=$1 status=0
    shift
    run_guard "$@" || status=$?
    if [[ "$status" -ne 0 ]]; then
        record "$label: a permitted start was denied"
    else
        check_outcome "$label" 0
    fi
}

expect_denied_at() {
    local label=$1 path=$2 status=0
    shift 2
    bash "$path" "$@" >"$work/stdout" 2>"$work/stderr" || status=$?
    if [[ "$status" -eq 0 ]]; then
        record "$label: an invalid start was allowed"
    else
        check_outcome "$label" "$status"
    fi
}

expect_denied() {
    local label=$1 fixture=$2
    shift 2
    expect_denied_at "$label" \
        "$fixture/scripts/verify-deploy-start-guard.sh" "$@"
}

write_state() {
    local fixture=$1 environment=$2 sha=$3
    mkdir -p -- "$fixture/.runtime-deploy"
    chmod 0755 "$fixture/.runtime-deploy"
    printf 'DEPLOYMENT_STATE=restart-authorized\nENVIRONMENT=%s\nRELEASE_SHA=%s\n' \
        "$environment" "$sha" \
        >"$fixture/.runtime-deploy/$environment.state"
    chmod 0644 "$fixture/.runtime-deploy/$environment.state"
}

create_lock() {
    local path=$1
    mkdir -p -- "${path%/*}"
    (umask 077 && : >"$path")
    chmod 0600 "$path"
}

hold_lock() {
    local path=$1
    create_lock "$path"
    # `-o` closes the descriptor before the command runs, so killing the holder
    # releases the lock even though its sleep child keeps running.
    flock -o "$path" sleep 60 &
    held_pids+=("$!")
    for _ in $(seq 1 200); do
        if ! flock -n "$path" true 2>/dev/null; then
            return 0
        fi
        sleep 0.05
    done
    printf 'lock holder did not start: %s\n' "$path" >&2
    exit 1
}

# An absent state file is the normal start path.
fixture=$(new_fixture absent)
expect_allowed 'absent state directory' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
mkdir -p -- "$fixture/.runtime-deploy"
chmod 0755 "$fixture/.runtime-deploy"
expect_allowed 'empty state directory' "$fixture" \
    --environment production --expected-release-sha "$sha_b"
mkdir -p -- "$fixture/bin"
cp -- "$guard" "$fixture/bin/verify-deploy-start-guard.sh"
expect_denied_at 'misplaced helper' "$fixture/bin/verify-deploy-start-guard.sh" \
    --environment staging --expected-release-sha "$sha_a"

expect_denied 'no arguments' "$fixture"
expect_denied 'missing value' "$fixture" --environment staging
expect_denied 'extra argument' "$fixture" \
    --environment staging --expected-release-sha "$sha_a" extra
expect_denied 'reordered flags' "$fixture" \
    --expected-release-sha "$sha_a" --environment staging
expect_denied 'test environment' "$fixture" \
    --environment test --expected-release-sha "$sha_a"
expect_denied 'short release' "$fixture" \
    --environment staging --expected-release-sha short
expect_denied 'uppercase release' "$fixture" \
    --environment staging --expected-release-sha \
    111111111111111111111111111111111111111A
expect_denied 'unknown flag' "$fixture" \
    --environment staging --expected-release-sha "$sha_a" --apply

# Stale authorization and restart windows require both locks.
fixture=$(new_fixture locks)
state_dir=$fixture/.runtime-deploy
write_state "$fixture" staging "$sha_a"
expect_denied 'stale authorization' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
hold_lock "$state_dir/staging.lock"
expect_denied 'main lock only' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
hold_lock "$state_dir/staging.restart.lock"
expect_allowed 'both locks held' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
kill "${held_pids[0]}" 2>/dev/null || true
wait "${held_pids[0]}" 2>/dev/null || true
expect_denied 'restart lock only' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"

# Exact state, wrong release and unsafe state paths.
fixture=$(new_fixture state)
state_dir=$fixture/.runtime-deploy
state=$state_dir/staging.state
write_state "$fixture" staging "$sha_a"
hold_lock "$state_dir/staging.lock"
hold_lock "$state_dir/staging.restart.lock"
expect_allowed 'exact state' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
expect_denied 'wrong release' "$fixture" \
    --environment staging --expected-release-sha "$sha_b"

mv -- "$state" "$fixture/valid.state"
ln -s -- "$fixture/missing.state" "$state"
expect_denied 'dangling state' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
rm -- "$state"
ln -s -- "$fixture/valid.state" "$state"
expect_denied 'state symlink' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
rm -- "$state"
mv -- "$fixture/valid.state" "$state"

printf 'EXTRA=denied\n' >>"$state"
expect_denied 'extra state line' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
printf '\n' >>"$state"
expect_denied 'trailing blank line' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"

printf '%0257d' 0 >"$state"
chmod 0644 "$state"
expect_denied 'oversize state' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"

write_state "$fixture" staging "$sha_a"
chmod 0666 "$state"
expect_denied 'writable state' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"

write_state "$fixture" staging "$sha_a"
rm -- "$state"
mkdir -- "$state"
expect_denied 'state directory' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
rmdir -- "$state"

write_state "$fixture" staging "$sha_a"
rm -- "$state_dir/staging.lock"
mkdir -- "$state_dir/staging.lock"
expect_denied 'lock directory' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
rmdir -- "$state_dir/staging.lock"
create_lock "$state_dir/staging.lock"
expect_denied 'replaced main lock' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
chmod 0666 "$state_dir/staging.lock"
expect_denied 'writable lock' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
chmod 0600 "$state_dir/staging.lock"

rm -- "$state_dir/staging.restart.lock"
expect_denied 'missing restart lock' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"

mv -- "$state_dir" "$fixture/real-deploy"
ln -s -- "$fixture/real-deploy" "$state_dir"
expect_denied 'linked deploy directory' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
rm -- "$state_dir"
mv -- "$fixture/real-deploy" "$state_dir"
chmod 0777 "$state_dir"
expect_denied 'writable deploy directory' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
chmod 0755 "$state_dir"

# The guard files are reviewed and independent of any interpreter.
forbidden=$(printf '%s%s' 'py' 'thon')
for path in "$guard" deploy/tests/guard-contract.sh; do
    grep -Fxq -- "$path" "$reviewed" || record "unreviewed script: $path"
    if grep -Fiq -- "$forbidden" "$path"; then
        record "$path: interpreter dependency"
    fi
done

if ((failures > 0)); then
    printf '%d guard contract check(s) failed\n' "$failures" >&2
    exit 1
fi
printf 'guard contract passed\n'
