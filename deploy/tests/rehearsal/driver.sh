#!/usr/bin/env bash
# Sourced only after the non-mutating entry gate.

CHECK_NAMES=(
    host_inventory_is_complete_and_signed
    host_inventory_covers_units_timers_dropins_cron_usr_local_backup_and_processes
    nightly_backup_and_staging_pull_schedules_are_unchanged
    rehearsal_runs_every_installed_unit_once
    rehearsal_deletes_python_and_nothing_restarts
    rehearsal_rollback_is_one_change_per_group
    release_bundle_rollback_starts_the_actual_pre_m_binary
    rollback_interruption_at_each_publication_never_starts_a_mixed_bundle
    no_python_process_remains_after_the_rehearsal
    every_required_scenario_has_evidence
)
rehearsal_hash() { sha256sum -- "$1" | cut -d ' ' -f 1; }
rehearsal_result() {
    local name=$1 status=$2 evidence=$3 digest
    digest=$(rehearsal_hash "$PUBLIC/$evidence")
    install -d -m 0700 "$PUBLIC/observations"
    jq -n --arg name "$name" --arg status "$status" --arg path "$evidence" --arg digest "$digest" \
        --arg environment "$REHEARSAL_ENVIRONMENT" --arg sha "$CURRENT_SHA" --arg run "$GITHUB_RUN_ID" \
        '{schema_version:1,check:$name,environment:$environment,release_sha:$sha,run_id:$run,outcome:$status,producer:"deploy/tests/full-rehearsal.sh",observations:[{path:$path,sha256:$digest}]}' \
        > "$PUBLIC/observations/$name.json"
    evidence="observations/$name.json"
    digest=$(rehearsal_hash "$PUBLIC/$evidence")
    jq -n --arg name "$name" --arg status "$status" --arg path "$evidence" --arg digest "$digest" \
        '{name:$name,status:$status,evidence:[{path:$path,sha256:$digest}]}' >> "$PRIVATE/results.jsonl"
    printf '%s %s evidence=%s sha256=%s\n' "$status" "$name" "$evidence" "$digest"
}
rehearsal_finish() {
    local exit_status=$? name
    trap - EXIT
    # A failed prerequisite blocks every dependent assertion. No missing case is
    # silently skipped, and protected logs are never copied into public evidence.
    for name in "${CHECK_NAMES[@]}"; do
        if ! jq -e --arg name "$name" 'select(.name==$name)' "$PRIVATE/results.jsonl" >/dev/null; then
            printf 'Prerequisite failed; no acceptance was observed. stage=%s\n' "${STAGE:-initialization}" > "$PUBLIC/$name.txt"
            rehearsal_result "$name" FAIL "$name.txt"
        fi
    done
    local names
    names=$(printf '%s\n' "${CHECK_NAMES[@]}" | jq -R . | jq -s .)
    jq -s --arg environment "$REHEARSAL_ENVIRONMENT" --arg sha "$CURRENT_SHA" --argjson names "$names" --arg run "$GITHUB_RUN_ID" \
        '. as $checks | {schema_version:1,scope:"disposable-candidate",environment:$environment,release_sha:$sha,run_id:$run,checks:[$names[] as $name | $checks[] | select(.name==$name)]}' \
        "$PRIVATE/results.jsonl" > "$PUBLIC/results.json"
    if [[ -f $PRIVATE/key.pem ]]; then
        openssl pkeyutl -sign -rawin -inkey "$PRIVATE/key.pem" -in "$PUBLIC/results.json" -out "$PUBLIC/results.json.sig"
        install -m 0600 "$PRIVATE/key.pub.pem" "$PUBLIC/disposable-signer.pub.pem"
    fi
    if jq -e 'all(.checks[];.status=="PASS")' "$PUBLIC/results.json" >/dev/null && (( exit_status == 0 )); then
        /usr/bin/python3 "$ROOT/tests/tooling/phase10/rehearsal_evidence.py" "$PUBLIC" \
            --environment "$REHEARSAL_ENVIRONMENT" --release-sha "$CURRENT_SHA" || exit 1
        exit 0
    fi
    printf 'FAIL full_rehearsal: stage=%s; protected logs retained outside upload directory\n' "${STAGE:-initialization}" >&2
    exit 1
}
rehearsal_main() {
    local path
    for path in "$REHEARSAL_SOURCE_DIR" "$REHEARSAL_CURRENT_ARTIFACT_DIR" "$REHEARSAL_PREVIOUS_ARTIFACT_DIR"; do
        [[ $path == /* && -d $path && ! -L $path ]] || { printf 'FAIL input path\n' >&2; return 2; }
    done
    [[ $REHEARSAL_EVIDENCE_DIR == /* && ! -e $REHEARSAL_EVIDENCE_DIR && ! -L $REHEARSAL_EVIDENCE_DIR ]] || { printf 'FAIL evidence output must be new\n' >&2; return 2; }
    [[ ${GITHUB_RUN_ID:-} =~ ^[0-9]+$ ]] || return 2
    for path in /home/verdaxis-prod /var/lib/verdaxis /etc/verdaxis /run/verdaxis-disposable-rehearsal; do
        [[ ! -e $path && ! -L $path ]] || { printf 'FAIL occupied disposable namespace: %s\n' "$path" >&2; return 2; }
    done
    for path in jq openssl git curl systemctl psql getfacl setfacl caddy timeout runuser; do command -v "$path" >/dev/null || return 2; done
    CURRENT_SHA=$(git -C "$ROOT" rev-parse HEAD)
    [[ $CURRENT_SHA =~ ^[a-f0-9]{40}$ ]] || return 2
    install -d -m 0700 "$REHEARSAL_EVIDENCE_DIR"
    PUBLIC=$REHEARSAL_EVIDENCE_DIR/public
    PRIVATE=/var/lib/verdaxis-rehearsal-$GITHUB_RUN_ID
    [[ ! -e $PRIVATE ]] || return 2
    install -d -m 0700 "$PUBLIC" "$PRIVATE"
    : > "$PRIVATE/results.jsonl"
    trap rehearsal_finish EXIT
    STAGE=prepare
    REHEARSAL_CHECKOUT=/home/verdaxis-prod/verdaxis/staging/be
    [[ $REHEARSAL_ENVIRONMENT != production ]] || REHEARSAL_CHECKOUT=/home/verdaxis-prod/verdaxis/prod/be
    REHEARSAL_PUBLIC_KEY=/etc/verdaxis/artifact-provenance.pub.pem
    export PUBLIC PRIVATE CURRENT_SHA REHEARSAL_CHECKOUT REHEARSAL_PUBLIC_KEY
    export VERDAXIS_DISPOSABLE_DEPLOY=1
    /usr/bin/python3 "$ROOT/deploy/tests/rehearsal/stack.py" prepare > "$PRIVATE/prepare.log" 2>&1
    STAGE=inventory
    /usr/bin/python3 "$ROOT/deploy/tests/rehearsal/stack.py" inventory > "$PRIVATE/inventory.log" 2>&1
    rehearsal_result "${CHECK_NAMES[0]}" PASS inventory.json
    rehearsal_result "${CHECK_NAMES[1]}" PASS inventory.json
    STAGE=pre-m-capture
    /usr/bin/python3 "$ROOT/deploy/tests/rehearsal/stack.py" capture > "$PRIVATE/capture.log" 2>&1
    STAGE=window
    /usr/bin/python3 "$ROOT/deploy/tests/rehearsal/stack.py" window > "$PRIVATE/window.log" 2>&1
    rehearsal_result "${CHECK_NAMES[3]}" PASS units.json
    rehearsal_result "${CHECK_NAMES[5]}" PASS groups.json
    STAGE=rollback
    /usr/bin/python3 "$ROOT/deploy/tests/rehearsal/stack.py" rollback > "$PRIVATE/rollback.log" 2>&1
    rehearsal_result "${CHECK_NAMES[6]}" PASS rollback.json
    STAGE=interruptions
    /usr/bin/python3 "$ROOT/deploy/tests/rehearsal/stack.py" interruptions > "$PRIVATE/interruptions.log" 2>&1
    rehearsal_result "${CHECK_NAMES[7]}" PASS interruptions.json
    STAGE=forward
    /usr/bin/python3 "$ROOT/deploy/tests/rehearsal/stack.py" forward > "$PRIVATE/forward.log" 2>&1
    STAGE=w5
    # All Python observers have exited before deletion and final process checks.
    # shellcheck source=deploy/tests/rehearsal/w5.sh
    source "$ROOT/deploy/tests/rehearsal/w5.sh"
    rehearsal_w5
    rehearsal_result "${CHECK_NAMES[4]}" PASS w5.txt
    rehearsal_result "${CHECK_NAMES[8]}" PASS processes.txt
    rehearsal_result "${CHECK_NAMES[2]}" PASS schedules.txt
    STAGE=evidence
    jq -e 'length==9 and all(.[];.status=="PASS")' < <(jq -s . "$PRIVATE/results.jsonl") >/dev/null
    printf 'All nine scenario assertions completed with digest-bound evidence.\n' > "$PUBLIC/coverage.txt"
    rehearsal_result "${CHECK_NAMES[9]}" PASS coverage.txt
    STAGE=complete
}
