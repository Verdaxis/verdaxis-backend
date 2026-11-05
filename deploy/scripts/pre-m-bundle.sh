#!/usr/bin/env bash
# Run a trusted installed copy outside the checkout. No candidate helper is run.
# Linux host tools only. Inventory and acceptance records are data, not commands.
set -euo pipefail
export LC_ALL=C
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
umask 077
unset BASH_ENV ENV CDPATH GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY
for git_variable in "${!GIT_@}"; do unset "$git_variable"; done
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_NO_REPLACE_OBJECTS=1
fail() { printf 'NO_GO pre-M bundle: %s\n' "$1" >&2; exit 1; }
mode=${1:-}; (( $# == 0 )) || shift
bundle='' inventory='' environment='' release='' approval='' reader='' evidence='' signature='' key=''
while (( $# )); do
    (( $# >= 2 )) || fail 'option requires a value'
    case "$1" in
        --bundle) bundle=$2 ;; --inventory) inventory=$2 ;;
        --environment) environment=$2 ;; --release-sha) release=$2 ;;
        --manifest-sha256) approval=$2 ;; --reader-service-file) reader=$2 ;;
        --acceptance) evidence=$2 ;; --acceptance-signature) signature=$2 ;;
        --acceptance-public-key) key=$2 ;;
        *) fail 'unknown option' ;;
    esac
    shift 2
done
[[ $mode == capture || $mode == verify || $mode == restore || $mode == accept ]] || fail 'use capture, verify, restore or accept'
[[ $environment == production || $environment == staging ]] || fail 'explicit deployed environment required'
[[ $release =~ ^[0-9a-f]{40}$ && $bundle == /* ]] || fail 'full SHA and absolute private bundle required'
[[ $(uname -s) == Linux && $EUID == 0 ]] || fail 'root on Linux is required'
for tool in jq git flock stat sha256sum install openssl psql systemctl timeout curl sync cmp runuser; do
    command -v "$tool" >/dev/null || fail 'trusted host tooling missing'
done
work=$(mktemp -d /tmp/verdaxis-pre-m.XXXXXXXX)
trap 'rm -rf -- "$work"' EXIT
trap 'exit 1' HUP INT TERM
hash() { sha256sum -- "$1" | cut -d ' ' -f 1; }
# All ancestors must be real directories owned by root or the deploy account,
# never writable by other users. Every leaf read is regular and single-linked.
safe_path() {
    local path=$1 parent
    [[ $path == /* && $path != *$'\n'* && $path != *$'\t'* && $path != *'//'* && $path != */../* && $path != */./* ]] || fail 'unsafe absolute path'
    parent=$(dirname -- "$path")
    while [[ $parent != / ]]; do
        [[ ! -L $parent && -d $parent ]] || fail 'unsafe directory ancestor'
        local owner permissions
        owner=$(stat -c %u -- "$parent"); permissions=$(stat -c %a -- "$parent")
        [[ $owner == 0 || $owner == "$deploy_uid" ]] || fail 'untrusted directory owner'
        (( (8#$permissions & 0022) == 0 )) || fail 'directory is writable by another user'
        parent=$(dirname -- "$parent")
    done
    [[ ! -L $path ]] || fail 'symlink refused'
}
regular() {
    safe_path "$1"
    [[ -f $1 && $(stat -c %h -- "$1") == 1 ]] || fail 'regular single-link file required'
}
private_file() {
    regular "$1"
    [[ $(stat -c %u -- "$1") == 0 ]] || fail 'private record must be root owned'
    local permissions; permissions=$(stat -c %a -- "$1")
    (( (8#$permissions & 0077) == 0 )) || fail 'private file permissions required'
}
# Inventory root paths are operator-reviewed. They cannot name command programs.
# Command programs always resolve from the trusted host PATH, never inventory.
if [[ $mode == capture ]]; then
    [[ -n $inventory ]] || fail 'inventory required'
    deploy_uid=0
    private_file "$inventory"
    [[ $approval =~ ^[0-9a-f]{64}$ && $(hash "$inventory") == "$approval" ]] || fail 'independently approved inventory digest required'
    cp -- "$inventory" "$work/inventory.json"
else
    [[ $approval =~ ^[0-9a-f]{64}$ ]] || fail 'independent manifest SHA-256 approval required'
    deploy_uid=0
    private_file "$bundle/manifest.json"
    [[ $(stat -c '%u:%a' "$bundle") == 0:700 && $(stat -c '%u:%a' "$bundle/payload") == 0:700 ]] || fail 'bundle directories must be private root owned'
    [[ $(hash "$bundle/manifest.json") == "$approval" ]] || fail 'manifest approval differs'
    cp -- "$bundle/manifest.json" "$work/manifest.json"
    jq '.inventory' "$work/manifest.json" > "$work/inventory.json"
fi
jq -e --arg env "$environment" --arg sha "$release" '
    . as $inventory | .schema == 1 and .environment == $env and .release_sha == $sha and
    (.checkout | type == "string") and (.deploy_uid | type == "number") and
    (.files | type == "array" and length > 0 and length < 4096) and
    (.timers | type == "array") and (.jobs | type == "array") and
    (.backend_unit | test("^verdaxis-[a-z0-9-]+\\.service$")) and
    (.ready_url | test("^https://api(-staging)?\\.verdaxis\\.exchange/health/ready$")) and
    (.final_alembic | test("^[a-z0-9_]+$")) and
    (.acceptance_key_sha256 | test("^[0-9a-f]{64}$")) and
    (.workflow_ref == "refs/heads/staging" or .workflow_ref == "refs/heads/prod") and
    (.checkout_branch == "staging" or .checkout_branch == "prod") and
    (.database_name | test("^verdaxis_(prod|staging)$")) and
    (.reader_role | test("^verdaxis_bundle_reader(_staging)?$")) and
    (.runtime_role | test("^verdaxis_app(_staging)?$")) and
    (.consumer_units | type == "array" and length > 0) and
    (.runtime_package_roots | type == "array" and length > 0) and
    (.dependency_closure_sha256 | test("^[0-9a-f]{64}$")) and
    (.native_dependency_files | type == "array" and length > 0) and
    all(.native_dependency_files[]; (.path | test("^/(lib|lib64|usr/lib|usr/lib64|bin|usr/bin)/[A-Za-z0-9_./+-]+$")) and (.sha256 | test("^[0-9a-f]{64}$"))) and
    all(.consumer_units[]; (.name | test("^verdaxis-[a-z0-9-]+\\.service$")) and
        (.properties_sha256 | test("^[0-9a-f]{64}$"))) and
    ([.files[].path] | length == (unique | length)) and
    all(.files[]; (.kind | IN("binary","artifact","provenance","signature","provenance_key",
        "runtime","release","revision","budget","policy","unit","dropin","guard",
        "monitor_binary","monitor_identity","caddy","credential","migrator_credential","runtime_package"))
        and (.path | test("^/[A-Za-z0-9_.@/+:-]+$"))) and
    all(["binary","artifact","provenance","signature","provenance_key","runtime","release",
        "revision","budget","policy","unit","guard","monitor_binary","monitor_identity","caddy",
        "credential","migrator_credential","runtime_package"][]; . as $kind |
        any($inventory.files[]; .kind == $kind))
' "$work/inventory.json" >/dev/null || fail 'incomplete inventory'
checkout=$(jq -r .checkout "$work/inventory.json")
deploy_uid=$(jq -r .deploy_uid "$work/inventory.json")
[[ $deploy_uid =~ ^[0-9]+$ ]] || fail 'invalid deploy owner'
if [[ $environment == production ]]; then
    [[ $(jq -r .database_name "$work/inventory.json") == verdaxis_prod && $(jq -r .runtime_role "$work/inventory.json") == verdaxis_app && $(jq -r .reader_role "$work/inventory.json") == verdaxis_bundle_reader ]] || fail 'production database identity differs'
else
    [[ $(jq -r .database_name "$work/inventory.json") == verdaxis_staging && $(jq -r .runtime_role "$work/inventory.json") == verdaxis_app_staging && $(jq -r .reader_role "$work/inventory.json") == verdaxis_bundle_reader_staging ]] || fail 'staging database identity differs'
fi
[[ $checkout == /home/verdaxis-prod/verdaxis/prod/be && $environment == production ||
   $checkout == /home/verdaxis-prod/verdaxis/staging/be && $environment == staging ]] || fail 'noncanonical checkout'
state_dir=$checkout/.runtime-deploy
state=$state_dir/$environment.state
backend=$(jq -r .backend_unit "$work/inventory.json")
[[ $backend == verdaxis-rust.service && $environment == production || $backend == verdaxis-rust-staging.service && $environment == staging ]] || fail 'actual old Rust service required'
ready=$(jq -r .ready_url "$work/inventory.json")
final_alembic=$(jq -r .final_alembic "$work/inventory.json")
file_of_kind() {
    jq -er --arg kind "$1" '[.files[] | select(.kind == $kind) | .path] | if length == 1 then .[0] else error("one file required") end' "$work/inventory.json"
}
binary=$(file_of_kind binary); caddy=$(file_of_kind caddy)
[[ $binary == "$checkout/.runtime-rust/verdaxis-read" && $(file_of_kind runtime) == "$checkout/.runtime-rust/runtime.env" &&
   $(file_of_kind revision) == "$checkout/.runtime-rust/revision.json" && $(file_of_kind budget) == "$checkout/.runtime-rust/budget.json" &&
   $(file_of_kind release) == "$checkout/.runtime-release.env" && $(file_of_kind monitor_identity) == "$checkout/.runtime-monitor.env" &&
   $(file_of_kind policy) == "$checkout/.runtime-rust/$environment-policy.json" ]] || fail 'canonical release input path differs'
# Freeze means old code has no migrator credential route. Migrator credential
# files are escrowed but never restored. Runtime envs with such keys refuse.
check_consumers() {
    local row unit digest
    # Bind the actual Rust-serving unit, not merely the legacy backend unit.
    expected_rust=verdaxis-rust.service
    [[ $environment != staging ]] || expected_rust=verdaxis-rust-staging.service
    jq -e --arg name "$expected_rust" 'any(.consumer_units[]; .name == $name)' "$work/inventory.json" >/dev/null || fail 'Rust service consumer missing'
    jq -e --arg name "$backend" 'any(.consumer_units[]; .name == $name)' "$work/inventory.json" >/dev/null || fail 'selected service consumer missing'
    local suffix=''
    [[ $environment != staging ]] || suffix=-staging
    for unit in "verdaxis-backend$suffix.service" verdaxis-health.service verdaxis-backup-verify.service \
        "verdaxis-news-refresh$suffix.service" "verdaxis-auth-maintenance$suffix.service" "verdaxis-product-analytics-prune$suffix.service"; do
        jq -e --arg name "$unit" 'any(.consumer_units[]; .name == $name)' "$work/inventory.json" >/dev/null || fail 'required old consumer missing'
    done
    while IFS= read -r row; do
        unit=$(jq -r .name <<< "$row"); digest=$(jq -r .properties_sha256 <<< "$row")
        systemctl show "$unit" -p FragmentPath -p DropInPaths -p ExecStart -p ExecStartPre \
            -p EnvironmentFiles -p Environment -p PassEnvironment -p SetCredential -p SetCredentialEncrypted \
            -p LoadCredential -p LoadCredentialEncrypted -p User -p Group -p WorkingDirectory > "$work/consumer"
        [[ $(hash "$work/consumer") == "$digest" ]] || fail 'installed consumer property binding differs'
        grep -q '^FragmentPath=/' "$work/consumer" || fail 'installed unit fragment absent'
        if grep -qE '^(PassEnvironment|SetCredential|SetCredentialEncrypted|LoadCredentialEncrypted)=.+' "$work/consumer" ||
            grep -qE '^(EnvironmentFiles|LoadCredential)=.*(\\$|%)|^Environment=.*(migrator|PGSERVICE|PGPASSFILE|DATABASE_URL)' "$work/consumer"; then fail 'dynamic or embedded credential consumer unsupported'; fi
        # Every absolute file input named by the installed consumer must be
        # present in the approved inventory (source files are captured as Git).
        while IFS= read -r input; do
            if [[ $input == "$checkout" ]]; then continue; fi
            if [[ $input == "$checkout/"* ]] && git -c "safe.directory=$checkout" -c core.hooksPath=/dev/null -C "$checkout" ls-files --error-unmatch "${input#"$checkout/"}" >/dev/null 2>&1; then continue; fi
            case $input in
                /usr/bin/*|/bin/*|/usr/lib/*)
                    jq -e --arg path "$input" 'any(.native_dependency_files[]; .path == $path)' "$work/inventory.json" >/dev/null || fail 'native consumer executable not in dependency closure'
                    continue ;;
            esac
            jq -e --arg path "$input" 'any(.files[]; .path == $path)' "$work/inventory.json" >/dev/null || fail 'installed consumer input missing from inventory'
        done < <(grep -oE '/[A-Za-z0-9_./@:+-]+' "$work/consumer" | sort -u)
    done < <(jq -c '.consumer_units[]' "$work/inventory.json")
}
check_package_closure() {
    local root path row
    jq -cS '{consumer_units,native_dependency_files,runtime_package_roots,files}' "$work/inventory.json" > "$work/closure-inputs"
    [[ $(hash "$work/closure-inputs") == "$(jq -r .dependency_closure_sha256 "$work/inventory.json")" ]] || fail 'concrete dependency closure digest differs'
    [[ -n $evidence && -n $signature && -n $key ]] || fail 'trusted signed native dependency closure evidence required'
    private_file "$evidence"; private_file "$signature"; regular "$key"
    [[ $(hash "$key") == "$(jq -r .acceptance_key_sha256 "$work/inventory.json")" ]] || fail 'dependency closure signer differs'
    openssl pkeyutl -verify -pubin -rawin -inkey "$key" -sigfile "$signature" -in "$evidence" >"$work/closure-verify" 2>&1 || fail 'dependency closure signature differs'
    jq -e --arg sha "$release" --arg env "$environment" --arg digest "$(jq -r .dependency_closure_sha256 "$work/inventory.json")" \
        '.environment == $env and .release_sha == $sha and .dependency_closure_sha256 == $digest and .runtime_dependencies == "PASS" and .credential_sources == "PASS" and .unit_inputs == "PASS"' "$evidence" >/dev/null || fail 'trusted dependency closure incomplete'
    while IFS= read -r row; do
        path=$(jq -r .path <<< "$row")
        regular "$path"
        [[ $(hash "$path") == "$(jq -r .sha256 <<< "$row")" ]] || fail 'native loader dependency differs; restore matching runtime package first'
    done < <(jq -c '.native_dependency_files[]' "$work/inventory.json")
    while IFS= read -r root; do
        [[ $root == /* ]] || fail 'runtime package root must be absolute'
        safe_path "$root"
        [[ -d $root && ! -L $root ]] || fail 'runtime package root unavailable'
        [[ -z $(find "$root" -type l -print -quit) ]] || fail 'runtime package symlinks need independent closure support'
        while IFS= read -r path; do
            jq -e --arg path "$path" 'any(.files[]; .path == $path and .kind == "runtime_package")' "$work/inventory.json" >/dev/null || fail 'runtime package file missing from inventory'
        done < <(find "$root" -type f -print | sort)
    done < <(jq -r '.runtime_package_roots[]' "$work/inventory.json")
}
check_runtime_credentials() {
    local row kind path runtime_url
    path=$(file_of_kind runtime)
    runtime_url=$(grep '^DATABASE_URL=' "$path")
    [[ $runtime_url != *$'\n'* ]] || fail 'runtime database URL repeated'
    runtime_url=${runtime_url#DATABASE_URL=}
    case $runtime_url in
        \"*\") runtime_url=${runtime_url#\"}; runtime_url=${runtime_url%\"} ;;
        \'*\') runtime_url=${runtime_url#\'}; runtime_url=${runtime_url%\'} ;;
    esac
    [[ $runtime_url != *\\* && $runtime_url != *\"* && $runtime_url != *\'* ]] || fail 'runtime database URL quoting unsupported'
    [[ $runtime_url =~ ^postgres(ql)?://verdaxis_app(_staging)?:[^@/[:space:]]+@127\.0\.0\.1:5432/verdaxis_(prod|staging)$ ]] || fail 'runtime app database authority is not canonical'
    if [[ $environment == staging ]]; then
        [[ $runtime_url == *'://verdaxis_app_staging:'*'/verdaxis_staging' ]] || fail 'runtime staging database differs'
    else
        [[ $runtime_url == *'://verdaxis_app:'*'/verdaxis_prod' ]] || fail 'runtime production database differs'
    fi
    # A legacy .env can otherwise remain as an untracked source of authority.
    [[ ! -e $checkout/.env ]] || jq -e --arg path "$checkout/.env" 'any(.files[]; .path == $path and (.kind == "credential" or .kind == "migrator_credential"))' "$work/inventory.json" >/dev/null || fail 'legacy env is not fenced or captured'
    while IFS= read -r row; do
        kind=$(jq -r .kind <<< "$row"); path=$(jq -r .path <<< "$row")
        case $kind in
            runtime|release|credential)
                if grep -qiE '(migrator|PGSERVICE|PGPASSFILE|PGPASSWORD|PGUSER)' "$path"; then
                    fail 'runtime still has a migrator credential reference'
                fi ;;
        esac
    done < <(jq -c '.files[]' "$work/inventory.json")
}
# Native catalog projection is shared with sqlx. Credentials remain in an
# independent private libpq service file, not process arguments or evidence.
read_database() {
    [[ -n $reader ]] || fail 'independent reader service file required'
    private_file "$reader"
    local sql=$work/catalog.sql result=$work/catalog
    {
        printf 'BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;\n'
        printf "SELECT 'DATABASE:' || current_database() || ':' || current_user || ':' || inet_server_addr() || ':' || inet_server_port();\n"
        printf "SELECT 'ALEMBIC:' || version_num FROM public.alembic_version ORDER BY version_num;\n"
        cat -- "$trusted_sql"
        printf '\nCOMMIT;\n'
    } > "$sql"
    env -i PATH="$PATH" PGSERVICEFILE="$reader" PGSERVICE=pre_m_reader PGOPTIONS='-c default_transaction_read_only=on -c statement_timeout=10000 -c lock_timeout=1000' \
        timeout 30 psql -X -qAt -v ON_ERROR_STOP=1 -f "$sql" > "$result" 2>"$work/db-error" || fail 'read-only catalog check failed'
    [[ $(grep '^DATABASE:' "$result") == "DATABASE:$(jq -r .database_name "$work/inventory.json"):$(jq -r .reader_role "$work/inventory.json"):127.0.0.1:5432" ]] || fail 'reader database endpoint or role differs'
    [[ $(grep -c '^ALEMBIC:' "$result") == 1 && $(grep '^ALEMBIC:' "$result") == "ALEMBIC:$final_alembic" ]] || fail 'final Alembic identity differs'
    grep -vE '^(ALEMBIC|DATABASE):' "$result" > "$work/catalog-projection"
    catalog_hash=$(hash "$work/catalog-projection")
}
trusted_base=$(cd "$(dirname "$0")" && pwd)
trusted_sql=$(dirname "$trusted_base")/rust/catalog-fingerprint.sql
trusted_verifier=$trusted_base/verify-artifact.sh
regular "$trusted_sql"; regular "$trusted_verifier"
[[ $(stat -c %u "$trusted_sql") == 0 && $(stat -c %u "$trusted_verifier") == 0 ]] || fail 'trusted native helpers must be root owned'
# Both stable lock files keep their inode across capture, restore and accept.
lock_one() {
    local path=$1 fd before after
    safe_path "$path"
    [[ -e $path && -f $path && $(stat -c %h "$path") == 1 ]] || fail 'stable locks must already exist'
    before=''; [[ ! -e $path ]] || before=$(stat -c '%d:%i:%u:%h' "$path")
    exec {fd}>>"$path"
    after=$(stat -Lc '%d:%i:%u:%h' "/proc/self/fd/$fd")
    [[ $after == "$(stat -c '%d:%i:%u:%h' "$path")" && $after == *":$deploy_uid:1" ]] || fail 'stable lock identity differs'
    [[ -z $before || $before == "$after" ]] || fail 'stable lock changed'
    chmod 0600 "/proc/self/fd/$fd"
    flock -n "$fd" || fail 'stable lock is busy'
    lock_fds+=("$fd")
}
locks() {
    safe_path "$state_dir"
    [[ -d $state_dir && $(stat -c %u "$state_dir") == "$deploy_uid" ]] || fail 'deploy state directory owner differs'
    lock_fds=()
    # Files must be created by the deploy owner before this root-only utility.
    lock_one "$state_dir/$environment.lock"
    if [[ $mode == capture ]]; then
        [[ ! -e $state && ! -L $state ]] || fail 'capture requires absent deploy state before restart lock'
    fi
    resume_accepted=false
    if [[ $mode == accept && -f $state_dir/$environment.rollback-accepted && ! -e $state && ! -L $state ]]; then
        regular "$state_dir/$environment.rollback-accepted"
        [[ $(stat -c '%u:%a' "$state_dir/$environment.rollback-accepted") == "$deploy_uid:600" ]] || fail 'accepted resume record metadata differs'
        jq -e --arg digest "$approval" '.manifest_sha256 == $digest' "$state_dir/$environment.rollback-accepted" >/dev/null || fail 'accepted resume names another bundle'
        resume_accepted=true
    fi
    if [[ $mode == restore || $mode == accept && $resume_accepted == false ]]; then
        if [[ $mode == accept ]]; then
            regular "$state_dir/$environment.rollback-nonce"
            nonce=$(< "$state_dir/$environment.rollback-nonce")
            [[ $nonce =~ ^[0-9a-f]{64}$ ]] || fail 'restore attempt nonce missing'
        else
            nonce=$(openssl rand -hex 32)
        fi
        write_state blocked
    fi
    lock_one "$state_dir/$environment.restart.lock"
}
# Test fixtures stop at observed rename operations; there is no runtime callback.
publish() {
    local source=$1 destination=$2 uid=$3 gid=$4 permissions=$5 tmp staging
    safe_path "$destination"
    printf 'PUBLICATION before destination=%s\n' "$destination" >&2
    [[ $(stat -c %d "$state_dir") == "$(stat -c %d "$(dirname "$destination")")" ]] || fail 'publication target is on another filesystem'
    staging=$state_dir/.pre-m-private
    if [[ ! -e $staging ]]; then mkdir -m 0700 -- "$staging"; fi
    safe_path "$staging"
    [[ $(stat -c '%u:%a' "$staging") == 0:700 ]] || fail 'private publication staging metadata differs'
    tmp=$(mktemp "$staging/publish.XXXXXXXX")
    install -o "$uid" -g "$gid" -m "$permissions" -- "$source" "$tmp"
    sync -f "$tmp"
    mv -T -- "$tmp" "$destination"
    sync -f "$(dirname "$destination")"
    printf 'PUBLICATION after destination=%s\n' "$destination" >&2
}
write_state() {
    local phase=$1
    printf 'DEPLOYMENT_STATE=%s\nENVIRONMENT=%s\nRELEASE_SHA=%s\n' "$phase" "$environment" "$release" > "$work/state"
    [[ $phase != blocked ]] || printf 'DEPLOY_NONCE=%s\n' "$nonce" >> "$work/state"
    publish "$work/state" "$state" "$deploy_uid" "$(stat -c %g "$state_dir")" 0644
}
verify_signed_artifact() {
    local artifact provenance sig pub output=$work/verified
    artifact=$(file_of_kind artifact); provenance=$(file_of_kind provenance)
    sig=$(file_of_kind signature); pub=$(file_of_kind provenance_key)
    if [[ $mode != capture ]]; then
        artifact=$(payload_of "$artifact"); provenance=$(payload_of "$provenance")
        sig=$(payload_of "$sig"); pub=$(payload_of "$pub")
    fi
    mkdir -m 0700 "$output"
    bash "$trusted_verifier" --archive "$artifact" --source-sha "$release" --output-dir "$output" \
        --provenance-record "$provenance" --provenance-signature "$sig" --provenance-public-key "$pub" \
        --workflow-ref "$(jq -r .workflow_ref "$work/inventory.json")" \
        >"$work/artifact-check" 2>&1 || fail 'original signed artifact verification failed'
    local actual=$binary
    [[ $mode == capture ]] || actual=$(payload_of "$binary")
    [[ $(hash "$actual") == "$(hash "$output/verdaxis-read")" ]] || fail 'actual old executable differs from signed artifact'
    if [[ $mode == capture ]]; then
        local release_file revision_file monitor_file
        release_file=$(file_of_kind release); revision_file=$(file_of_kind revision); monitor_file=$(file_of_kind monitor_identity)
        if ! grep -qx "ENVIRONMENT=$environment" "$release_file" || ! grep -qx "RELEASE_SHA=$release" "$release_file" ||
            ! grep -qx "MIGRATION_REVISION=$final_alembic" "$release_file"; then fail 'captured release identity differs'; fi
        if ! grep -qx "ENVIRONMENT=$environment" "$monitor_file" || ! grep -qx "RELEASE_SHA=$release" "$monitor_file"; then fail 'captured monitor identity differs'; fi
        jq -e --arg revision "$final_alembic" '.revision == $revision' "$revision_file" >/dev/null || fail 'captured revision record differs'
    fi
    local policy_path policy_actual
    policy_path=$(file_of_kind policy)
    policy_actual=$policy_path
    [[ $mode == capture ]] || policy_actual=$(payload_of "$policy_path")
    git -c "safe.directory=$checkout" -c core.hooksPath=/dev/null -C "$checkout" show "$release:deploy/rust/$environment-policy.json" > "$work/source-policy"
    cmp -s "$policy_actual" "$work/source-policy" || fail 'selected policy differs from exact checkout source'
    timeout 10 "$output/verdaxis-read" --version-json > "$work/version" 2>"$work/version-error" || fail 'old executable version check failed'
    jq -e --arg sha "$release" --arg revision "$final_alembic" '.release_sha == $sha and (.migration_revision_allowlist | index($revision) != null)' "$work/version" >/dev/null || fail 'old executable source identity or compiled pin differs'
}
payload_of() {
    local id
    id=$(jq -er --arg path "$1" '.files[] | select(.path == $path) | .id' "$work/manifest.json")
    printf '%s/payload/%s' "$bundle" "$id"
}
if [[ $mode == capture ]]; then
    safe_path "$bundle"
    [[ $(stat -c '%u:%a' "$(dirname "$bundle")") == 0:700 ]] || fail 'bundle parent must be private root owned'
    [[ ! -e $bundle ]] || fail 'bundle already exists'
    [[ $(basename "$bundle") == "pre-M-$environment-$release" ]] || fail 'bundle name differs'
    branch=$(jq -r .checkout_branch "$work/inventory.json")
    [[ $(git -c "safe.directory=$checkout" -c core.hooksPath=/dev/null -C "$checkout" symbolic-ref --short HEAD) == "$branch" && $(git -c "safe.directory=$checkout" -c core.hooksPath=/dev/null -C "$checkout" rev-parse HEAD) == "$release" && -z $(git -c "safe.directory=$checkout" -c core.hooksPath=/dev/null -C "$checkout" status --porcelain --untracked-files=no) ]] || fail 'checkout branch, identity or tracked bytes differ'
    locks
    [[ ! -e $state && ! -L $state ]] || fail 'capture requires an accepted release'
    check_runtime_credentials
    check_consumers
    check_package_closure
    if grep -qE '^[[:space:]]*import[[:space:]]' "$caddy"; then fail 'Caddy imports require a self-contained saved site'; fi
    [[ -f $checkout/scripts/deploy.sh && -f $checkout/deploy/scripts/verify-deploy-start-guard.sh ]] || fail 'checkout deploy helper or guard missing'
    read_database
    verify_signed_artifact
    stage=$(mktemp -d "$(dirname "$bundle")/.pre-M-capture.XXXXXXXX")
    mkdir -m 0700 "$stage/payload"
    index=0
    : > "$work/files"
    while IFS= read -r row; do
        path=$(jq -r .path <<< "$row"); kind=$(jq -r .kind <<< "$row")
        regular "$path"
        case $kind in
            credential|migrator_credential|runtime|runtime_package)
                permissions=$(stat -c %a "$path")
                (( (8#$permissions & 0077) == 0 )) || fail 'private runtime or credential modes required' ;;
        esac
        index=$((index + 1)); id=$(printf '%06d' "$index")
        cp -- "$path" "$stage/payload/$id"; chmod 0600 "$stage/payload/$id"
        [[ $(hash "$path") == "$(hash "$stage/payload/$id")" ]] || fail 'capture input changed'
        jq -n --arg id "$id" --arg kind "$kind" --arg path "$path" --arg sha "$(hash "$path")" \
            --argjson uid "$(stat -c %u "$path")" --argjson gid "$(stat -c %g "$path")" \
            --arg mode "$(stat -c %a "$path")" --argjson size "$(stat -c %s "$path")" \
            '{id:$id,kind:$kind,path:$path,sha256:$sha,uid:$uid,gid:$gid,mode:$mode,size:$size}' >> "$work/files"
    done < <(jq -c '.files[]' "$work/inventory.json")
    git -c "safe.directory=$checkout" -c core.hooksPath=/dev/null -C "$checkout" bundle create "$stage/checkout.bundle" HEAD >"$work/git-output" 2>&1
    git -c "safe.directory=$checkout" -c core.hooksPath=/dev/null -C "$checkout" ls-tree -r "$release" > "$stage/source-tree"
    ! grep -qvE $'^100(644|755) blob [0-9a-f]{40}\t' "$stage/source-tree" || fail 'nonregular checkout source member'
    : > "$stage/source-metadata"
    while IFS=$'\t' read -r metadata source_path; do
        [[ $source_path =~ ^[A-Za-z0-9_./@+-]+$ && $source_path != /* && $source_path != *../* ]] || fail 'unsafe source path'
        regular "$checkout/$source_path"
        printf '%s\t%s\n' "$(stat -c '%u:%g:%a:%s' "$checkout/$source_path")" "$source_path" >> "$stage/source-metadata"
    done < "$stage/source-tree"
    jq -n --slurpfile inventory "$work/inventory.json" --slurpfile files "$work/files" \
        --arg catalog "$catalog_hash" --arg objects "$(hash "$stage/checkout.bundle")" --arg tree "$(hash "$stage/source-tree")" \
        --arg metadata "$(hash "$stage/source-metadata")" \
        '{schema:1,inventory:$inventory[0],files:$files,catalog_sha256:$catalog,checkout_sha256:$objects,source_tree_sha256:$tree,source_metadata_sha256:$metadata}' > "$stage/manifest.json"
    chmod 0600 "$stage/manifest.json" "$stage/checkout.bundle" "$stage/source-tree" "$stage/source-metadata"
    chmod 0700 "$stage/payload"
    sync -f "$stage"
    mv -T -- "$stage" "$bundle"; sync -f "$(dirname "$bundle")"
    printf 'CAPTURED manifest_sha256=%s\n' "$(hash "$bundle/manifest.json")"
    exit 0
fi
# Validate every protected payload before using any captured byte.
private_file "$bundle/checkout.bundle"; private_file "$bundle/source-tree"; private_file "$bundle/source-metadata"
[[ $(hash "$bundle/checkout.bundle") == "$(jq -r .checkout_sha256 "$work/manifest.json")" &&
   $(hash "$bundle/source-tree") == "$(jq -r .source_tree_sha256 "$work/manifest.json")" &&
   $(hash "$bundle/source-metadata") == "$(jq -r .source_metadata_sha256 "$work/manifest.json")" ]] || fail 'checkout bundle digest differs'
while IFS= read -r row; do
    id=$(jq -r .id <<< "$row"); [[ $id =~ ^[0-9]{6}$ ]] || fail 'invalid payload identifier'
    payload=$bundle/payload/$id; private_file "$payload"
    [[ $(hash "$payload") == "$(jq -r .sha256 <<< "$row")" && $(stat -c %s "$payload") == "$(jq -r .size <<< "$row")" ]] || fail 'protected payload digest differs'
done < <(jq -c '.files[]' "$work/manifest.json")
verify_signed_artifact
if [[ $mode == verify ]]; then
    check_package_closure
    read_database
    [[ $catalog_hash == "$(jq -r .catalog_sha256 "$work/manifest.json")" ]] || fail 'application catalog differs'
    printf 'VERIFIED manifest_sha256=%s\n' "$approval"
    exit 0
fi
# Restoration must not overwrite the trusted script or its native dependencies.
case "$trusted_base/" in "$checkout/"*) fail 'restore utility must run outside checkout' ;; esac
locks
if [[ $mode == accept && $resume_accepted == true ]]; then
    # State was cleared only after complete signed acceptance. Resume only the
    # saved timer-start tail while the actual accepted process is unchanged.
    invocation=$(systemctl show "$backend" -p InvocationID --value)
    jq -e --arg invocation "$invocation" '.invocation_id == $invocation' "$state_dir/$environment.rollback-accepted" >/dev/null || fail 'accepted service changed before timer resume'
    while IFS= read -r unit; do
        [[ $unit =~ ^verdaxis-[a-z0-9-]+\.timer$ ]] || fail 'invalid saved timer name'
        jq -e --arg unit "$unit" '.timers | index($unit) != null' "$work/inventory.json" >/dev/null || fail 'saved timer not approved'
        systemctl start "$unit"
    done < <(jq -r '.timers[]' "$state_dir/$environment.rollback-accepted")
    rm -f -- "$state_dir/$environment.rollback-timers"
    rm -f -- "$state_dir/$environment.rollback-nonce"
    rm -f -- "$state_dir/$environment.rollback-accepted"
    sync -f "$state_dir"
    printf 'ACCEPTED timer_resume manifest_sha256=%s\n' "$approval"
    exit 0
fi
if [[ $mode == accept ]]; then
    [[ $(stat -c '%u:%a' "$state_dir/$environment.rollback-nonce") == "$deploy_uid:600" ]] || fail 'restore nonce ownership differs'
fi
# The signed acceptance is bound to this restore attempt. The current accept
# invocation reacquires both locks and writes fresh authorization only after checks.
if [[ $mode == restore ]]; then
    printf '%s\n' "$nonce" > "$work/nonce"
    publish "$work/nonce" "$state_dir/$environment.rollback-nonce" "$deploy_uid" "$(stat -c %g "$state_dir")" 0600
    # Remove all old migration credential paths before any source publication.
    while IFS= read -r path; do
        safe_path "$path"
        [[ ! -e $path ]] || { regular "$path"; rm -- "$path"; sync -f "$(dirname "$path")"; }
    done < <(jq -r '.files[] | select(.kind == "migrator_credential") | .path' "$work/inventory.json")
    # Publish the complete pre-stop snapshot first. A killed stop cannot lose
    # an originally active timer, and a retry retains the same snapshot.
    timers=$state_dir/$environment.rollback-timers
    if [[ ! -e $timers ]]; then
        : > "$work/active-timers"
        while IFS= read -r unit; do
            [[ $unit =~ ^verdaxis-[a-z0-9-]+\.timer$ ]] || fail 'invalid timer name'
            if systemctl is-active --quiet "$unit"; then printf '%s\n' "$unit" >> "$work/active-timers"; fi
        done < <(jq -r '.timers[]' "$work/inventory.json")
        publish "$work/active-timers" "$timers" "$deploy_uid" "$(stat -c %g "$state_dir")" 0600
    fi
    while IFS= read -r unit; do
        [[ $unit =~ ^verdaxis-[a-z0-9-]+\.timer$ ]] || fail 'invalid timer name'
        systemctl stop "$unit"
    done < <(jq -r '.timers[]' "$work/inventory.json")
    while IFS= read -r unit; do
        [[ $unit =~ ^verdaxis-[a-z0-9-]+\.service$ ]] || fail 'invalid job name'
        [[ $(systemctl show "$unit" -p ActiveState --value) == inactive ]] || fail 'direct job has not drained'
    done < <(jq -r '.jobs[]' "$work/inventory.json")
    # All other consumers must already be quiescent. Never kill an active
    # oneshot to manufacture a drained state or auto-start legacy Python.
    while IFS= read -r unit; do
        [[ $unit == "$backend" ]] && continue
        [[ $(systemctl show "$unit" -p ActiveState --value) == inactive ]] || fail 'non-serving consumer must be quiesced before restore'
    done < <(jq -r '.consumer_units[].name' "$work/inventory.json")
    systemctl stop "$backend"
    read_database
    check_package_closure
    [[ $catalog_hash == "$(jq -r .catalog_sha256 "$work/manifest.json")" ]] || fail 'application catalog differs'
    git init --bare "$work/source.git" >"$work/git-output" 2>&1
    git -C "$work/source.git" fetch "$bundle/checkout.bundle" HEAD >"$work/git-output" 2>&1
    [[ $(git -C "$work/source.git" rev-parse FETCH_HEAD) == "$release" ]] || fail 'source object identity differs'
    # Remove only tracked files absent from the saved release. Untracked user
    # files are never removed. Runtime files are published from the saved list.
    git -c "safe.directory=$checkout" -c core.hooksPath=/dev/null -C "$checkout" ls-files > "$work/current-files"
    while IFS= read -r path; do
        [[ $path =~ ^[A-Za-z0-9_./@+-]+$ && $path != /* && $path != *../* ]] || fail 'unsafe tracked source path'
        if ! git -C "$work/source.git" cat-file -e "$release:$path" 2>/dev/null; then
            safe_path "$checkout/$path"
            [[ ! -e $checkout/$path ]] || { regular "$checkout/$path"; rm -- "$checkout/$path"; sync -f "$(dirname "$checkout/$path")"; }
        fi
    done < "$work/current-files"
    while IFS=$'\t' read -r _metadata path; do
        [[ $path =~ ^[A-Za-z0-9_./@+-]+$ && $path != /* && $path != *../* ]] || fail 'unsafe saved source path'
        saved_metadata=$(grep -F $'\t'"$path" "$bundle/source-metadata")
        [[ $saved_metadata == *$'\t'"$path" && $saved_metadata != *$'\n'* ]] || fail 'source metadata is missing or repeated'
        IFS=: read -r source_uid source_gid permissions source_size <<< "${saved_metadata%%$'\t'*}"
        [[ $source_uid =~ ^[0-9]+$ && $source_gid =~ ^[0-9]+$ && $permissions =~ ^[0-7]{3,4}$ ]] || fail 'invalid source metadata'
        git -C "$work/source.git" show "$release:$path" > "$work/source-file"
        [[ $(stat -c %s "$work/source-file") == "$source_size" ]] || fail 'source size differs'
        parent=$(dirname "$checkout/$path")
        # Parent creation is permitted only inside the fixed checkout.
        safe_path "$parent"; mkdir -p -- "$parent"; safe_path "$checkout/$path"
        publish "$work/source-file" "$checkout/$path" "$source_uid" "$source_gid" "$permissions"
    done < "$bundle/source-tree"
    git -c "safe.directory=$checkout" -c core.hooksPath=/dev/null -C "$checkout" fetch "$bundle/checkout.bundle" HEAD >"$work/git-output" 2>&1
    # Git objects are immutable prerequisites. Publish index and branch ref
    # as explicit transaction entries, not an in-place branch reset.
    git -C "$work/source.git" read-tree "$release"
    git_dir=$(git -c "safe.directory=$checkout" -c core.hooksPath=/dev/null -C "$checkout" rev-parse --absolute-git-dir)
    publish "$work/source.git/index" "$git_dir/index" "$deploy_uid" "$(stat -c %g "$checkout")" 0644
    branch=$(jq -r .checkout_branch "$work/inventory.json")
    [[ $(git -c "safe.directory=$checkout" -c core.hooksPath=/dev/null -C "$checkout" symbolic-ref --short HEAD) == "$branch" ]] || fail 'canonical checkout branch differs'
    printf '%s\n' "$release" > "$work/source-ref"
    publish "$work/source-ref" "$git_dir/refs/heads/$branch" "$deploy_uid" "$(stat -c %g "$checkout")" 0644
    while IFS= read -r row; do
        [[ $(jq -r .kind <<< "$row") != migrator_credential ]] || continue
        publish "$bundle/payload/$(jq -r .id <<< "$row")" "$(jq -r .path <<< "$row")" \
            "$(jq -r .uid <<< "$row")" "$(jq -r .gid <<< "$row")" "$(jq -r .mode <<< "$row")"
    done < <(jq -c '.files[]' "$work/manifest.json")
    check_runtime_credentials
    systemctl daemon-reload
    check_consumers
    if grep -qE '^[[:space:]]*import[[:space:]]' "$caddy"; then fail 'saved Caddy import closure unsupported'; fi
    runuser -u caddy -- caddy validate --config "$caddy" --adapter caddyfile >"$work/caddy-output" 2>&1 || fail 'saved Caddy bytes are not valid'
    runuser -u caddy -- caddy reload --config "$caddy" --adapter caddyfile >"$work/caddy-output" 2>&1 || fail 'saved Caddy reload failed'
    write_state restart-authorized
    # systemd does not inherit these descriptors. Close them in the client so
    # a fixture child cannot retain deploy ownership after this process exits.
    (
        for fd in "${lock_fds[@]}"; do exec {fd}>&-; done
        systemctl restart "$backend"
    )
    printf 'RESTORED acceptance_required nonce=%s manifest_sha256=%s\n' "$nonce" "$approval"
    exit 0
fi
# Accept is a new lock-held attempt. Never reuse a prior restart authorization.
# Task 31 signs the read-only acceptance result after this invocation supplies
# its nonce; a missing/freshness mismatch remains blocked and prints NO_GO.
[[ -n $evidence && -n $signature && -n $key ]] || fail 'signed auth/session/monitor/passive acceptance required'
private_file "$evidence"; private_file "$signature"; regular "$key"
[[ $(hash "$key") == "$(jq -r .acceptance_key_sha256 "$work/inventory.json")" ]] || fail 'acceptance signer is not independently approved'
openssl pkeyutl -verify -pubin -rawin -inkey "$key" -sigfile "$signature" -in "$evidence" >"$work/signature-output" 2>&1 || fail 'acceptance signature differs'
jq -e --arg sha "$release" --arg env "$environment" --arg digest "$approval" --arg nonce "$nonce" \
    '.environment == $env and .release_sha == $sha and .manifest_sha256 == $digest and .nonce == $nonce and
     .auth == "PASS" and .session == "PASS" and .monitor == "PASS" and .passive == "PASS" and
     .old_compiled_pin == "PASS" and .migration_freeze == "PASS" and
     (.monitor_invocations | type == "array" and length >= 2)' "$evidence" >/dev/null || fail 'bound acceptance incomplete'
read_database
[[ $catalog_hash == "$(jq -r .catalog_sha256 "$work/manifest.json")" ]] || fail 'application catalog differs'
check_consumers
check_package_closure
[[ $(git -c "safe.directory=$checkout" -c core.hooksPath=/dev/null -C "$checkout" rev-parse HEAD) == "$release" && -z $(git -c "safe.directory=$checkout" -c core.hooksPath=/dev/null -C "$checkout" status --porcelain --untracked-files=no) ]] || fail 'restored checkout identity or bytes differ'
while IFS=$'\t' read -r metadata source_path; do
    regular "$checkout/$source_path"
    [[ $(stat -c '%u:%g:%a:%s' "$checkout/$source_path") == "$metadata" ]] || fail 'restored source metadata differs'
done < "$bundle/source-metadata"
while IFS= read -r row; do
    path=$(jq -r .path <<< "$row"); kind=$(jq -r .kind <<< "$row")
    if [[ $kind == migrator_credential ]]; then
        [[ ! -e $path && ! -L $path ]] || fail 'migration credential fence absent'
    else
        regular "$path"
        [[ $(hash "$path") == "$(jq -r .sha256 <<< "$row")" ]] || fail 'installed release bytes differ'
        [[ $(stat -c '%u:%g:%a:%s' "$path") == "$(jq -r '[.uid,.gid,.mode,.size] | map(tostring) | join(":")' <<< "$row")" ]] || fail 'installed release metadata differs'
    fi
done < <(jq -c '.files[]' "$work/manifest.json")
pid=$(systemctl show "$backend" -p MainPID --value)
invocation=$(systemctl show "$backend" -p InvocationID --value)
[[ $pid =~ ^[1-9][0-9]*$ && $invocation =~ ^[0-9a-f]{32}$ ]] || fail 'old service did not start'
[[ $(hash "/proc/$pid/exe") == "$(hash "$(payload_of "$binary")")" ]] || fail 'running executable is not the captured old executable'
timeout 15 curl --fail --silent --show-error --max-time 10 "$ready" > "$work/ready" 2>"$work/curl-error" || fail 'readiness request failed'
jq -e --arg env "$environment" --arg sha "$release" 'keys == ["db","environment","release_sha","status"] and .status == "ok" and .db == "ok" and .environment == $env and .release_sha == $sha' "$work/ready" >/dev/null || fail 'exact readiness differs'
for unit in verdaxis-health.service verdaxis-backup-verify.service; do
    monitor_invocation=$(systemctl show "$unit" -p InvocationID --value)
    [[ $monitor_invocation =~ ^[0-9a-f]{32}$ ]] || fail 'captured monitor has no accepted invocation'
    [[ $(systemctl show "$unit" -p Result --value) == success ]] || fail 'monitor invocation failed'
    jq -e --arg unit "$unit" --arg invocation "$monitor_invocation" 'any(.monitor_invocations[]; .unit == $unit and .invocation_id == $invocation)' "$evidence" >/dev/null || fail 'monitor acceptance names another invocation'
done
# Acceptance must name the current actual service invocation, not a previous run.
jq -e --arg invocation "$invocation" '.invocation_id == $invocation' "$evidence" >/dev/null || fail 'acceptance names another service invocation'
write_state restart-authorized
jq -n --arg digest "$approval" --arg nonce "$nonce" --arg invocation "$invocation" \
    --rawfile timers "$state_dir/$environment.rollback-timers" \
    '{manifest_sha256:$digest,nonce:$nonce,invocation_id:$invocation,timers:($timers | split("\n") | map(select(length > 0)))}' > "$work/accepted"
publish "$work/accepted" "$state_dir/$environment.rollback-accepted" "$deploy_uid" "$(stat -c %g "$state_dir")" 0600
rm -- "$state"; sync -f "$state_dir"
while IFS= read -r unit; do
    [[ $unit =~ ^verdaxis-[a-z0-9-]+\.timer$ ]] || fail 'saved timer name invalid'
    jq -e --arg unit "$unit" '.timers | index($unit) != null' "$work/inventory.json" >/dev/null || fail 'saved timer not approved'
    systemctl start "$unit"
done < <(jq -r '.timers[]' "$state_dir/$environment.rollback-accepted")
rm -f -- "$state_dir/$environment.rollback-timers"
rm -f -- "$state_dir/$environment.rollback-nonce"
rm -f -- "$state_dir/$environment.rollback-accepted"
sync -f "$state_dir"
printf 'ACCEPTED manifest_sha256=%s\n' "$approval"
