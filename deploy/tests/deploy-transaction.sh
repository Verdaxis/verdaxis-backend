#!/usr/bin/env bash
# Phase 10 Task 7: the deploy transaction without Python.
#
# Each case builds a disposable host: a bare `origin` with two commits on
# `staging` (the running release A and the candidate B), a checkout at A, a
# deployed .env and the repository's real scripts/deploy.sh. systemctl, sudo,
# curl and systemd-analyze are stubs that record their calls. The fixture's
# artifact verifier stands in for deploy/scripts/verify-artifact.sh (tested by
# deploy/tests/artifact-contract.sh): it installs a fake verdaxis-read that
# records every call, keeps a simulated database revision, checks deploy
# ownership the way `migrate` does, and validates readiness bodies the way
# `guard health` does. Nothing contacts a network, a database or systemd.
#
# Part 1 holds the named Task 7 cases. Part 2 replaces every Python deploy
# test through an explicit gate-map entry. Those tests exercised scripts/deploy.sh
# through a fake interpreter (plan section 10.4a: no gate is lost).
#
# Linux only (the deploy reads /proc/self/fd); CI runs it in the shellcheck
# job. Do not run this suite directly on macOS.
# Usage: bash deploy/tests/deploy-transaction.sh (from the repository root)
#
# Many checks match the literal text of scripts/deploy.sh, so single-quoted
# `$NAME` and a trailing backslash inside quotes are intended here.
# shellcheck disable=SC2016,SC1003
set -euo pipefail
export LC_ALL=C

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"
[[ $(uname -s) == Linux ]] || {
    echo 'deploy-transaction.sh needs Linux (the deploy reads /proc/self/fd)' >&2
    exit 2
}
for tool in git flock jq stat mktemp sha256sum tar openssl; do
    command -v "$tool" > /dev/null || {
        echo "missing tool: $tool" >&2
        exit 2
    }
done

work=$(mktemp -d "${TMPDIR:-/tmp}/verdaxis-deploy-transaction.XXXXXX")
# KEEP_WORK=1 keeps the disposable hosts for inspection.
if [[ -z ${KEEP_WORK:-} ]]; then trap 'rm -rf -- "$work"' EXIT; fi
failures=0
passed_cases=()
pass() {
    echo "  ok   $1"
    passed_cases+=("$1")
}
fail() {
    echo "  FAIL $1" >&2
    failures=$((failures + 1))
}
# check <name> <command...>: the command is a case function; its status decides.
check() {
    local name=$1
    shift
    if "$@"; then
        pass "$name"
    else
        fail "$name"
        # The last host's output and calls, to show where it went wrong.
        if [[ -n ${H:-} && -f $H/calls.log ]]; then
            tail -n 4 "$H/stderr" 2> /dev/null | sed 's/^/      stderr: /' >&2
            tail -n 3 "$H/calls.log" | sed 's/^/      call: /' >&2
        fi
    fi
}

CURRENT_REVISION=ob_20261031_offboarding
ADOPTION_ROW=$(printf '%s\tsqlx_1' "$CURRENT_REVISION")
export GIT_AUTHOR_NAME=fixture GIT_AUTHOR_EMAIL=fixture@example.invalid
export GIT_COMMITTER_NAME=fixture GIT_COMMITTER_EMAIL=fixture@example.invalid

# The release files the deploy reads from a commit, copied from this checkout.
FIXTURE_FILES=(
    .gitignore
    requirements.txt
    constraints.txt
    scripts/deploy.sh
    deploy/migration-checkpoints.tsv
    deploy/postgres/app_acl_policy.sql
    deploy/postgres/converge_runtime_object_acls.sql
    deploy/rust/staging-policy.json
    deploy/rust/production-policy.json
    deploy/rust/artifact-provenance.pub.pem
    deploy/scripts/converge-acls.sh
    deploy/scripts/safe-root-git.sh
    deploy/scripts/verify-deploy-start-guard.sh
)

# ---- fixture --------------------------------------------------------------------------

# new_host <name> [environment] [extra checkpoint row]: a fresh host. Sets H
# (harness dir), BE (checkout), ENVIRONMENT_NAME, A and B (commit SHAs).
new_host() {
    unset APPROVED_RELEASE_SHA MIGRATION_APPROVED_SOURCE_SHA MIGRATION_EXPECTED_CURRENT_REVISION MIGRATION_TARGET_REVISION
    H="$work/$1"
    rm -rf -- "$H"
    ENVIRONMENT_NAME=${2:-staging}
    local extra_row=${3:-} relative unit app_role database
    mkdir -p "$H/bin" "$H/src"
    printf '%s' "$ENVIRONMENT_NAME" > "$H/environment"
    for relative in "${FIXTURE_FILES[@]}"; do
        mkdir -p "$H/src/$(dirname "$relative")"
        cp -- "$relative" "$H/src/$relative"
    done
    mkdir -p "$H/src/deploy/systemd"
    cp -- deploy/systemd/runtime-units.manifest "$H/src/deploy/systemd/"
    while IFS=$'\t' read -r _ unit; do
        [[ -n ${unit:-} ]] && cp -- "deploy/systemd/$unit" "$H/src/deploy/systemd/$unit"
    done < <(grep -v '^#' deploy/systemd/runtime-units.manifest)
    for relative in "$ADOPTION_ROW" "$(printf 'sqlx_1\tsqlx_1')" "$extra_row"; do
        if [[ -n $relative ]] && ! grep -q -F -x -- "$relative" "$H/src/deploy/migration-checkpoints.tsv"; then
            printf '%s\n' "$relative" >> "$H/src/deploy/migration-checkpoints.tsv"
        fi
    done
    # The disposable release pins its own Ed25519 public key. Only this host's
    # acceptance producer receives the matching private key.
    openssl genpkey -algorithm Ed25519 -out "$H/acceptance-private.pem" 2> /dev/null
    chmod 0600 "$H/acceptance-private.pem"
    openssl pkey -in "$H/acceptance-private.pem" -pubout \
        -out "$H/src/deploy/rust/artifact-provenance.pub.pem" 2> /dev/null
    write_fixture_verifier "$H/src/deploy/scripts/verify-artifact.sh"
    git -C "$H/src" init -q -b staging
    git -C "$H/src" add -A
    git -C "$H/src" commit -q -m 'release A'
    A=$(git -C "$H/src" rev-parse HEAD)
    echo 'candidate' > "$H/src/CANDIDATE"
    git -C "$H/src" add CANDIDATE
    git -C "$H/src" commit -q -m 'release B'
    B=$(git -C "$H/src" rev-parse HEAD)
    git clone -q --bare "$H/src" "$H/origin.git"
    git -C "$H/src" remote add origin "$H/origin.git"
    git clone -q -b staging "$H/origin.git" "$H/host/backend"
    BE="$H/host/backend"
    git -C "$BE" reset -q --hard "$A"
    if [[ $ENVIRONMENT_NAME == production ]]; then
        app_role=verdaxis_app database=verdaxis_prod
    else
        app_role=verdaxis_app_staging database=verdaxis_staging
    fi
    cat > "$BE/.env" << EOF
ENVIRONMENT=$ENVIRONMENT_NAME
RELEASE_SHA=$B
DATABASE_URL=postgresql+asyncpg://$app_role:app-pw@127.0.0.1:5432/$database
MIGRATOR_DATABASE_URL=postgresql+asyncpg://verdaxis_migrator:MIGRATOR-SECRET@127.0.0.1:5432/$database
EOF
    chmod 0600 "$BE/.env"
    printf '%s' "$CURRENT_REVISION" > "$H/db-revision"
    health_body "$B"
    for input in archive record signature; do echo "$input" > "$H/$input"; done
    : > "$H/calls.log"
    touch "$H/python-active" "$H/python-enabled"
    write_stubs
}

# Both Rust readiness surfaces return exactly the four deployed keys.
health_body() {
    printf '{"status":"ok","db":"ok","environment":"%s","release_sha":"%s"}' \
        "$ENVIRONMENT_NAME" "$1" > "$H/health-body"
}

# push_release: a further commit on origin's staging; B becomes it and the
# deployed .env names it, as the operator does before a deploy.
push_release() {
    echo "release $RANDOM" >> "$H/src/CANDIDATE"
    git -C "$H/src" commit -q -am 'next release'
    git -C "$H/src" push -q origin staging
    B=$(git -C "$H/src" rev-parse HEAD)
    sed -i "s/^RELEASE_SHA=.*/RELEASE_SHA=$B/" "$BE/.env"
    health_body "$B"
}

# The fixture's artifact verifier: it records its arguments and extracts the
# recording fake binary; an injected failure refuses like a bad provenance.
write_fixture_verifier() {
    cat > "$1" << 'EOF'
#!/usr/bin/env bash
# Fixture only: record the call and install the recording fake binary.
set -euo pipefail
source "$DEPLOY_HARNESS_DIR/stub-lib.sh"
record "verifier $*"
maybe_fail "verifier"
output=''
while (($#)); do
    [[ $1 == --output-dir ]] && output=$2
    shift 2
done
mkdir -p -- "$output"
cp -- "$DEPLOY_HARNESS_DIR/fake-binary" "$output/verdaxis-read"
chmod 0755 "$output/verdaxis-read"
digest=$(sha256sum "$output/verdaxis-read" | cut -d ' ' -f1)
jq -n --arg digest "$digest" '{binary:{sha256:$digest}}' > "$output/artifact-metadata.json"
if [[ -f $H/tamper-staged ]]; then printf '\n# changed after verification\n' >> "$output/verdaxis-read"; fi
EOF
}

write_stubs() {
    # Every stub records one line. kill-at kills the deploy with SIGKILL (no
    # trap runs, as in a host crash); fail-at makes the named call fail;
    # hold-at keeps the named call waiting while $H/hold exists.
    cat > "$H/stub-lib.sh" << EOF
H='$H'
BE='$BE'
ENVIRONMENT_NAME=\$(cat "\$H/environment")
record() { printf '%s\n' "\$*" >> "\$H/calls.log"; }
matches() { [[ -f "\$H/\$1" ]] && [[ "\$2" == *"\$(cat "\$H/\$1")"* ]]; }
maybe_kill() {
    if matches kill-at "\$*"; then
        rm -f -- "\$H/kill-at"
        record "KILLED at: \$*"
        kill -KILL "\$(cat "\$H/deploy.pid")"
        exit 1
    fi
}
maybe_fail() {
    if matches fail-at "\$*"; then
        record "FAILED at: \$*"
        exit 3
    fi
}
maybe_hold() {
    if matches hold-at "\$*"; then
        record "HELD at: \$*"
        while [[ -f "\$H/hold" ]]; do sleep 0.1; done
    fi
}
locks_held() {
    local dir="\$BE/.runtime-deploy"
    ! flock -n "\$dir/\$ENVIRONMENT_NAME.lock" true 2> /dev/null \\
        && ! flock -n "\$dir/\$ENVIRONMENT_NAME.restart.lock" true 2> /dev/null
}
state_phase() {
    sed -n 's/^DEPLOYMENT_STATE=//p' "\$BE/.runtime-deploy/\$ENVIRONMENT_NAME.state" 2> /dev/null || true
}
EOF
    cat > "$H/bin/systemctl" << EOF
#!/usr/bin/env bash
source '$H/stub-lib.sh'
case "\$*" in
    restart*)
        if locks_held; then locks=held; else locks=free; fi
        record "systemctl \$* locks=\$locks state=\$(state_phase)"
        maybe_kill "systemctl \$*"
        maybe_fail "systemctl \$*"
        ;;
    "show --property=LoadState --value "*)
        record "systemctl \$*"
        if matches unloaded "\$*"; then echo not-found; else echo loaded; fi
        ;;
    stop*verdaxis-backend*)
        record "systemctl \$*"
        rm -f "\$H/python-active"
        ;;
    disable*verdaxis-backend*)
        record "systemctl \$*"
        rm -f "\$H/python-enabled"
        ;;
    is-active*verdaxis-backend*)
        record "systemctl \$*"
        [[ -f \$H/python-active ]]
        exit \$?
        ;;
    is-enabled*verdaxis-backend*)
        record "systemctl \$*"
        [[ -f \$H/python-enabled ]]
        exit \$?
        ;;
    *) record "systemctl \$*" ;;
esac
EOF
    cat > "$H/bin/sudo" << EOF
#!/usr/bin/env bash
source '$H/stub-lib.sh'
[[ \$1 == -n ]] || exit 1
shift
if [[ \$1 == -l ]]; then
    shift
    record "sudo-check \$*"
    if matches deny-sudo "\$*"; then exit 1; fi
    exit 0
fi
if [[ \$1 == /usr/local/libexec/verdaxis/provision-bundle-reader.sh ]]; then
    record "reader-preflight \$*"
    maybe_fail 'reader-preflight'
    exit 0
fi
exec '$H/bin/systemctl' "\${@:2}"
EOF
    cat > "$H/bin/curl" << EOF
#!/usr/bin/env bash
source '$H/stub-lib.sh'
url=\${*: -1}
record "curl \$url"
maybe_kill "curl \$url"
if [[ \$url == http://127.0.0.1:* ]]; then
    [[ ! -f "\$H/rust-unready" ]] || exit 22
else
    [[ ! -f "\$H/backend-unready" ]] || exit 22
fi
# The disposable acceptance producer runs after the restart, just as an
# operator gathers evidence during the blocked window. It is not the verifier.
if [[ -f "\$H/acceptance-context" && ! -f "\$H/no-acceptance" ]]; then
    IFS=' ' read -r current target nonce sha < "\$H/acceptance-context"
    digest=\$(sha256sum "\$BE/.runtime-rust/verdaxis-read" | cut -d ' ' -f1)
    jq -n --arg environment "\$ENVIRONMENT_NAME" --arg sha "\$sha" \
        --arg current "\$current" --arg target "\$target" --arg nonce "\$nonce" --arg digest "\$digest" \
        '{environment:\$environment, release_sha:\$sha, expected_current:\$current,
          target:\$target, deploy_nonce:\$nonce, binary_sha256:\$digest,
          auth:true, session:true, monitor:true, passive_harm:true}' > "\$H/acceptance.json"
    if [[ -f "\$H/acceptance-filter" ]]; then
        jq "\$(cat "\$H/acceptance-filter")" "\$H/acceptance.json" > "\$H/acceptance.tmp"
        mv "\$H/acceptance.tmp" "\$H/acceptance.json"
    fi
    if [[ -f "\$H/acceptance-stream" ]]; then
        cp "\$H/acceptance.json" "\$H/acceptance.json.copy"
        cat "\$H/acceptance.json.copy" >> "\$H/acceptance.json"
    fi
    openssl pkeyutl -sign -rawin -inkey "\$H/acceptance-private.pem" \
        -in "\$H/acceptance.json" -out "\$H/acceptance.signature"
    if [[ -f "\$H/bad-signature" ]]; then
        printf tampered >> "\$H/acceptance.json"
    fi
    chmod 0600 "\$H/acceptance.json" "\$H/acceptance.signature"
    record 'acceptance record prepared'
fi
cat "\$H/health-body"
EOF
    cat > "$H/bin/systemd-analyze" << EOF
#!/usr/bin/env bash
source '$H/stub-lib.sh'
record "systemd-analyze \$1"
EOF
    cat > "$H/fake-binary" << EOF
#!/usr/bin/env bash
# The recording stand-in for the verified release binary.
source '$H/stub-lib.sh'
where=staged
[[ "\$0" == */.runtime-rust/verdaxis-read ]] && where=active
arguments="\$*"
record "binary(\$where) \${arguments//MIGRATOR-SECRET/<secret>}"
maybe_kill "\$arguments"
maybe_hold "\$arguments"
maybe_fail "\$arguments"
option() {
    local name=\$1 previous=''
    for argument in "\${@:2}"; do
        [[ \$previous == "\$name" ]] && { printf '%s' "\$argument"; return; }
        previous=\$argument
    done
}
case "\$1 \$2" in
    "migrate status")
        expected=\$(option --expected-current "\$@")
        [[ "\$(cat "\$H/db-revision")" == "\$expected" ]] || exit 3
        ;;
    "migrate run")
        current=\$(option --expected-current "\$@")
        target=\$(option --target "\$@")
        nonce=\$(option --deploy-nonce "\$@")
        sha=\$(option --release-sha "\$@")
        printf '%s %s %s %s\\n' "\$current" "\$target" "\$nonce" "\$sha" > "\$H/acceptance-context"
        expected_state=\$(printf 'DEPLOYMENT_STATE=blocked\nENVIRONMENT=%s\nRELEASE_SHA=%s\nDEPLOY_NONCE=%s' "\$ENVIRONMENT_NAME" "\$sha" "\$nonce")
        if [[ "\$(cat "\$BE/.runtime-deploy/\$ENVIRONMENT_NAME.state")" == "\$expected_state" ]] \\
            && [[ \$nonce =~ ^[0-9a-f]{32,128}\$ ]] && locks_held; then
            record 'ownership=deploy-owned'
        else
            record 'ownership=refused'
            exit 3
        fi
        database=\$(cat "\$H/db-revision")
        if [[ \$database == "\$current" ]]; then
            printf '%s' "\$target" > "\$H/db-revision"
            record "migrated \$current -> \$target"
        elif [[ \$database != "\$target" ]]; then
            exit 3
        else
            record "at target \$target"
        fi
        ;;
    "ops cache-authority")
        [[ \$(state_phase) == blocked ]] && locks_held || exit 3
        if [[ \$3 == revoke ]]; then
            echo revoked > "\$H/cache-authority"
        elif [[ \$3 == activate ]]; then
            [[ \$(cat "\$H/cache-authority") == revoked ]] || exit 3
            echo active > "\$H/cache-authority"
        else
            exit 3
        fi
        ;;
    "ops preflight")
        [[ \$(state_phase) == blocked ]] && locks_held || exit 3
        [[ \$(sed -n 's/^RELEASE_SHA=//p' "\$BE/.env") == \$(option --release-sha "\$@") ]] || exit 3
        record 'preflight=blocked-owned-exact-env'
        ;;
    "ops prepare-budget")
        output=\$(option --output "\$@")
        if [[ "\$*" == *--verify* ]]; then
            [[ -f "\$output" && ! -f "\$H/budget-changed" ]] || exit 3
        else
            echo '{"version": 1}' > "\$output"
        fi
        ;;
    "ops prepare-runtime")
        output=\$(option --output-dir "\$@")
        mkdir -p "\$output"
        echo "ENVIRONMENT=\$ENVIRONMENT_NAME" > "\$output/runtime.env"
        ;;
    "guard health")
        environment=\$(option --environment "\$@")
        sha=\$(option --release-sha "\$@")
        jq -e --arg environment "\$environment" --arg sha "\$sha" \\
            '(keys == ["db", "environment", "release_sha", "status"]) and .status == "ok"
             and .db == "ok" and .environment == \$environment and .release_sha == \$sha' \\
            > /dev/null || exit 3
        ;;
esac
EOF
    chmod 0755 "$H/bin/"* "$H/fake-binary"
}

# deploy [--dry-run]: run the real deploy from the checkout with the stubs.
# DEPLOY_EXTRA_ENVIRONMENT holds extra NAME=value words for one run.
deploy() {
    local service=verdaxis-backend-staging.service
    [[ $ENVIRONMENT_NAME == production ]] && service=verdaxis-backend.service
    (
        cd "$BE"
        export PATH="$H/bin:$PATH" DEPLOY_HARNESS_DIR="$H"
        export TARGET_BRANCH=staging DEPLOY_ENVIRONMENT="$ENVIRONMENT_NAME"
        export SERVICE_NAME=$service
        export HEALTH_URL="https://api-$ENVIRONMENT_NAME.example.invalid/health/ready"
        export HOST_PROVENANCE_KEY_PATH="${HOST_KEY:-$H/no-host-key.pem}"
        export RUST_ARTIFACT_ARCHIVE="$H/archive" RUST_PROVENANCE_RECORD="$H/record"
        export RUST_PROVENANCE_SIGNATURE="$H/signature"
        export DEPLOY_ACCEPTANCE_RECORD="$H/acceptance.json" DEPLOY_ACCEPTANCE_SIGNATURE="$H/acceptance.signature"
        export DEPLOY_PUBLICATION_LOG="$H/publications.log" VERDAXIS_DISPOSABLE_DEPLOY=1
        export HEALTH_ATTEMPTS=1 HEALTH_RETRY_DELAY=0
        local word
        for word in ${DEPLOY_EXTRA_ENVIRONMENT:-}; do export "${word?}"; done
        bash scripts/deploy.sh "$@" > "$H/stdout" 2> "$H/stderr" &
        echo $! > "$H/deploy.pid"
        wait $!
    )
    local status=$?
    cat "$H/stdout" "$H/stderr" >> "$H/deploy.log"
    return "$status"
}

# approve <current> <target>: the operator's four approval variables.
approve() {
    export APPROVED_RELEASE_SHA=$B MIGRATION_APPROVED_SOURCE_SHA=$B
    export MIGRATION_EXPECTED_CURRENT_REVISION=$1 MIGRATION_TARGET_REVISION=$2
}

state_file() { printf '%s/.runtime-deploy/%s.state' "$BE" "$ENVIRONMENT_NAME"; }
head_sha() { git -C "$BE" rev-parse HEAD; }

# The line number of the first or last call log line matching a pattern.
first_line() { grep -n -m 1 -E -- "$1" "$H/calls.log" | cut -d: -f1 || true; }
last_line() { grep -n -E -- "$1" "$H/calls.log" | tail -n 1 | cut -d: -f1 || true; }
before() {
    local first second
    first=$(first_line "$1")
    second=$(first_line "$2")
    [[ -n $first && -n $second && $first -lt $second ]]
}
called() { grep -q -E -- "$1" "$H/calls.log"; }
said() { grep -q -F -- "$1" "$H/stdout" "$H/stderr"; }

# A ready host: a dry run of B, then the approval of a transition.
approved_host() {
    new_host "$1" "${2:-staging}" "${3:-}"
    deploy --dry-run
    approve "${4:-$CURRENT_REVISION}" "${5:-sqlx_1}"
}

# The real run was refused before any change: nonzero, the checkout still at
# A, no durable state and no service touched.
refused_before_any_change() {
    ! deploy && [[ $(head_sha) == "$A" && ! -e $(state_file) ]] \
        && ! called '^systemctl (restart|stop|disable|enable)'
}

python_retired_before_migration_guard() {
    local unit=verdaxis-backend-staging.service
    [[ $ENVIRONMENT_NAME == production ]] && unit=verdaxis-backend.service
    called "^systemctl stop $unit\$" && called "^systemctl disable $unit\$" \
        && ! called '^systemctl (restart|enable) verdaxis-backend'
}

python_retired_before_migration() {
    local unit=verdaxis-backend-staging.service
    [[ $ENVIRONMENT_NAME == production ]] && unit=verdaxis-backend.service
    python_retired_before_migration_guard \
        && before "^systemctl stop $unit\$" 'binary\(staged\) migrate run' \
        && before "^systemctl disable $unit\$" 'binary\(staged\) migrate run'
}

case_reader_preflight() {
    local environment
    for environment in staging production; do
        approved_host "reader-missing-$environment" "$environment"
        echo reader-preflight > "$H/fail-at"
        refused_before_any_change && called '^reader-preflight ' || return 1
    done
}
check missing_bundle_reader_refuses_before_any_stop case_reader_preflight

# ---- part 1: the Task 7 cases -----------------------------------------------------------

echo '== deploy transaction'

case_no_python() {
    # `legacy-python` is the name of a watchlist event producer (a recorded
    # data value), not an interpreter.
    ! grep -n -i -E 'python|\.py\b|\bpip\b|venv' scripts/deploy.sh | grep -v -F 'legacy-python'
}
check deploy_script_has_no_python_or_pip_reference case_no_python

case_python_retired_before_source() {
    local stop disable source
    stop=$(grep -n -m 1 -F 'sudo -n systemctl stop "$SERVICE_NAME"' scripts/deploy.sh | cut -d: -f1)
    disable=$(grep -n -m 1 -F 'sudo -n systemctl disable "$SERVICE_NAME"' scripts/deploy.sh | cut -d: -f1)
    source=$(grep -n -m 1 -F 'publication_boundary before source' scripts/deploy.sh | cut -d: -f1)
    [[ -n $stop && -n $disable && -n $source && $stop -lt $source && $disable -lt $source ]] \
        && ! grep -q -F 'sudo -n systemctl restart "$SERVICE_NAME"' scripts/deploy.sh
}
check python_is_stopped_and_disabled_before_source_publication case_python_retired_before_source

case_full_deploy() {
    approved_host happy && deploy && [[ ! -e $(state_file) && $(head_sha) == "$B" ]]
}
check 'a full deploy completes and clears its state' case_full_deploy

check migrate_runs_before_any_restart before 'binary\(staged\) migrate run' '^systemctl restart'

case_acl_order() { before 'binary\(staged\) migrate run' 'acl converge' && before 'acl converge' '^systemctl restart'; }
check acl_convergence_runs_between_migrate_and_restart case_acl_order

case_cache_handoff() {
    called 'ops cache-authority revoke --runtime-dir ' \
        && ! called 'ops cache-authority revoke .*--environment-file' \
        && before 'ops cache-authority revoke' 'ops preflight' \
        && before 'migrate run' 'ops cache-authority activate --apply' \
        && before 'ops prepare-runtime' 'ops cache-authority activate --apply' \
        && before 'ops cache-authority activate --apply' '^systemctl restart' \
        && [[ $(cat "$H/cache-authority") == active ]]
}
check cache_authority_revokes_before_source_and_binds_before_restart case_cache_handoff

case_cache_failure() {
    local operation
    for operation in revoke activate; do
        approved_host "cache-$operation-failed"
        printf 'ops cache-authority %s' "$operation" > "$H/fail-at"
        ! deploy && grep -q '^DEPLOYMENT_STATE=blocked$' "$(state_file)" \
            && ! called '^systemctl restart' \
            && ! called '^systemctl enable verdaxis-backend' || return 1
        if [[ $operation == revoke ]]; then
            [[ $(head_sha) == "$A" ]] || return 1
        else
            [[ $(cat "$H/cache-authority") == revoked ]] || return 1
        fi
    done
}

case_staged_only() {
    # Every binary call before the active copy exists is the verified staged
    # binary; the active binary is used only after the last staged call.
    local last_staged first_active
    last_staged=$(last_line 'binary\(staged\)')
    first_active=$(first_line 'binary\(active\)')
    [[ -n $last_staged && -n $first_active && $last_staged -lt $first_active ]] \
        && ! called 'python|venv'
}
check staged_binary_is_the_only_artifact_executed_before_install case_staged_only

case_locks_before_restart() {
    called '^systemctl restart verdaxis-rust-staging.service locks=held state=restart-authorized$' \
        && ! called '^systemctl restart .* locks=free'
}
check main_and_restart_locks_precede_authorization case_locks_before_restart

case_ownership() { called '^ownership=deploy-owned$' && ! called '^ownership=refused$'; }
check deploy_owned_migration_accepts_only_its_blocked_state_and_held_locks case_ownership
check cache_authority_failure_blocks_activation case_cache_failure

case_binding() {
    approved_host adoption staging "$ADOPTION_ROW" "$CURRENT_REVISION" sqlx_1 && deploy \
        && called "migrate status .*--check --expected-current $CURRENT_REVISION" \
        && called "migrate run .*--expected-current $CURRENT_REVISION --target sqlx_1 .*--apply" \
        && before 'migrate run' 'migrate status .*--expected-current sqlx_1' \
        && grep -q '^MIGRATION_REVISION=sqlx_1$' "$BE/.runtime-release.env" \
        && jq -e '.revision == "sqlx_1"' "$BE/.runtime-rust/revision.json" > /dev/null \
        && [[ $(cat "$H/db-revision") == sqlx_1 ]] && ! called 'migrate baseline'
}
check dry_run_real_run_and_postcheck_bind_the_same_exact_transition case_binding

case_failure_after_migrate() {
    approved_host acl-failure
    echo 'acl converge' > "$H/fail-at"
    ! deploy && grep -q '^DEPLOYMENT_STATE=blocked$' "$(state_file)" \
        && called 'migrate run' && ! called '^systemctl restart'
}
check failure_after_migrate_leaves_durable_blocked_state case_failure_after_migrate

case_interruptions() {
    # A crash right after each publication (the blocked state and release
    # identity; the adoption commit; the revision record; the active binary;
    # the restart authorization) leaves durable state, the real start guard
    # denies a backend start, and a rerun with the same approval resumes.
    local ok=0 crash=0 point
    for point in 'ops preflight' 'acl converge' 'ops prepare-runtime' '--verify' \
        'restart verdaxis-rust-staging'; do
        crash=$((crash + 1))
        approved_host "crash-$crash" staging "$ADOPTION_ROW" "$CURRENT_REVISION" sqlx_1
        printf '%s' "$point" > "$H/kill-at"
        deploy
        if ! called '^KILLED at' || [[ ! -f $(state_file) ]]; then
            echo "    no durable state after a crash at: $point" >&2
            ok=1
            continue
        fi
        if bash "$BE/deploy/scripts/verify-deploy-start-guard.sh" --environment staging \
            --expected-release-sha "$B" 2> /dev/null; then
            echo "    the start guard allowed a start after a crash at: $point" >&2
            ok=1
        fi
        if ! deploy || [[ -f $(state_file) ]] || [[ $(cat "$H/db-revision") != sqlx_1 ]]; then
            echo "    a rerun did not resume after a crash at: $point" >&2
            ok=1
        fi
    done
    return "$ok"
}
check interruption_at_each_state_identity_revision_and_binary_publication_fails_closed case_interruptions

case_publication_kills() {
    local name side
    for name in cache-revoke source release monitor policy budget revision runtime cache-bind binary authorization clear; do
        for side in before after; do
            approved_host "publication-$side-$name"
            DEPLOY_EXTRA_ENVIRONMENT="DEPLOY_KILL_POINT=$side:$name" deploy && return 1
            grep -q "^$side $name staging $B$" "$H/publications.log" || return 1
            # A clear that has completed is the only safe absent-state point.
            if [[ $side == after && $name == clear ]]; then
                [[ ! -e $(state_file) ]] || return 1
                continue
            fi
            [[ -f $(state_file) ]] || return 1
            flock -n "$BE/.runtime-deploy/staging.lock" true \
                && flock -n "$BE/.runtime-deploy/staging.restart.lock" true || return 1
            if bash "$BE/deploy/scripts/verify-deploy-start-guard.sh" --environment staging \
                --expected-release-sha "$B" 2> /dev/null; then return 1; fi
            deploy && [[ ! -e $(state_file) && $(head_sha) == "$B" ]] \
                && [[ $(cat "$H/db-revision") == sqlx_1 ]] \
                && ! called '^systemctl restart verdaxis-backend' || return 1
        done
    done
}
check publication_boundary_kills_keep_guarded_state_and_resume case_publication_kills

case_exact_readiness() {
    approved_host extra-key
    printf '{"status":"ok","db":"ok","environment":"staging","release_sha":"%s","extra":1}' "$B" > "$H/health-body"
    ! deploy && [[ -f $(state_file) ]] && called 'guard health'
}
check readiness_requires_the_exact_four_key_body case_exact_readiness

case_signed_acceptance() {
    approved_host acceptance-ok && deploy && [[ ! -e $(state_file) ]] \
        && called '^acceptance record prepared$' \
        && ! grep -q -E 'MIGRATOR-SECRET|DATABASE_URL|MIGRATOR_DATABASE_URL' "$H/publications.log"
}
check signed_window_acceptance_follows_readiness_before_state_clear case_signed_acceptance

case_acceptance_binding() {
    local filter count=0
    for filter in '.environment = "production"' '.release_sha = "wrong"' \
        '.deploy_nonce = "wrong"' '.expected_current = "wrong"' '.target = "wrong"' \
        '.binary_sha256 = "wrong"' '.auth = false' '.session = false' \
        '.monitor = false' '.passive_harm = false' '.auth = "true"' \
        'del(.session)' '.extra = true'; do
        count=$((count + 1))
        approved_host "acceptance-binding-$count"
        printf '%s' "$filter" > "$H/acceptance-filter"
        ! deploy && grep -q '^DEPLOYMENT_STATE=blocked$' "$(state_file)" \
            && called '^acceptance record prepared$' && called '^systemctl stop verdaxis-rust-staging.service$' \
            && called '^systemctl disable verdaxis-rust-staging.service$' \
            && ! called '^systemctl restart verdaxis-backend' || return 1
    done
}
check signed_acceptance_binds_every_transaction_field_and_true_result case_acceptance_binding

case_acceptance_missing_or_unsigned() {
    local kind
    for kind in missing signature stream; do
        approved_host "acceptance-$kind"
        case $kind in
            missing) touch "$H/no-acceptance" ;;
            signature) touch "$H/bad-signature" ;;
            stream) touch "$H/acceptance-stream" ;;
        esac
        ! deploy && grep -q '^DEPLOYMENT_STATE=blocked$' "$(state_file)" \
            && called '^systemctl stop verdaxis-rust-staging.service$' \
            && ! called '^systemctl restart verdaxis-backend' || return 1
    done
}
check missing_or_unverified_acceptance_never_clears_state case_acceptance_missing_or_unsigned

case_dependency_change() {
    approved_host requirements
    echo 'example-new-package==1.0' >> "$H/src/requirements.txt"
    git -C "$H/src" commit -q -am 'release B with a new dependency'
    git -C "$H/src" push -q origin staging
    B=$(git -C "$H/src" rev-parse HEAD)
    sed -i "s/^RELEASE_SHA=.*/RELEASE_SHA=$B/" "$BE/.env"
    approve "$CURRENT_REVISION" sqlx_1
    refused_before_any_change && ! called 'migrate run' && said 'installs no dependency'
}
check 'a backend dependency change is refused before any state' case_dependency_change

# ---- part 2: the replaced Python deploy tests (10.4a) --------------------------------------

echo '== replaced Python deploy tests'

# Static gates: the source text keeps every structural rule the Python static
# tests held, restated for the Python-free commands.
case_static_canonical() {
    local s=scripts/deploy.sh
    grep -q -F 'DEPLOY_STATE_DIR="$BACKEND_DIR/.runtime-deploy"' "$s" \
        && grep -q -F 'PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"' "$s" \
        && grep -q -F 'GIT=(safe_root_git "$BACKEND_DIR")' "$s" && grep -q -F 'TARGET_BRANCH="$DEFAULT_BRANCH"' "$s" \
        && grep -q -F 'unset DATABASE_URL MIGRATOR_DATABASE_URL' "$s" \
        && grep -q -F 'export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1' "$s" \
        && grep -q -F 'BINARY_ENV=(/usr/bin/env -i PATH="$PATH")' "$s"
}
check test_live_deploy_uses_canonical_guard_state_and_trusted_tool_path case_static_canonical

# The line number of the first line holding a fixed string in deploy.sh.
source_line() { grep -n -m 1 -F -- "$1" scripts/deploy.sh | cut -d: -f1; }

case_static_dirty_and_archive() {
    local s=scripts/deploy.sh publish
    publish=$(source_line 'write_release_artifact "$CURRENT_SHA"')
    ! grep -q ALLOW_DIRTY "$s" && grep -q 'status --porcelain' "$s" \
        && grep -q -F 'guard health' "$s" && grep -q -F 'archive --format=tar' "$s" \
        && ! grep -q -w clone "$s" && ! grep -q -F 'ln -s' "$s" \
        && (($(grep -n -F 'assert_clean_tree "Selected release"' "$s" | cut -d: -f1) < publish)) \
        && (($(source_line 'assert_release_tree_regular "$CURRENT_SHA"') < publish))
}
check test_deploy_rejects_dirty_release_and_uses_json_health_gate case_static_dirty_and_archive

case_static_guarded_staged_execution() {
    local s=scripts/deploy.sh publish
    publish=$(source_line 'write_release_artifact "$CURRENT_SHA"')
    (($(source_line 'migrate run \') < publish)) \
        && (($(source_line '"$ACL_CONVERGENCE_TEMP/$ACL_CONVERGENCE_HELPER_PATH"') < publish)) \
        && ((publish < $(source_line 'sudo -n systemctl restart "$RUST_UNIT_NAME"'))) \
        && grep -q -F 'verify_binary_digest "$binary"' "$s" \
        && ! grep -q -F 'alembic upgrade head' "$s" && ! grep -q -F 'reset --hard' "$s" \
        && ! grep -q -F '"${GIT[@]}" checkout --' "$s" \
        && approved_host guarded-staged && deploy \
        && called '^preflight=blocked-owned-exact-env$' && called '^ownership=deploy-owned$' \
        && grep -q "^RELEASE_SHA=$B$" "$BE/.runtime-release.env" \
        && ! called '^systemctl restart verdaxis-backend'
}
check test_deploy_guards_verified_staged_execution_and_publishes_before_restart case_static_guarded_staged_execution

case_static_no_head_traversal() {
    local s=scripts/deploy.sh
    ! grep -q -F 'alembic upgrade head' "$s" && grep -q MIGRATION_APPROVED_SOURCE_SHA "$s" \
        && grep -q MIGRATION_EXPECTED_CURRENT_REVISION "$s" && grep -q MIGRATION_TARGET_REVISION "$s" \
        && grep -q 'MIGRATION_REVISION=' "$s" && grep -q -F 'migrate run \' "$s" \
        && grep -q -F 'migrate status --check' "$s"
}
check test_deploy_contains_no_unconditional_head_traversal case_static_no_head_traversal

case_static_acl_order() {
    local s=scripts/deploy.sh
    (($(source_line 'migrate run \') < $(source_line '"$ACL_CONVERGENCE_TEMP/$ACL_CONVERGENCE_HELPER_PATH"'))) \
        && (($(source_line '"$ACL_CONVERGENCE_TEMP/$ACL_CONVERGENCE_HELPER_PATH"') < $(source_line 'sudo -n systemctl restart "$RUST_UNIT_NAME"'))) \
        && grep -q -F deploy/postgres/converge_runtime_object_acls.sql "$s" \
        && grep -q -F deploy/postgres/app_acl_policy.sql "$s" && ! grep -q bootstrap_roles.sql "$s"
}
check test_deploy_converges_acl_policy_after_migration_before_restart case_static_acl_order

case_static_unit_paths() {
    local s=scripts/deploy.sh
    grep -q -F 'RUST_UNIT_NAME="verdaxis-rust-staging.service"' "$s" \
        && grep -q -F 'RUST_RUNTIME_DIR="$BACKEND_DIR/.runtime-rust"' "$s" \
        && grep -q -F 'ACTIVE_BINARY="$RUST_RUNTIME_DIR/verdaxis-read"' "$s" \
        && grep -q -F -- '--output "$RUST_RUNTIME_DIR/budget.json"' "$s"
}
check test_rust_unit_paths_are_the_files_the_deploy_transaction_writes case_static_unit_paths

case_static_staging_ref() {
    [[ $(grep -c -F 'RUST_WORKFLOW_REF="refs/heads/staging"' scripts/deploy.sh) == 1 ]] \
        && ! grep -q -F 'refs/heads/prod' scripts/deploy.sh
}
check test_production_artifact_ref_is_the_staging_push_and_staging_is_unchanged case_static_staging_ref

case_staged_digest_changed() {
    approved_host staged-digest-changed
    touch "$H/tamper-staged"
    refused_before_any_change && ! called '^binary' && said 'Executable bytes differ from the verified artifact'
}
check changed_staged_binary_digest_refuses_before_execution case_staged_digest_changed

# Dry run.
case_dry_run_attests() {
    new_host dry-run
    # A candidate guard that would leave a marker if anything executed it.
    printf '#!/usr/bin/env bash\ntouch %q\n' "$H/candidate-executed" \
        > "$H/src/deploy/scripts/verify-deploy-start-guard.sh"
    git -C "$H/src" commit -q -am 'instrumented candidate guard'
    git -C "$H/src" push -q origin staging
    B=$(git -C "$H/src" rev-parse HEAD)
    deploy --dry-run && [[ $(head_sha) == "$A" ]] && [[ ! -e $H/candidate-executed ]] \
        && [[ ! -e $BE/.runtime-release.env && ! -e $BE/.runtime-deploy ]] \
        && ! called '^binary' && ! called '^verifier' && said "APPROVED_RELEASE_SHA=$B" \
        && said 'No candidate code was executed' && said 'source update' \
        && said 'candidate application preflight' && said 'migration upgrade' \
        && said 'service restart' && said 'health gate' \
        && said 'runtime compatibility remains untested'
}
check test_deploy_dry_run_only_attests_pinned_archive_without_candidate_execution case_dry_run_attests
check test_dry_run_never_executes_build_code case_dry_run_attests

case_dry_run_symlinks() {
    local ok=0 relative count=0
    for relative in deploy/systemd/runtime-units.manifest deploy/migration-checkpoints.tsv \
        deploy/systemd/verdaxis-backend-staging.service deploy/scripts/converge-acls.sh \
        deploy/scripts/verify-deploy-start-guard.sh; do
        count=$((count + 1))
        new_host "symlink-$count"
        echo 'OPERATOR_SECRET=must-not-be-read' > "$H/operator-secret"
        rm -f -- "$H/src/$relative"
        ln -s "$H/operator-secret" "$H/src/$relative"
        git -C "$H/src" add -A
        git -C "$H/src" commit -q -m 'candidate symlink'
        git -C "$H/src" push -q origin staging
        if deploy --dry-run || ! said 'regular committed files' || said OPERATOR_SECRET \
            || [[ $(head_sha) != "$A" ]]; then
            echo "    a symlinked $relative was not refused" >&2
            ok=1
        fi
    done
    return "$ok"
}
check test_deploy_dry_run_rejects_candidate_symlink_artifacts case_dry_run_symlinks

case_dry_run_malformed_policy() {
    new_host malformed-policy
    printf 'head\trh\textra\n' > "$H/src/deploy/migration-checkpoints.tsv"
    git -C "$H/src" commit -q -am 'malformed checkpoint policy'
    git -C "$H/src" push -q origin staging
    ! deploy --dry-run && said 'Invalid migration checkpoint policy' \
        && [[ $(head_sha) == "$A" && ! -e $(state_file) ]]
}
check test_deploy_dry_run_rejects_malformed_checkpoint_policy case_dry_run_malformed_policy

case_dry_run_producer_keys() {
    new_host producer-dry-run
    printf 'WATCHLIST_EVENT_PRODUCER=rust-market-v1\nWATCHLIST_PRODUCER_ACTIVATED_AT=2026-11-01T00:00:00Z\n' \
        > "$BE/.runtime-release.env"
    DEPLOY_EXTRA_ENVIRONMENT='WATCHLIST_EVENT_PRODUCER=legacy-python' deploy --dry-run \
        && said 'WATCHLIST_EVENT_PRODUCER recorded=rust-market-v1 supplied=legacy-python' \
        && said 'WATCHLIST_PRODUCER_ACTIVATED_AT recorded=2026-11-01T00:00:00Z supplied=<absent>'
}
check test_dry_run_prints_the_producer_keys case_dry_run_producer_keys

case_dry_run_checkpoint_pair() {
    new_host dry-pair
    MIGRATION_EXPECTED_CURRENT_REVISION=$CURRENT_REVISION MIGRATION_TARGET_REVISION=sqlx_1 deploy --dry-run \
        && said "APPROVED_MIGRATION_CURRENT=$CURRENT_REVISION" && said 'APPROVED_MIGRATION_TARGET=sqlx_1' || return 1
    MIGRATION_EXPECTED_CURRENT_REVISION=$CURRENT_REVISION MIGRATION_TARGET_REVISION='' deploy --dry-run && return 1
    said 'both literal migration current and target' || return 1
    MIGRATION_EXPECTED_CURRENT_REVISION='' MIGRATION_TARGET_REVISION=sqlx_1 deploy --dry-run && return 1
    said 'both literal migration current and target' || return 1
    MIGRATION_EXPECTED_CURRENT_REVISION=$CURRENT_REVISION MIGRATION_TARGET_REVISION=head deploy --dry-run && return 1
    [[ $(head_sha) == "$A" && ! -e $(state_file) ]] && ! called '^binary'
}
check dry_run_attests_optional_exact_checkpoint_pair_without_candidate_execution case_dry_run_checkpoint_pair

# Refusals before any source change or state.
case_missing_approval() {
    new_host missing-approval
    export APPROVED_RELEASE_SHA=$B
    unset MIGRATION_APPROVED_SOURCE_SHA MIGRATION_EXPECTED_CURRENT_REVISION MIGRATION_TARGET_REVISION
    refused_before_any_change && said MIGRATION_APPROVED_SOURCE_SHA
}
check test_real_deploy_refuses_missing_migration_checkpoint_before_source_mutation case_missing_approval

case_symbolic_target() {
    approved_host symbolic
    export MIGRATION_TARGET_REVISION=HEAD
    refused_before_any_change && said 'literal migration'
}
check test_real_deploy_refuses_symbolic_checkpoint_before_source_mutation case_symbolic_target

case_moved_remote() {
    approved_host moved
    export APPROVED_RELEASE_SHA=$A MIGRATION_APPROVED_SOURCE_SHA=$A
    refused_before_any_change && said 'moved after approval'
}
check test_real_deploy_refuses_moved_remote_before_source_mutation case_moved_remote
check test_moved_sha_refuses_before_source_mutation case_moved_remote

case_unallowlisted() {
    approved_host unallowlisted
    export MIGRATION_TARGET_REVISION=pref_20260709_user_preferences
    refused_before_any_change && said 'exact offboarding adoption or sqlx_1 self-transition'
}
check test_real_deploy_refuses_unallowlisted_checkpoint_before_source_mutation case_unallowlisted

case_unexpected_revision() {
    approved_host unexpected-revision
    printf 'p5_20261020_read_indexes' > "$H/db-revision"
    refused_before_any_change \
        && called "migrate status .*--check --expected-current $CURRENT_REVISION" \
        && (($(grep -n -m 1 -x 'acquire_deploy_lock' scripts/deploy.sh | cut -d: -f1) < $(source_line 'migrate status --check \'))) \
        && (($(source_line 'migrate status --check \') < $(source_line 'write_deploy_state "blocked" "$APPROVED_RELEASE_SHA"')))
}
check test_real_deploy_refuses_unexpected_live_revision_before_source_mutation case_unexpected_revision

case_health_controls() {
    # An arithmetic injection attempt; it would create the marker if the
    # deploy evaluated the value before checking it.
    approved_host health-controls
    # The value must reach the deploy unexpanded.
    DEPLOY_EXTRA_ENVIRONMENT='HEALTH_ATTEMPTS=1+$(touch${IFS}'"$H/injected"')' refused_before_any_change \
        && [[ ! -e $H/injected ]] && said HEALTH_ATTEMPTS
}
check test_real_deploy_rejects_non_numeric_health_controls_without_execution case_health_controls

case_sudo_denial() {
    approved_host sudo-denied "${1:-staging}"
    echo "${2:-restart}" > "$H/deny-sudo"
    refused_before_any_change && said 'authority is unavailable'
}
check test_production_sudo_denial_refuses_before_source_or_state case_sudo_denial production restart
check test_missing_enable_authority_refuses_before_source_or_restart case_sudo_denial staging enable

case_missing_provenance() {
    approved_host no-provenance
    DEPLOY_EXTRA_ENVIRONMENT='RUST_PROVENANCE_RECORD=' refused_before_any_change \
        && said 'provenance record and signature'
}
check test_missing_record_and_signature_refuses_before_any_stop case_missing_provenance

case_divergent_host_key() {
    approved_host divergent-key
    echo 'another key' > "$H/host-key.pem"
    HOST_KEY="$H/host-key.pem" refused_before_any_change && said 'provenance public key'
}
check test_divergent_host_key_refuses_before_any_stop case_divergent_host_key

case_matching_host_key() {
    approved_host matching-key
    cp -- "$BE/deploy/rust/artifact-provenance.pub.pem" "$H/host-key.pem"
    HOST_KEY="$H/host-key.pem" deploy \
        && called "^verifier .*--provenance-public-key $H/host-key.pem"
}
check test_host_key_matching_the_pinned_digest_is_preferred case_matching_host_key

case_tree_key() {
    approved_host tree-key && deploy \
        && called "^verifier .*--provenance-public-key $BE/deploy/rust/artifact-provenance.pub.pem"
}
check test_missing_host_key_file_falls_back_to_tree_key case_tree_key

case_verifier_binding() {
    # The verifier refuses another ref or commit (tests/rust_contract/phase10/
    # artifact/contract.py); the deploy always binds the staging push and the
    # exact approved commit, in both environments.
    approved_host verifier-binding "$1" && deploy \
        && called "^verifier .*--source-sha $B .*--workflow-ref refs/heads/staging " \
        && approved_host verifier-refusal "$1" && echo verifier > "$H/fail-at" \
        && refused_before_any_change && said 'Rust artifact verification refused'
}
check test_production_refuses_an_artifact_attested_from_any_other_ref case_verifier_binding production
check test_production_refuses_the_staging_ref_for_a_different_commit case_verifier_binding production
check test_production_refuses_a_staging_artifact_built_for_another_commit case_verifier_binding production

# The Python-only paths are gone by design: a release whose policy does not
# enable Rust, or whose manifest does not select the Rust unit, is refused
# before any state, and an unloaded Rust unit is refused before activation.
case_policy_off() {
    new_host "policy-off-$1" "$1"
    jq '.rust_enabled = false' "$H/src/deploy/rust/$1-policy.json" > "$H/policy.json"
    mv -- "$H/policy.json" "$H/src/deploy/rust/$1-policy.json"
    git -C "$H/src" commit -q -am 'policy off'
    git -C "$H/src" push -q origin staging
    B=$(git -C "$H/src" rev-parse HEAD)
    sed -i "s/^RELEASE_SHA=.*/RELEASE_SHA=$B/" "$BE/.env"
    approve "$CURRENT_REVISION" sqlx_1
    refused_before_any_change && said 'does not enable the Rust service' && ! called '^binary'
}
check test_policy_off_refuses_before_any_service_change case_policy_off staging
check test_policy_off_never_enables_either_runtime case_policy_off staging
check test_policy_off_has_no_mutating_command_ledger case_policy_off staging
check test_production_policy_off_refuses_without_python_fallback case_policy_off production
check test_bootstrap_release_requires_rust_unit_and_artifact case_policy_off staging

case_manifest_without_rust() {
    new_host manifest-without-rust
    grep -v -F 'verdaxis-rust-staging.service' "$H/src/deploy/systemd/runtime-units.manifest" > "$H/manifest"
    mv -- "$H/manifest" "$H/src/deploy/systemd/runtime-units.manifest"
    git -C "$H/src" commit -q -am 'manifest without the Rust unit'
    git -C "$H/src" push -q origin staging
    B=$(git -C "$H/src" rev-parse HEAD)
    sed -i "s/^RELEASE_SHA=.*/RELEASE_SHA=$B/" "$BE/.env"
    approve "$CURRENT_REVISION" sqlx_1
    refused_before_any_change && said 'not manifest-selected'
}
check test_missing_rust_manifest_entry_refuses_before_service_change case_manifest_without_rust

case_activation_requirements() {
    # Production: verify the loaded Rust unit and artifact before mutation;
    # both Rust readiness surfaces must pass after its only restart.
    local first_mutation
    approved_host production-ok production && deploy && [[ ! -e $(state_file) ]] \
        && called '^sudo-check systemctl enable verdaxis-rust.service$' \
        && called '^systemctl show --property=LoadState --value verdaxis-rust.service' \
        && called '^verifier ' && called '^curl http://127.0.0.1:8010/health/ready' || return 1
    first_mutation=$(first_line '^binary\(staged\) migrate run')
    (($(first_line '^verifier ') < first_mutation)) \
        && (($(first_line '^systemctl show') < first_mutation)) \
        && before '^systemctl stop verdaxis-backend.service' '^binary\(staged\) migrate run' \
        && before '^systemctl disable verdaxis-backend.service' '^binary\(staged\) migrate run' \
        && before '^systemctl restart verdaxis-rust.service' 'curl https://api-production' \
        && ! called '^systemctl restart verdaxis-backend' || return 1
    approved_host production-unloaded production && echo verdaxis-rust.service > "$H/unloaded"
    refused_before_any_change || return 1
    approved_host production-unverified production && echo verifier > "$H/fail-at"
    refused_before_any_change || return 1
    approved_host production-unready production && touch "$H/rust-unready"
    ! deploy && grep -q '^DEPLOYMENT_STATE=blocked$' "$(state_file)"
}
check test_production_activation_requires_its_unit_provenance_and_readiness case_activation_requirements
check test_activation_requires_the_manifest_unit_and_a_verified_artifact case_activation_requirements

# Failures after the source update keep code and identity aligned.
case_failure_alignment() {
    local ok=0 point count=0
    for point in 'ops preflight' 'ops prepare-budget' 'migrate run' 'acl converge' \
        'ops prepare-runtime'; do
        count=$((count + 1))
        approved_host "aligned-$count"
        printf '%s' "$point" > "$H/fail-at"
        if deploy || [[ $(head_sha) != "$B" ]] \
            || ! grep -q '^DEPLOYMENT_STATE=blocked$' "$(state_file)" \
            || ! said 'durable state stays fail-closed' || called '^systemctl restart'; then
            echo "    a failure at $point did not keep code, identity and state aligned" >&2
            ok=1
        fi
        # Before publication a failed staged operation leaves the prior identity;
        # after publication it must match the selected source. Neither may serve.
        if [[ $point == 'ops prepare-runtime' ]]; then
            grep -q "^RELEASE_SHA=$B\$" "$BE/.runtime-release.env" || ok=1
        elif [[ -e $BE/.runtime-release.env ]]; then
            echo "    identity was published before migration and ACL completion" >&2
            ok=1
        fi
    done
    return "$ok"
}
check test_deploy_failure_keeps_selected_code_and_identity_aligned_and_guarded case_failure_alignment
check test_rust_failure_keeps_selected_code_identity_and_blocked_state case_failure_alignment
check test_failures_retain_blocked_state_and_stop_attempted_services case_failure_alignment

case_post_restart_health() {
    approved_host backend-unready
    touch "$H/backend-unready"
    ! deploy && grep -q '^DEPLOYMENT_STATE=blocked$' "$(state_file)" \
        && grep -q "^RELEASE_SHA=$B\$" "$BE/.runtime-release.env" \
        && (($(first_line '^systemctl restart verdaxis-rust-staging') < $(last_line '^systemctl stop verdaxis-rust-staging'))) \
        && python_retired_before_migration && ! called '^systemctl restart verdaxis-backend' \
        && called "migrate run .*--expected-current $CURRENT_REVISION --target sqlx_1" \
        && before 'migrate run' 'acl converge' && before 'acl converge' '^systemctl restart'
}
check test_failed_post_restart_health_stops_service_and_restores_guard case_post_restart_health

case_changed_budget() {
    approved_host changed-budget
    touch "$H/budget-changed"
    ! deploy && grep -q '^DEPLOYMENT_STATE=blocked$' "$(state_file)" \
        && python_retired_before_migration_guard && ! called '^systemctl restart'
}
check test_changed_budget_refuses_and_blocks case_changed_budget

# Locks and interruption.
case_concurrent() {
    approved_host concurrent
    echo 'ops preflight' > "$H/hold-at"
    touch "$H/hold"
    deploy &
    local first=$!
    local waited=0
    until called '^HELD at' || ((waited > 300)); do
        sleep 0.1
        waited=$((waited + 1))
    done
    local first_pid refused=1
    first_pid=$(cat "$H/deploy.pid")
    if ! deploy && said 'already running' \
        && grep -q '^DEPLOYMENT_STATE=blocked$' "$(state_file)"; then
        refused=0
    fi
    # The first deploy dies holding its lock; its blocked state survives.
    kill -KILL "$first_pid" 2> /dev/null
    rm -f -- "$H/hold"
    wait "$first" || true
    ((refused == 0)) && grep -q '^DEPLOYMENT_STATE=blocked$' "$(state_file)"
}
check test_deploy_lock_serializes_concurrent_runs_and_state_survives_interruption case_concurrent
check test_concurrent_deploy_refuses_while_lock_held case_concurrent

case_kill_in_restart_window() {
    # SIGKILL inside the restart window (during the restart, then during
    # readiness) keeps the authorization or readiness state, frees both
    # locks, and the real start guard then denies a start.
    local point
    for point in 'restart verdaxis-rust-staging' 'curl https://api-staging'; do
        approved_host "window-${point%% *}"
        printf '%s' "$point" > "$H/kill-at"
        deploy
        called '^KILLED at' && [[ -f $(state_file) ]] || return 1
        grep -q -E '^DEPLOYMENT_STATE=(restart-authorized|readiness-pending)$' "$(state_file)" || return 1
        flock -n "$BE/.runtime-deploy/staging.lock" true && flock -n "$BE/.runtime-deploy/staging.restart.lock" true || return 1
        if bash "$BE/deploy/scripts/verify-deploy-start-guard.sh" --environment staging \
            --expected-release-sha "$B" 2> "$H/guard.err"; then
            return 1
        fi
        grep -q '^Backend start denied by deployment guard' "$H/guard.err" || return 1
    done
}
check test_sigkill_in_restart_window_releases_both_locks_and_closes_authorization case_kill_in_restart_window
check test_interrupted_restart_window_releases_locks_and_keeps_state case_kill_in_restart_window

# Rust activation.
case_rust_only_service() {
    approved_host both-services && deploy && [[ ! -e $(state_file) ]] \
        && python_retired_before_migration && ! called '^systemctl restart verdaxis-backend' \
        && [[ $(grep -c '^systemctl restart' "$H/calls.log") == 1 ]] \
        && called 'binary\(staged\) ops prepare-runtime .*--output-dir '"$BE"'/.runtime-rust' \
        && ! called 'prepare-runtime .*MIGRATOR'
}
check test_rust_deploy_restarts_only_rust_and_clears_state case_rust_only_service
check test_runtime_environment_carries_only_the_projected_settings case_rust_only_service

case_enable_once() {
    approved_host enable-once && deploy \
        && [[ $(grep -c '^systemctl enable' "$H/calls.log") == 1 ]] \
        && called '^systemctl enable verdaxis-rust-staging.service$' \
        && (($(first_line '^systemctl restart verdaxis-rust') < $(last_line 'guard health'))) \
        && (($(last_line 'guard health') < $(first_line '^systemctl enable')))
}
check test_rust_deploy_enables_the_unit_once_after_activation case_enable_once

case_loopback_url() { approved_host loopback && deploy && called '^curl http://127.0.0.1:8011/health/ready$'; }
check test_rust_health_uses_loopback_identity_url case_loopback_url

case_rust_failure() {
    # A Rust start or readiness failure blocks and disables Rust. Python stays
    # stopped and disabled through failure, retry and the next release.
    approved_host "rust-$1" "${2:-staging}"
    local unit=verdaxis-rust-staging.service
    [[ ${2:-staging} == production ]] && unit=verdaxis-rust.service
    if [[ $1 == start ]]; then echo "systemctl restart $unit" > "$H/fail-at"; else touch "$H/rust-unready"; fi
    ! deploy && grep -q '^DEPLOYMENT_STATE=blocked$' "$(state_file)" \
        && python_retired_before_migration && ! called '^systemctl restart verdaxis-backend' \
        && called "^systemctl stop $unit\$" && called "^systemctl disable $unit\$" || return 1
    rm -f -- "$H/fail-at" "$H/rust-unready"
    deploy && [[ ! -e $(state_file) ]] || return 1
    push_release
    approve sqlx_1 sqlx_1
    deploy && [[ ! -e $(state_file) && $(head_sha) == "$B" ]] \
        && ! called '^systemctl restart verdaxis-backend'
}
check test_rust_start_failure_blocks_without_python_restart case_rust_failure start
check test_rust_readiness_failure_blocks_and_disables_rust case_rust_failure readiness
check test_public_and_loopback_rust_health_are_required case_rust_failure readiness
check test_retry_after_rust_failure_then_second_release case_rust_failure readiness
check test_production_rust_failure_retry_and_next_release_never_restart_python case_rust_failure readiness production

# The connection budget listeners.
case_listener() {
    approved_host "listener-$1"
    [[ $1 == absent ]] || printf 'RUST_CACHE_LISTENER=%s\n' "$1" >> "$BE/.env"
    deploy && called "ops prepare-budget --cache-listener $2 --stream-listener 1 "
}
check test_deploy_without_the_listener_key_keeps_the_budget_byte_identical case_listener absent 0
check test_deploy_with_the_listener_flag_budgets_the_dedicated_connection case_listener 1 1

case_invalid_listener() {
    local ok=0 value count=0
    for value in 2 yes "'1 '"; do
        count=$((count + 1))
        approved_host "invalid-listener-$count"
        printf 'RUST_CACHE_LISTENER=%s\n' "$value" >> "$BE/.env"
        if deploy || ! said RUST_CACHE_LISTENER || called 'migrate run' || called '^systemctl restart' \
            || ! grep -q '^DEPLOYMENT_STATE=blocked$' "$(state_file)"; then
            echo "    RUST_CACHE_LISTENER=$value was not refused before the migration" >&2
            ok=1
        fi
    done
    return "$ok"
}
check test_deploy_refuses_an_invalid_listener_value_before_schema_change case_invalid_listener

case_stream_listener() {
    approved_host stream-listener && deploy \
        && called 'ops prepare-budget --cache-listener 0 --stream-listener 1 ' || return 1
    approved_host stream-self-loop staging '' sqlx_1 sqlx_1
    printf sqlx_1 > "$H/db-revision"
    deploy && called 'ops prepare-budget --cache-listener 0 --stream-listener 1 '
}
check test_canonical_stream_release_prepares_enabled_budget case_stream_listener

# Release metadata: the watchlist producer keys (unchanged logic, run through
# the Python-free transaction).
release_value() { sed -n "s/^$1=//p" "$BE/.runtime-release.env"; }
INSTANT=2026-11-01T00:00:00Z

case_metadata_forward() {
    approved_host metadata-forward
    printf 'WATCHLIST_EVENT_PRODUCER=rust-market-v1\nWATCHLIST_PRODUCER_ACTIVATED_AT=%s\n' "$INSTANT" \
        > "$BE/.runtime-release.env"
    deploy && said '+ write immutable release metadata' \
        && [[ $(release_value WATCHLIST_EVENT_PRODUCER) == rust-market-v1 ]] \
        && [[ $(release_value WATCHLIST_PRODUCER_ACTIVATED_AT) == "$INSTANT" ]] \
        && [[ $(release_value RELEASE_SHA) == "$B" ]]
}
check test_release_metadata_carries_the_producer_keys_forward case_metadata_forward

case_metadata_supplied() {
    approved_host "metadata-supplied-$1"
    DEPLOY_EXTRA_ENVIRONMENT="WATCHLIST_EVENT_PRODUCER=rust-market-v1 WATCHLIST_PRODUCER_ACTIVATED_AT=$2" deploy \
        && said '+ write immutable release metadata' \
        && [[ $(release_value WATCHLIST_EVENT_PRODUCER) == rust-market-v1 ]] \
        && [[ $(release_value WATCHLIST_PRODUCER_ACTIVATED_AT) == "$2" ]]
}
check test_release_metadata_records_a_supplied_activation case_metadata_supplied plain "$INSTANT"
check test_release_metadata_records_a_utc_instant_with_fraction_and_z case_metadata_supplied fraction 2026-11-01T00:00:00.123456Z

case_metadata_refused() {
    # <label> <reason text> <extra environment words>: refused before any
    # state, and the recorded metadata is unchanged.
    approved_host "metadata-$1"
    printf 'WATCHLIST_EVENT_PRODUCER=rust-market-v1\nWATCHLIST_PRODUCER_ACTIVATED_AT=%s\n' "$INSTANT" \
        > "$BE/.runtime-release.env"
    cp -- "$BE/.runtime-release.env" "$H/metadata-before"
    DEPLOY_EXTRA_ENVIRONMENT=$3 refused_before_any_change && said "$2" \
        && cmp -s "$BE/.runtime-release.env" "$H/metadata-before" \
        && ! said '+ write immutable release metadata'
}
check test_release_metadata_refuses_a_changed_activation_instant case_metadata_refused changed \
    'activation instant never changes' 'WATCHLIST_PRODUCER_ACTIVATED_AT=2026-11-02T00:00:00Z'
check test_release_metadata_refuses_an_instant_that_is_not_a_real_utc_time case_metadata_refused unreal \
    'real UTC time' 'WATCHLIST_PRODUCER_ACTIVATED_AT=2026-02-30T00:00:00Z'

case_metadata_producer_without_instant() {
    approved_host metadata-no-instant
    DEPLOY_EXTRA_ENVIRONMENT='WATCHLIST_EVENT_PRODUCER=rust-market-v1' refused_before_any_change \
        && said 'requires WATCHLIST_PRODUCER_ACTIVATED_AT'
}
check test_release_metadata_refuses_a_producer_without_an_instant case_metadata_producer_without_instant

case_metadata_pause() {
    approved_host metadata-pause
    printf 'WATCHLIST_EVENT_PRODUCER=rust-market-v1\nWATCHLIST_PRODUCER_ACTIVATED_AT=%s\n' "$INSTANT" \
        > "$BE/.runtime-release.env"
    DEPLOY_EXTRA_ENVIRONMENT='WATCHLIST_EVENT_PRODUCER=legacy-python' deploy \
        && [[ $(release_value WATCHLIST_EVENT_PRODUCER) == legacy-python ]] \
        && [[ $(release_value WATCHLIST_PRODUCER_ACTIVATED_AT) == "$INSTANT" ]] || return 1
    push_release
    approve sqlx_1 sqlx_1
    DEPLOY_EXTRA_ENVIRONMENT='WATCHLIST_EVENT_PRODUCER=rust-market-v1' deploy \
        && [[ $(release_value WATCHLIST_EVENT_PRODUCER) == rust-market-v1 ]] \
        && [[ $(release_value WATCHLIST_PRODUCER_ACTIVATED_AT) == "$INSTANT" ]]
}
check test_release_metadata_allows_pause_and_reactivation_with_the_same_instant case_metadata_pause

case_metadata_below_checkpoint() {
    approved_host metadata-below staging '' p3_20261020_auth_tokens p3_20261020_auth_tokens
    printf 'p3_20261020_auth_tokens' > "$H/db-revision"
    printf 'WATCHLIST_EVENT_PRODUCER=rust-market-v1\nWATCHLIST_PRODUCER_ACTIVATED_AT=%s\n' "$INSTANT" \
        > "$BE/.runtime-release.env"
    cp -- "$BE/.runtime-release.env" "$H/metadata-before"
    refused_before_any_change && cmp -s "$BE/.runtime-release.env" "$H/metadata-before"
}
check test_release_metadata_drops_producer_keys_below_the_producer_checkpoint case_metadata_below_checkpoint

# Dependency installation is gone; its gate is the dependency change refusal.
check test_real_deploy_refuses_unconstrained_dependency_install case_dependency_change

# Every removed Python deploy test names the case that replaces it; a case that
# did not run and pass leaves its gate unmapped (plan section 10.4a).
echo '== gate map'
while IFS=$'\t' read -r python_test harness_case; do
    [[ -z $python_test || $python_test == \#* ]] && continue
    if [[ " ${passed_cases[*]} " != *" $harness_case "* ]]; then
        fail "unmapped gate: $python_test"
    fi
done < deploy/tests/deploy-gate-map.tsv
echo "  $(grep -c -v '^#' deploy/tests/deploy-gate-map.tsv) removed Python tests mapped"

if ((failures > 0)); then
    echo "DEPLOY_TRANSACTION=failed ($failures)" >&2
    exit 1
fi
echo 'DEPLOY_TRANSACTION=ok'
