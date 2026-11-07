#!/usr/bin/env bash
# Disposable native transaction tests. All host commands are fixture adapters.
set -euo pipefail
umask 077
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
script=$ROOT/deploy/scripts/pre-m-bundle.sh
[[ -f $script ]] || { printf 'FAIL: native bundle entrypoint missing\n' >&2; exit 1; }
[[ $(uname -s) == Linux ]] || { printf 'Linux fixture required\n' >&2; exit 2; }
work=$(mktemp -d /var/lib/verdaxis-bundle-fixture.XXXXXXXX)
trap 'rm -rf "$work"' EXIT
bash "$script" capture >"$work/out" 2>"$work/err" && exit 1
grep -q NO_GO "$work/err"
printf 'ok incomplete capture refuses\n'
mkdir -p "$work/bin" "$work/trusted/scripts" "$work/trusted/rust" "$work/private"
chmod 0700 "$work" "$work/private"
# Replace only the trusted PATH in the disposable copy so adapter commands
# cannot become a runtime option or ambient production override.
perl -pe 's@^export PATH=.*$@export PATH='"$work"'/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin@' "$script" > "$work/trusted/scripts/pre-m-bundle.sh"
cp "$ROOT/deploy/scripts/safe-root-git.sh" "$work/trusted/scripts/"
cp "$ROOT/deploy/rust/catalog-fingerprint.sql" "$work/trusted/rust/"
script=$work/trusted/scripts/pre-m-bundle.sh
# Record the transaction parent only in the private fixture copy.
# The injected script must expand these variables in its own process.
# shellcheck disable=SC2016
printf '\nprintf "%%s\\n" "$$" > "$FIXTURE_WORK/transaction.pid"\n' > "$work/record-parent"
perl -0777 -i -pe 'BEGIN { open my $in, "<", $ARGV[1] or die $!; local $/; $insert=<$in>; pop @ARGV; } s/(umask 077\n)/$1$insert/' "$script" "$work/record-parent"
checkout=/home/verdaxis-prod/verdaxis/staging/be
[[ ! -e $checkout ]] || { printf 'fixture checkout path occupied\n' >&2; exit 2; }
mkdir -p "$checkout/.runtime-deploy" "$checkout/scripts" "$checkout/deploy/scripts" "$checkout/deploy/rust" "$checkout/.runtime-rust" "$work/installed" "$work/package"
printf 'guard bytes\n' > "$checkout/deploy/scripts/verify-deploy-start-guard.sh"
printf '{"environment":"staging","rust_enabled":true}\n' > "$checkout/deploy/rust/staging-policy.json"
openssl genpkey -algorithm ED25519 -out "$work/private/acceptance-key" 2>/dev/null
openssl pkey -in "$work/private/acceptance-key" -pubout -out "$work/private/acceptance-pub" 2>/dev/null
key_sha=$(sha256sum "$work/private/acceptance-pub" | cut -d ' ' -f 1)
# Installed effective unit properties are bound independently in the inventory.
printf 'FragmentPath=%s/unit\nExecStart=%s/.runtime-rust/verdaxis-read\n' "$work/installed" "$checkout" > "$work/consumer-properties"
consumer_sha=$(sha256sum "$work/consumer-properties" | cut -d ' ' -f 1)
closure_sha=$(printf fixture-closure | sha256sum | cut -d ' ' -f 1)
native_dep=/lib/ld-musl-$(uname -m).so.1
[[ -f $native_dep && ! -L $native_dep ]] || native_dep=$(find /usr/lib /lib -type f -name 'libc.so.6' -print -quit)
native_sha=$(sha256sum "$native_dep" | cut -d ' ' -f 1)
jq -n --arg sha placeholder --arg closure "$closure_sha" '{environment:"staging",release_sha:$sha,dependency_closure_sha256:$closure,runtime_dependencies:"PASS",credential_sources:"PASS",unit_inputs:"PASS"}' > "$work/private/closure"
openssl pkeyutl -sign -rawin -inkey "$work/private/acceptance-key" -in "$work/private/closure" -out "$work/private/closure-sig"
trap 'status=$?; if (( status != 0 )); then cat "$work/err" >&2; fi; if [[ -f $work/old.pid ]]; then kill "$(< "$work/old.pid")" 2>/dev/null || true; fi; rm -rf "$work" "$checkout"' EXIT
export GIT_AUTHOR_NAME=fixture GIT_AUTHOR_EMAIL=fixture@example.invalid
export GIT_COMMITTER_NAME=fixture GIT_COMMITTER_EMAIL=fixture@example.invalid
git -C "$checkout" init -q -b staging
printf 'old source\n' > "$checkout/source.txt"
printf 'old helper\n' > "$checkout/scripts/deploy.sh"
git -C "$checkout" add .
git -C "$checkout" commit -qm old
sha=$(git -C "$checkout" rev-parse HEAD)
jq --arg sha "$sha" '.release_sha=$sha' "$work/private/closure" > "$work/private/closure-new"
mv "$work/private/closure-new" "$work/private/closure"
openssl pkeyutl -sign -rawin -inkey "$work/private/acceptance-key" -in "$work/private/closure" -out "$work/private/closure-sig"
: > "$checkout/.runtime-deploy/staging.lock"
: > "$checkout/.runtime-deploy/staging.restart.lock"
chmod 0600 "$checkout/.runtime-deploy/"*.lock
cat > "$work/old.c" <<'OLD'
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#ifndef RELEASE_SHA
#error RELEASE_SHA required
#endif
int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "--version-json") == 0) {
        puts("{\"release_sha\":\"" RELEASE_SHA "\",\"migration_revision_allowlist\":[\"final_head\"]}");
        return 0;
    }
    for (;;) pause();
}
OLD
cc -O2 "-DRELEASE_SHA=\"$sha\"" "$work/old.c" -o "$work/installed/binary"
chmod 0700 "$work/installed/binary"
cat > "$work/trusted/scripts/verify-artifact.sh" <<'VERIFY'
#!/usr/bin/env bash
set -euo pipefail
while (( $# )); do
    case $1 in --archive) archive=$2 ;; --output-dir) output=$2 ;; esac
    shift 2
done
cp "$archive" "$output/verdaxis-read"
chmod 0700 "$output/verdaxis-read"
VERIFY
cat > "$work/bin/psql" <<'PSQL'
#!/usr/bin/env bash
printf 'DATABASE:verdaxis_staging:verdaxis_bundle_reader_staging:127.0.0.1:5432\nALEMBIC:final_head\ncatalog fixture\n'
PSQL
cat > "$work/bin/systemctl" <<'SYSTEMCTL'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$FIXTURE_WORK/calls"
boundary="$1:${2:-}"
if [[ ${KILL_SERVICE_BOUNDARY:-} == "before:$boundary" ]]; then kill -KILL "$(cat "$FIXTURE_WORK/transaction.pid")"; exit 1; fi
case $1 in
    is-active) exit 0 ;;
    show)
        case $4 in
            FragmentPath) cat "$FIXTURE_WORK/consumer-properties" ;;
            MainPID) cat "$FIXTURE_WORK/old.pid" ;;
            InvocationID) printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n' ;;
            Result) printf 'success\n' ;;
            ActiveState)
                if [[ -f $FIXTURE_WORK/active-$2 ]]; then printf 'active\n'; else printf 'inactive\n'; fi ;;
            *) printf 'inactive\n' ;;
        esac ;;
    start)
        if [[ ${KILL_TIMER_START:-} == 1 ]]; then kill -KILL "$PPID"; exit 1; fi ;;
    stop) rm -f "$FIXTURE_WORK/active-$2" ;;
    enable) : > "$FIXTURE_WORK/enabled-$2" ;;
    restart)
        for legacy_environment in staging production; do
            legacy_file=$FIXTURE_WORK/installed/legacy-$legacy_environment.env
            cmp -s "$legacy_file" "$FIXTURE_WORK/private/saved-$legacy_environment.env" || exit 1
            [[ $(stat -c '%u:%g:%a' "$legacy_file") == 1234:1234:600 ]] || exit 1
        done
        state=/home/verdaxis-prod/verdaxis/staging/be/.runtime-deploy/staging.state
        grep -qx 'DEPLOYMENT_STATE=restart-authorized' "$state" || exit 1
        for lock in staging.lock staging.restart.lock; do
            if flock -n "${state%/*}/$lock" true; then exit 1; fi
        done
        : > "$FIXTURE_WORK/active-$2"
        if [[ $2 == verdaxis-rust-staging.service ]]; then
            if [[ -f $FIXTURE_WORK/old.pid ]]; then kill "$(cat "$FIXTURE_WORK/old.pid")" 2>/dev/null || true; fi
            "$FIXTURE_WORK/installed/binary" </dev/null >"$FIXTURE_WORK/old-output" 2>&1 &
            printf '%s\n' "$!" > "$FIXTURE_WORK/old.pid"
        fi ;;
esac
if [[ ${KILL_SERVICE_BOUNDARY:-} == "after:$boundary" ]]; then kill -KILL "$(cat "$FIXTURE_WORK/transaction.pid")"; exit 1; fi
SYSTEMCTL
cat > "$work/bin/caddy" <<'CADDY'
#!/usr/bin/env bash
printf 'caddy %s\n' "$*" >> "$FIXTURE_WORK/calls"
if [[ ${KILL_SERVICE_BOUNDARY:-} == "before:caddy:$1" ]]; then kill -KILL "$(cat "$FIXTURE_WORK/transaction.pid")"; exit 1; fi
if [[ $1 == reload ]]; then
    [[ -f $FIXTURE_WORK/active-verdaxis-backend-staging.service && -f $FIXTURE_WORK/active-verdaxis-rust-staging.service ]] || {
        printf 'FAIL saved Caddy reload precedes both restored services\n' >&2; exit 1;
    }
    [[ -f $FIXTURE_WORK/enabled-verdaxis-backend-staging.service ]] || {
        printf 'FAIL saved Caddy reload precedes retained enable-state restore\n' >&2; exit 1;
    }
    [[ -f $FIXTURE_WORK/ready-python && -f $FIXTURE_WORK/ready-rust ]] || exit 1
    : > "$FIXTURE_WORK/caddy-restored"
fi
if [[ ${KILL_SERVICE_BOUNDARY:-} == "after:caddy:$1" ]]; then kill -KILL "$(cat "$FIXTURE_WORK/transaction.pid")"; exit 1; fi
CADDY
cat > "$work/bin/runuser" <<'RUNUSER'
#!/bin/bash
[[ $1 == -u && $2 == caddy && $3 == -- ]] || exit 1
shift 3
exec "$@"
RUNUSER
cat > "$work/bin/curl" <<'CURL'
#!/usr/bin/env bash
set -euo pipefail
url=${!#}
printf 'curl %s\n' "$url" >> "$FIXTURE_WORK/calls"
if [[ ${KILL_SERVICE_BOUNDARY:-} == "before:curl:$url" ]]; then kill -KILL "$(cat "$FIXTURE_WORK/transaction.pid")"; exit 1; fi
case $url in
    http://127.0.0.1:8001/health/ready)
        [[ -f $FIXTURE_WORK/active-verdaxis-backend-staging.service ]] || exit 1
        : > "$FIXTURE_WORK/ready-python" ;;
    http://127.0.0.1:8011/health/ready)
        [[ -f $FIXTURE_WORK/active-verdaxis-rust-staging.service ]] || exit 1
        : > "$FIXTURE_WORK/ready-rust" ;;
    https://api-staging.verdaxis.exchange/health/ready)
        [[ -f $FIXTURE_WORK/caddy-restored ]] || exit 1 ;;
    https://api-staging.verdaxis.exchange/openapi.json)
        [[ -f $FIXTURE_WORK/caddy-restored && -f $FIXTURE_WORK/active-verdaxis-backend-staging.service ]] || exit 1 ;;
    *) exit 1 ;;
esac
if [[ ${BAD_READY_URL:-} == "$url" ]]; then printf '%s' "${BAD_READY_BODY:-}";
elif [[ $url == */openapi.json ]]; then
    printf '{"openapi":"3.1.0","info":{"title":"Verdaxis Intelligence Cockpit"},"paths":{"/api/auth/login":{"post":{}}}}\n'
else printf '{"status":"ok","db":"ok","environment":"staging","release_sha":"%s"}\n' "$FIXTURE_SHA"; fi
if [[ ${KILL_SERVICE_BOUNDARY:-} == "after:curl:$url" ]]; then kill -KILL "$(cat "$FIXTURE_WORK/transaction.pid")"; exit 1; fi
CURL
cat > "$work/bin/mv" <<'MOVE'
#!/usr/bin/env bash
set -euo pipefail
destination=${!#}
printf '%s\n' "$destination" >> "$FIXTURE_WORK/publications"
count=$(wc -l < "$FIXTURE_WORK/publications")
if [[ ${KILL_POINT:-} == "before:$count" ]]; then kill -KILL "$PPID"; exit 1; fi
/bin/mv "$@"
if [[ ${KILL_POINT:-} == "after:$count" ]]; then kill -KILL "$PPID"; exit 1; fi
MOVE
chmod 0700 "$work/bin/"* "$work/trusted/scripts/verify-artifact.sh"
# Skip physical disk flush only in disposable tests; publication order and
# rename interruption remain real. Production always calls trusted sync.
printf '#!/bin/bash\nexit 0\n' > "$work/bin/sync"
chmod 0700 "$work/bin/sync"
export FIXTURE_WORK=$work FIXTURE_SHA=$sha PATH=$work/bin:$PATH
printf '[pre_m_reader]\nhost=127.0.0.1\n' > "$work/private/reader"
chmod 0600 "$work/private/reader"
: > "$work/files"
for kind in binary artifact provenance signature provenance_key runtime release revision budget policy unit dropin guard monitor_binary monitor_identity caddy credential migrator_credential runtime_package; do
    path=$work/installed/$kind
    case $kind in
        binary) path=$checkout/.runtime-rust/verdaxis-read; cp "$work/installed/binary" "$path" ;;
        runtime) path=$checkout/.runtime-rust/runtime.env ;;
        release) path=$checkout/.runtime-release.env ;;
        revision) path=$checkout/.runtime-rust/revision.json ;;
        budget) path=$checkout/.runtime-rust/budget.json ;;
        policy) path=$checkout/.runtime-rust/staging-policy.json ;;
        monitor_identity) path=$checkout/.runtime-monitor.env ;;
        runtime_package) path=$work/package/runtime-library ;;
    esac
    if [[ $kind == artifact ]]; then cp "$work/installed/binary" "$path";
    elif [[ $kind != binary ]]; then printf '%s private bytes\n' "$kind" > "$path"; fi
    chmod 0600 "$path"
    [[ $kind != binary ]] || chmod 0700 "$path"
    jq -n --arg kind "$kind" --arg path "$path" '{kind:$kind,path:$path}' >> "$work/files"
done
printf 'ENVIRONMENT=staging\nRELEASE_SHA=%s\nMIGRATION_REVISION=final_head\n' "$sha" > "$checkout/.runtime-release.env"
printf 'ENVIRONMENT=staging\nRELEASE_SHA=%s\n' "$sha" > "$checkout/.runtime-monitor.env"
printf '{"revision":"final_head"}\n' > "$checkout/.runtime-rust/revision.json"
cp "$checkout/deploy/rust/staging-policy.json" "$checkout/.runtime-rust/staging-policy.json"
printf 'DATABASE_URL="postgresql://verdaxis_app_staging:fixture@127.0.0.1:5432/verdaxis_staging"\nMIGRATOR_DATABASE_URL="postgresql://verdaxis_migrator_staging:private-fixture@127.0.0.1:5432/verdaxis_staging"\n' > "$checkout/.runtime-rust/runtime.env"
# Both legacy environments need their complete private Settings inputs back.
for legacy_environment in staging production; do
    database=verdaxis_prod; role_suffix=''
    if [[ $legacy_environment == staging ]]; then database=verdaxis_staging; role_suffix=_staging; fi
    legacy_file=$work/installed/legacy-$legacy_environment.env
    printf 'ENVIRONMENT=%s\nRELEASE_SHA=%s\nDATABASE_NAME=%s\nDATABASE_USER=verdaxis_app%s\nJWT_SECRET=fixture-private-jwt-key-at-least-32-characters\nJWT_ISSUER=fixture-issuer\nJWT_AUDIENCE=fixture-audience\nDATABASE_URL=postgresql://verdaxis_app%s:private-app-fixture@127.0.0.1:5432/%s\nMIGRATOR_DATABASE_URL=postgresql://verdaxis_migrator%s:private-migration-fixture@127.0.0.1:5432/%s\n' \
        "$legacy_environment" "$sha" "$database" "$role_suffix" "$role_suffix" "$database" "$role_suffix" "$database" > "$legacy_file"
    chmod 0600 "$legacy_file"
    chown 1234:1234 "$legacy_file"
    jq -n --arg path "$legacy_file" '{kind:"migrator_credential",path:$path}' >> "$work/files"
done
# Validate the actual previous release, not a rewritten Settings substitute.
git -C "$ROOT" show 0920be378:app/config.py > "$work/previous-config.py"
cat > "$work/validate-settings.py" <<'SETTINGS'
import os
import sys
from pathlib import Path

os.environ.clear()
namespace = {"__name__": "previous_release_settings"}
source = Path(sys.argv[1]).read_text().rsplit("settings = Settings()", 1)[0]
exec(compile(source, sys.argv[1], "exec"), namespace)
for path in sys.argv[2:]:
    try:
        settings = namespace["Settings"](_env_file=path)
        assert settings.ENVIRONMENT in {"staging", "production"}
        assert settings.MIGRATOR_DATABASE_URL
    except Exception as error:
        if hasattr(error, "errors"):
            for item in error.errors():
                print("Settings field:", item["loc"], "type:", item["type"], file=sys.stderr)
        # Settings errors can contain input values. Never print them.
        raise SystemExit("FAIL restored previous-release Settings did not validate") from None
SETTINGS
jq -n --slurpfile files "$work/files" --arg sha "$sha" --arg checkout "$checkout" \
    --arg key_sha "$key_sha" --arg consumer_sha "$consumer_sha" --arg package "$work/package" \
    --arg closure_sha "$closure_sha" --arg native_dep "$native_dep" --arg native_sha "$native_sha" \
    '{schema:1,environment:"staging",release_sha:$sha,deploy_uid:0,checkout:$checkout,
      backend_unit:"verdaxis-rust-staging.service",ready_url:"https://api-staging.verdaxis.exchange/health/ready",
      final_alembic:"final_head",acceptance_key_sha256:$key_sha,workflow_ref:"refs/heads/staging",checkout_branch:"staging",database_name:"verdaxis_staging",
      reader_role:"verdaxis_bundle_reader_staging",runtime_role:"verdaxis_app_staging",runtime_package_roots:[$package],
      dependency_closure_sha256:$closure_sha,native_dependency_files:[{path:$native_dep,sha256:$native_sha}],
      consumer_units:(["verdaxis-rust-staging.service","verdaxis-backend-staging.service","verdaxis-health.service","verdaxis-backup-verify.service","verdaxis-news-refresh-staging.service","verdaxis-auth-maintenance-staging.service","verdaxis-product-analytics-prune-staging.service"] | map({name:.,properties_sha256:$consumer_sha})),timers:["verdaxis-news-refresh-staging.timer"],jobs:["verdaxis-news-refresh-staging.service"],files:$files}' > "$work/private/inventory"
jq -cS '{consumer_units,native_dependency_files,runtime_package_roots,files}' "$work/private/inventory" > "$work/closure-inputs"
closure_sha=$(sha256sum "$work/closure-inputs" | cut -d ' ' -f 1)
jq --arg digest "$closure_sha" '.dependency_closure_sha256=$digest' "$work/private/inventory" > "$work/private/inventory-new"
mv "$work/private/inventory-new" "$work/private/inventory"
jq --arg digest "$closure_sha" '.dependency_closure_sha256=$digest' "$work/private/closure" > "$work/private/closure-new"
mv "$work/private/closure-new" "$work/private/closure"
openssl pkeyutl -sign -rawin -inkey "$work/private/acceptance-key" -in "$work/private/closure" -out "$work/private/closure-sig"
chmod 0600 "$work/private/inventory"
bundle=$work/private/pre-M-staging-$sha
run() {
    bash "$script" "$@" --environment staging --release-sha "$sha" --bundle "$bundle" \
        --reader-service-file "$work/private/reader" \
        --acceptance "${acceptance_file:-$work/private/closure}" --acceptance-signature "${acceptance_sig:-$work/private/closure-sig}" \
        --acceptance-public-key "$work/private/acceptance-pub" > "$work/out" 2> "$work/err"
}
inventory_approval=$(sha256sum "$work/private/inventory" | cut -d ' ' -f 1)
# Root recovery must reject executable Git configuration before any status call.
printf '#!/bin/sh\ntouch "%s/fsmonitor-executed"\n' "$work" > "$work/fsmonitor"
chmod 0755 "$work/fsmonitor"
git -C "$checkout" config core.fsmonitor "$work/fsmonitor"
if run capture --inventory "$work/private/inventory" --manifest-sha256 "$inventory_approval"; then exit 1; fi
grep -q 'inert recovery allowlist' "$work/err"
[[ ! -e $work/fsmonitor-executed ]]
git -C "$checkout" config --unset core.fsmonitor
mv "$checkout/.git" "$work/redirect.git"
printf 'gitdir: %s\n' "$work/redirect.git" > "$checkout/.git"
if run capture --inventory "$work/private/inventory" --manifest-sha256 "$inventory_approval"; then exit 1; fi
grep -q 'canonical Git directory must be a real directory' "$work/err"
rm "$checkout/.git"
mv "$work/redirect.git" "$checkout/.git"
printf 'ok privileged recovery refuses executable config and redirected metadata\n'
run capture --inventory "$work/private/inventory" --manifest-sha256 "$inventory_approval"
approval=$(sha256sum "$bundle/manifest.json" | cut -d ' ' -f 1)
run verify --manifest-sha256 "$approval"
[[ $(stat -c %a "$bundle") == 700 && $(stat -c %a "$bundle/payload/000001") == 600 ]]
if grep -q 'private bytes' "$work/out"; then exit 1; fi
printf 'ok private complete capture and independent verify\n'
printf changed >> "$bundle/payload/000001"
if run verify --manifest-sha256 "$approval"; then exit 1; fi
grep -q 'digest differs' "$work/err"
printf 'ok tampered protected copy refuses\n'
cp "$work/installed/binary" "$bundle/payload/000001"
for legacy_environment in staging production; do
    cp "$work/installed/legacy-$legacy_environment.env" "$work/private/saved-$legacy_environment.env"
    rm "$work/installed/legacy-$legacy_environment.env"
done
rm "$work/installed/migrator_credential"
run restore --manifest-sha256 "$approval"
python3 "$work/validate-settings.py" "$work/previous-config.py" \
    "$work/installed/legacy-staging.env" "$work/installed/legacy-production.env"
[[ -f $work/active-verdaxis-backend-staging.service && -f $work/active-verdaxis-rust-staging.service ]]
grep -q 'curl https://api-staging.verdaxis.exchange/openapi.json' "$work/calls"
[[ -f $work/installed/migrator_credential ]]
grep -q '^DEPLOYMENT_STATE=restart-authorized$' "$checkout/.runtime-deploy/staging.state"
grep -q 'old source' "$checkout/source.txt"
printf 'ok full restore preserves private credentials and validates both previous-release environments\n'
if run accept --manifest-sha256 "$approval"; then exit 1; fi
grep -q NO_GO "$work/err"
grep -q '^DEPLOYMENT_STATE=blocked$' "$checkout/.runtime-deploy/staging.state"
printf 'ok absent signed acceptance leaves blocked state\n'
# Observe every publication in a full replay, then kill before and after each.
: > "$work/publications"
run restore --manifest-sha256 "$approval"
count=$(wc -l < "$work/publications")
for when in before after; do
    for (( point=1; point<=count; point++ )); do
        : > "$work/publications"
        if KILL_POINT=$when:$point run restore --manifest-sha256 "$approval"; then
            printf 'FAIL missed kill %s:%s\n' "$when" "$point" >&2; exit 1
        fi
        # Stable lock inodes survive, no lock remains held, and canonical replay
        # renews authorization after every interrupted manifest publication.
        flock -n "$checkout/.runtime-deploy/staging.lock" true
        flock -n "$checkout/.runtime-deploy/staging.restart.lock" true
        : > "$work/publications"
        run restore --manifest-sha256 "$approval"
        grep -q '^DEPLOYMENT_STATE=restart-authorized$' "$checkout/.runtime-deploy/staging.state"
    done
done
printf 'ok before/after every restore publication (%s boundaries)\n' "$count"
# Each externally visible transition may be interrupted on either side.
for boundary in 'enable:verdaxis-backend-staging.service' 'restart:verdaxis-backend-staging.service' \
    'restart:verdaxis-rust-staging.service' \
    'curl:http://127.0.0.1:8001/health/ready' 'curl:http://127.0.0.1:8011/health/ready' \
    'caddy:reload' 'curl:https://api-staging.verdaxis.exchange/health/ready' \
    'curl:https://api-staging.verdaxis.exchange/openapi.json'; do
    for when in before after; do
        rm -f "$work/ready-python" "$work/ready-rust" "$work/caddy-restored" "$work/enabled-verdaxis-backend-staging.service"
        if KILL_SERVICE_BOUNDARY=$when:$boundary run restore --manifest-sha256 "$approval"; then
            printf 'FAIL missed service boundary %s:%s\n' "$when" "$boundary" >&2; exit 1
        fi
        [[ -e $checkout/.runtime-deploy/staging.state ]]
        [[ ! -e $checkout/.runtime-deploy/staging.rollback-accepted ]]
        flock -n "$checkout/.runtime-deploy/staging.lock" true
        flock -n "$checkout/.runtime-deploy/staging.restart.lock" true
        run restore --manifest-sha256 "$approval"
    done
done
printf 'ok before/after restart readiness reload and Python route boundaries\n'
# A successful HTTP exchange must still carry exactly one complete readiness.
valid_ready=$(printf '{"status":"ok","db":"ok","environment":"staging","release_sha":"%s"}' "$sha")
for url in http://127.0.0.1:8001/health/ready http://127.0.0.1:8011/health/ready https://api-staging.verdaxis.exchange/health/ready; do
    for body in '' "$valid_ready$valid_ready" "${valid_ready/staging/production}" "${valid_ready/\"db\":\"ok\"/\"db\":\"bad\"}"; do
        : > "$work/calls"
        if BAD_READY_URL=$url BAD_READY_BODY=$body run restore --manifest-sha256 "$approval"; then exit 1; fi
        grep -q 'exact readiness differs' "$work/err"
        [[ -e $checkout/.runtime-deploy/staging.state ]]
        if [[ $url == http:* ]] && grep -q 'caddy reload' "$work/calls"; then exit 1; fi
    done
done
run restore --manifest-sha256 "$approval"
printf 'ok every readiness surface refuses empty multiple and mismatched bodies\n'
nonce=$(< "$checkout/.runtime-deploy/staging.rollback-nonce")
jq -n --arg sha "$sha" --arg digest "$approval" --arg nonce "$nonce" --arg closure "$closure_sha" \
    '{dependency_closure_sha256:$closure,runtime_dependencies:"PASS",credential_sources:"PASS",unit_inputs:"PASS",environment:"staging",release_sha:$sha,manifest_sha256:$digest,nonce:$nonce,
      invocation_id:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",auth:"PASS",session:"PASS",monitor:"PASS",passive:"PASS",
      old_compiled_pin:"PASS",migration_freeze:"PASS",monitor_invocations:[
        {unit:"verdaxis-health.service",invocation_id:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
        {unit:"verdaxis-backup-verify.service",invocation_id:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}' > "$work/private/acceptance"
chmod 0600 "$work/private/acceptance"
openssl pkeyutl -sign -rawin -inkey "$work/private/acceptance-key" -in "$work/private/acceptance" -out "$work/private/acceptance-sig"
# Signed claims cannot override exact checked-out bytes or metadata.
printf 'changed helper\n' > "$checkout/scripts/deploy.sh"
if acceptance_file=$work/private/acceptance acceptance_sig=$work/private/acceptance-sig run accept --manifest-sha256 "$approval"; then exit 1; fi
grep -q 'checkout identity or bytes differ' "$work/err"
printf 'old helper\n' > "$checkout/scripts/deploy.sh"
# Signed acceptance cannot waive captured migration input bytes or metadata.
legacy_file=$work/installed/legacy-staging.env
printf '\n' >> "$legacy_file"
if acceptance_file=$work/private/acceptance acceptance_sig=$work/private/acceptance-sig run accept --manifest-sha256 "$approval"; then exit 1; fi
grep -q 'bytes differ' "$work/err"
cp "$work/private/saved-staging.env" "$legacy_file"
chown 0:0 "$legacy_file"
if acceptance_file=$work/private/acceptance acceptance_sig=$work/private/acceptance-sig run accept --manifest-sha256 "$approval"; then exit 1; fi
grep -q 'metadata differs' "$work/err"
chown 1234:1234 "$legacy_file"
chmod 0644 "$checkout/.runtime-rust/runtime.env"
if acceptance_file=$work/private/acceptance acceptance_sig=$work/private/acceptance-sig run accept --manifest-sha256 "$approval"; then exit 1; fi
grep -q 'metadata differs' "$work/err"
chmod 0600 "$checkout/.runtime-rust/runtime.env"
printf 'ok signed acceptance cannot override changed source or modes\n'
# The final accepted-journal publication is interrupted before and after rename.
for when in before after; do
    : > "$work/publications"
    if KILL_POINT=$when:3 acceptance_file=$work/private/acceptance acceptance_sig=$work/private/acceptance-sig run accept --manifest-sha256 "$approval"; then exit 1; fi
    acceptance_file=$work/private/acceptance acceptance_sig=$work/private/acceptance-sig run accept --manifest-sha256 "$approval"
    [[ ! -e $checkout/.runtime-deploy/staging.state ]]
    # Prepare a fresh complete old restore and sign its new attempt nonce.
    run restore --manifest-sha256 "$approval"
    nonce=$(< "$checkout/.runtime-deploy/staging.rollback-nonce")
    jq --arg nonce "$nonce" '.nonce=$nonce' "$work/private/acceptance" > "$work/private/acceptance-new"
    mv "$work/private/acceptance-new" "$work/private/acceptance"
    openssl pkeyutl -sign -rawin -inkey "$work/private/acceptance-key" -in "$work/private/acceptance" -out "$work/private/acceptance-sig"
done
printf 'ok before/after accepted journal publication resumes\n'
if KILL_TIMER_START=1 acceptance_file=$work/private/acceptance acceptance_sig=$work/private/acceptance-sig run accept --manifest-sha256 "$approval"; then exit 1; fi
[[ ! -e $checkout/.runtime-deploy/staging.state && -f $checkout/.runtime-deploy/staging.rollback-accepted ]]
# Simulate interruption after each auxiliary unlink; accepted journal is complete.
rm "$checkout/.runtime-deploy/staging.rollback-timers"
rm "$checkout/.runtime-deploy/staging.rollback-nonce"
acceptance_file=$work/private/acceptance acceptance_sig=$work/private/acceptance-sig run accept --manifest-sha256 "$approval"
[[ ! -e $checkout/.runtime-deploy/staging.state ]]
grep -q ACCEPTED "$work/out"
printf 'ok genuine Ed25519 bound acceptance checks actual old ELF process\n'
# A new restore is a new authorization attempt and uses a new nonce.
run restore --manifest-sha256 "$approval"
printf '\n' >> "$work/private/acceptance"
if acceptance_file=$work/private/acceptance acceptance_sig=$work/private/acceptance-sig run accept --manifest-sha256 "$approval"; then exit 1; fi
grep -q 'signature differs' "$work/err"
grep -q '^DEPLOYMENT_STATE=blocked$' "$checkout/.runtime-deploy/staging.state"
printf 'ok changed signed acceptance remains blocked\n'
