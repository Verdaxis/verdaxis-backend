#!/usr/bin/env bash
# Disposable corpus for the deploy-start guard. Every case builds a private
# temporary runtime root, runs the real guard file against it, and contacts no
# host, database or network. Every denial is compared against the exact reason.
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
denial='Backend start denied by deployment guard'
# The shims below delegate to the same stat the guard would see without them.
real_stat=$(type -ap stat | head -n 1)

work=$(mktemp -d "${TMPDIR:-/tmp}/verdaxis-guard-contract.XXXXXX")
held_pids=()
cleanup() {
    local pid
    for pid in "${held_pids[@]}"; do
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done
    chmod -R u+rwx -- "$work" 2>/dev/null || true
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

run_at() {
    local target=$1 prefix=$2
    shift 2
    if [[ -n "$prefix" ]]; then
        PATH="$prefix:$PATH" bash "$target" "$@" >"$work/stdout" 2>"$work/stderr"
    else
        bash "$target" "$@" >"$work/stdout" 2>"$work/stderr"
    fi
}

check_allowed_output() {
    local label=$1
    [[ ! -s "$work/stdout" ]] || record "$label: stdout is not empty"
    [[ ! -s "$work/stderr" ]] || record "$label: stderr is not empty"
}

check_denied_output() {
    local label=$1 status=$2 reason=$3 expected
    [[ ! -s "$work/stdout" ]] || record "$label: denial wrote to stdout"
    if [[ "$status" -ne 1 ]]; then
        record "$label: exit $status, expected 1"
    fi
    expected="$denial: $reason"
    if ! grep -Fxq -- "$expected" "$work/stderr"; then
        record "$label: reason is '$(head -n 1 "$work/stderr")', expected '$expected'"
    fi
}

expect_allowed() {
    local label=$1 fixture=$2 status=0
    shift 2
    run_at "$fixture/scripts/verify-deploy-start-guard.sh" '' "$@" || status=$?
    if [[ "$status" -ne 0 ]]; then
        record "$label: a permitted start was denied: $(head -n 1 "$work/stderr")"
    else
        check_allowed_output "$label"
    fi
}

expect_denied_at() {
    local label=$1 reason=$2 prefix=$3 path=$4 status=0
    shift 4
    run_at "$path" "$prefix" "$@" || status=$?
    if [[ "$status" -eq 0 ]]; then
        record "$label: an invalid start was allowed"
    else
        check_denied_output "$label" "$status" "$reason"
    fi
}

expect_denied() {
    local label=$1 reason=$2 fixture=$3
    shift 3
    expect_denied_at "$label" "$reason" '' \
        "$fixture/scripts/verify-deploy-start-guard.sh" "$@"
}

expect_denied_with_shim() {
    local label=$1 reason=$2 prefix=$3 fixture=$4
    shift 4
    expect_denied_at "$label" "$reason" "$prefix" \
        "$fixture/scripts/verify-deploy-start-guard.sh" "$@"
}

write_state() {
    local fixture=$1 environment=$2 sha=$3 phase=${4:-restart-authorized}
    mkdir -p -- "$fixture/.runtime-deploy"
    chmod 0755 "$fixture/.runtime-deploy"
    printf 'DEPLOYMENT_STATE=%s\nENVIRONMENT=%s\nRELEASE_SHA=%s\n' \
        "$phase" "$environment" "$sha" \
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

# Two controlled shims prove the guard does not fail open on a bad descriptor
# identity or on a flock error that is not contention.
mkdir -p -- "$work/stat-shim" "$work/flock-shim"
cat >"$work/stat-shim/stat" <<SHIM
#!/usr/bin/env bash
real=\${REAL_STAT:?}
for argument in "\$@"; do
    case \$argument in
        */fd/7)
            output=\$("\$real" "\$@") || exit \$?
            printf '%s\n' "\$output" | awk -F: 'BEGIN { OFS = ":" } { \$2 = \$2 + 1; print }'
            exit 0
            ;;
    esac
done
exec "\$real" "\$@"
SHIM
printf '#!/usr/bin/env bash\nexit 1\n' >"$work/flock-shim/flock"
chmod 0755 "$work/stat-shim/stat" "$work/flock-shim/flock"
export REAL_STAT=$real_stat

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
expect_denied_at 'misplaced helper' 'misplaced helper' '' \
    "$fixture/bin/verify-deploy-start-guard.sh" \
    --environment staging --expected-release-sha "$sha_a"

expect_denied 'no arguments' 'invalid arguments' "$fixture"
expect_denied 'missing value' 'invalid arguments' "$fixture" --environment staging
expect_denied 'extra argument' 'invalid arguments' "$fixture" \
    --environment staging --expected-release-sha "$sha_a" extra
expect_denied 'reordered flags' 'invalid arguments' "$fixture" \
    --expected-release-sha "$sha_a" --environment staging
expect_denied 'test environment' 'unsupported environment' "$fixture" \
    --environment test --expected-release-sha "$sha_a"
expect_denied 'short release' 'invalid release sha' "$fixture" \
    --environment staging --expected-release-sha short
expect_denied 'uppercase release' 'invalid release sha' "$fixture" \
    --environment staging --expected-release-sha \
    111111111111111111111111111111111111111A
expect_denied 'unknown flag' 'invalid arguments' "$fixture" \
    --environment staging --expected-release-sha "$sha_a" --apply

# An unsearchable state directory denies instead of reading as absent.
fixture=$(new_fixture unsearchable)
mkdir -p -- "$fixture/.runtime-deploy"
write_state "$fixture" staging "$sha_a"
hold_lock "$fixture/.runtime-deploy/staging.lock"
hold_lock "$fixture/.runtime-deploy/staging.restart.lock"
chmod 0000 "$fixture/.runtime-deploy"
expect_denied 'unsearchable deploy directory' 'deploy state is unsearchable' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"
chmod 0755 "$fixture/.runtime-deploy"
expect_allowed 'searchable deploy directory restored' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
chmod 0644 "$fixture/.runtime-deploy"
expect_denied 'unreadable deploy directory' 'deploy state is unsearchable' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"
chmod 0755 "$fixture/.runtime-deploy"

# Stale authorization and restart windows require both locks.
fixture=$(new_fixture locks)
state_dir=$fixture/.runtime-deploy
write_state "$fixture" staging "$sha_a"
expect_denied 'stale authorization' 'restart authorization is not held' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"
hold_lock "$state_dir/staging.lock"
expect_denied 'main lock only' 'restart authorization is not held' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"
hold_lock "$state_dir/staging.restart.lock"
expect_allowed 'both locks held' "$fixture" \
    --environment staging --expected-release-sha "$sha_a"
kill "${held_pids[0]}" 2>/dev/null || true
wait "${held_pids[0]}" 2>/dev/null || true
expect_denied 'restart lock only' 'restart authorization is not held' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"

# A flock error that is not contention must deny, not authorize.
expect_denied_with_shim 'flock error denies' 'main lock check failed' \
    "$work/flock-shim" "$fixture" \
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
expect_denied 'wrong release' 'deployment state does not authorize a restart' \
    "$fixture" --environment staging --expected-release-sha "$sha_b"

# A descriptor identity that differs from the path identity denies.
expect_denied_with_shim 'descriptor identity mismatch' \
    'deployment state changed during validation' "$work/stat-shim" "$fixture" \
    --environment staging --expected-release-sha "$sha_a"

for phase in deploying failed blocked rust-failed readiness-pending unknown; do
    write_state "$fixture" staging "$sha_a" "$phase"
    expect_denied "phase $phase" 'deployment state does not authorize a restart' \
        "$fixture" --environment staging --expected-release-sha "$sha_a"
done
write_state "$fixture" staging "$sha_a"

mv -- "$state" "$fixture/valid.state"
ln -s -- "$fixture/missing.state" "$state"
expect_denied 'dangling state' 'deployment state is not a regular file' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"
rm -- "$state"
ln -s -- "$fixture/valid.state" "$state"
expect_denied 'state symlink' 'deployment state is not a regular file' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"
rm -- "$state"
mv -- "$fixture/valid.state" "$state"

printf 'EXTRA=denied\n' >>"$state"
expect_denied 'extra state line' 'deployment state does not authorize a restart' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"
printf '\n' >>"$state"
expect_denied 'trailing blank line' 'deployment state does not authorize a restart' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"

printf '%0257d' 0 >"$state"
chmod 0644 "$state"
expect_denied 'oversize state' 'deployment state is too large' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"

write_state "$fixture" staging "$sha_a"
chmod 0666 "$state"
expect_denied 'writable state' 'deployment state is not owner-controlled' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"

write_state "$fixture" staging "$sha_a"
ln -- "$state" "$fixture/state-link"
expect_denied 'hard-linked state' 'deployment state is not owner-controlled' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"
rm -- "$fixture/state-link"

write_state "$fixture" staging "$sha_a"
rm -- "$state"
mkdir -- "$state"
expect_denied 'state directory' 'deployment state is not a regular file' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"
rmdir -- "$state"

write_state "$fixture" staging "$sha_a"
rm -- "$state_dir/staging.lock"
mkdir -- "$state_dir/staging.lock"
expect_denied 'lock directory' 'main lock is not a regular file' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"
rmdir -- "$state_dir/staging.lock"
create_lock "$state_dir/staging.lock"
# The main lock is now released again; hold it so the restart path is reached.
expect_denied 'released main lock' 'restart authorization is not held' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"
hold_lock "$state_dir/staging.lock"
chmod 0666 "$state_dir/staging.lock"
expect_denied 'writable lock' 'main lock is not owner-controlled' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"
chmod 0600 "$state_dir/staging.lock"

# With the main lock held, replacing the restart lock is the failing path.
rm -- "$state_dir/staging.restart.lock"
mkdir -- "$state_dir/staging.restart.lock"
expect_denied 'replaced restart lock' 'restart lock is not a regular file' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"
rmdir -- "$state_dir/staging.restart.lock"
expect_denied 'missing restart lock' 'restart lock is not a regular file' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"

mv -- "$state_dir" "$fixture/real-deploy"
ln -s -- "$fixture/real-deploy" "$state_dir"
expect_denied 'linked deploy directory' 'deploy state is not a private directory' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"
rm -- "$state_dir"
mv -- "$fixture/real-deploy" "$state_dir"
chmod 0777 "$state_dir"
expect_denied 'writable deploy directory' 'deploy state is not a private directory' \
    "$fixture" --environment staging --expected-release-sha "$sha_a"
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
