#!/usr/bin/env bash
# Ordered, install-separated job handover. No live authority is implied.
set -euo pipefail

fail() { printf 'W2 NO_GO: %s\n' "$*" >&2; return 1; }
usage() {
    printf 'usage: %s --action install|advance|accept|rollback|status --environment production|staging --source-ref FULL_SHA --binary-sha256 DIGEST [--group GROUP] [--evidence FILE] [--timer-inventory FILE]\n' "$0" >&2
    exit 2
}
load_groups() {
    GROUPS_ORDER=(news auth-prune reminders onboarding demo local-monitor external-recovery)
    local suffix=''
    [[ $ENVIRONMENT == production ]] || suffix=-staging
    JOB_TIMERS=("verdaxis-news-refresh$suffix.timer" "verdaxis-auth-maintenance$suffix.timer" "verdaxis-product-analytics-prune$suffix.timer")
    if [[ $ENVIRONMENT == production ]]; then
        JOB_TIMERS+=(verdaxis-order-expiry-reminders.timer verdaxis-onboarding-attention.timer)
    fi
    JOB_TIMERS+=("verdaxis-demo-activity$suffix.timer")
}
group_units() {
    local group=$1 suffix=''
    [[ $ENVIRONMENT == production ]] || suffix=-staging
    case $group in
        news) printf 'verdaxis-news-refresh%s.service\n' "$suffix" ;;
        auth-prune) printf 'verdaxis-auth-maintenance%s.service\nverdaxis-product-analytics-prune%s.service\n' "$suffix" "$suffix" ;;
        reminders) [[ $ENVIRONMENT != production ]] || printf 'verdaxis-order-expiry-reminders.service\n' ;;
        onboarding) [[ $ENVIRONMENT != production ]] || printf 'verdaxis-onboarding-attention.service\n' ;;
        demo) printf 'verdaxis-demo-activity%s.service\n' "$suffix" ;;
        local-monitor) printf '%s\n' verdaxis-health.service verdaxis-backup-verify.service verdaxis-monitor-alert@health.service verdaxis-monitor-alert@backup.service verdaxis-monitor-alert-reminder.service ;;
        external-recovery) printf '%s\n' verdaxis-monitor.service verdaxis-monitor-verify.service ;;
        *) fail 'unknown group' ;;
    esac
}
group_timers() {
    local unit
    case $1 in
        local-monitor) printf '%s\n' verdaxis-health.timer verdaxis-backup-verify.timer verdaxis-monitor-alert-reminder.timer ;;
        external-recovery) printf 'verdaxis-monitor.timer\n' ;;
        *) while IFS= read -r unit; do [[ -z $unit ]] || printf '%s.timer\n' "${unit%.service}"; done < <(group_units "$1") ;;
    esac
}
service_ctl() { /usr/bin/systemctl "$@"; }
service_value() { /usr/bin/systemctl show --value --property="$2" "$1"; }
group_phase() { if [[ -f $STATE_DIR/$1/phase ]]; then cat "$STATE_DIR/$1/phase"; else printf absent; fi; }
set_phase() {
    printf '%s\n' "$2" > "$STATE_DIR/$1/phase.tmp" || return
    mv -- "$STATE_DIR/$1/phase.tmp" "$STATE_DIR/$1/phase" || return
}
# Save complete regular-file unit/drop-in ABI before the install-only operation.
snapshot_dropins() {
    local group=$1 unit=$2 path name directory
    [[ $unit != *@*.service ]] || unit=${unit%@*}@.service
    directory=$UNIT_DIR/$unit.d
    [[ ! -L $directory ]] || { fail 'redirected drop-in directory'; return 1; }
    if [[ -d $directory ]]; then
        mkdir -p "$STATE_DIR/$group/dropins/$unit.d" || return
        for path in "$directory/"*; do
            [[ -e $path ]] || continue
            name=${path##*/}
            [[ $name =~ ^[a-zA-Z0-9_.-]+\.conf$ && -f $path && ! -L $path ]] || { fail 'unsafe installed drop-in'; return 1; }
            cp -p "$path" "$STATE_DIR/$group/dropins/$unit.d/$name" || return
        done
    fi
    for path in $(service_value "$unit" DropInPaths); do
        [[ $path == "$directory/"* && -f $path && ! -L $path ]] || { fail 'drop-in outside inventoried system directory'; return 1; }
    done
}
snapshot_group() {
    local group=$1 unit file
    [[ ! -e $STATE_DIR/$group ]] || { fail 'group already captured'; return 1; }
    mkdir -m 0700 "$STATE_DIR/$group" || return
    mkdir -m 0700 "$STATE_DIR/$group/previous" || return
    while IFS= read -r unit; do
        [[ -n $unit ]] || continue
        snapshot_dropins "$group" "$unit" || return
        file=$UNIT_DIR/$unit
        if [[ $unit == *@*.service ]]; then file=$UNIT_DIR/${unit%@*}@.service; fi
        if [[ -e $file || -L $file ]]; then
            [[ -f $file && ! -L $file ]] || { fail 'unit must be a regular file'; return 1; }
            cp -p -- "$file" "$STATE_DIR/$group/previous/${file##*/}" || return
        else
            printf '%s\n' "${file##*/}" >> "$STATE_DIR/$group/absent" || return
        fi
    done < <(group_units "$group"; group_timers "$group")
    : > "$STATE_DIR/$group/active-timers" || return
    while IFS= read -r unit; do
        [[ -n $unit ]] || continue
        if [[ $(service_value "$unit" ActiveState) == active ]]; then
            printf '%s\n' "$unit" >> "$STATE_DIR/$group/active-timers" || return
        fi
    done < <(group_timers "$group")
    : > "$STATE_DIR/$group/snapshot.tsv" || return
    for file in "$STATE_DIR/$group/previous/"* "$STATE_DIR/$group/dropins/"*/*.conf "$STATE_DIR/$group/active-timers" "$STATE_DIR/$group/absent"; do
        [[ -f $file ]] || continue
        printf '%s\t%s\n' "${file#"$STATE_DIR/$group/"}" "$(file_digest "$file")" >> "$STATE_DIR/$group/snapshot.tsv" || return
    done
    verify_snapshot "$group" || return
    set_phase "$group" captured || return
}
verify_snapshot() {
    local group=$1 relative digest
    [[ -s $STATE_DIR/$group/snapshot.tsv ]] || { fail 'snapshot manifest absent'; return 1; }
    while IFS=$'\t' read -r relative digest; do
        [[ $relative =~ ^(previous/verdaxis-[a-z0-9@.-]+|dropins/verdaxis-[a-z0-9@.-]+\.d/[a-zA-Z0-9_.-]+\.conf|active-timers|absent)$ && $digest =~ ^[a-f0-9]{64}$ ]] || { fail 'snapshot manifest invalid'; return 1; }
        [[ -f $STATE_DIR/$group/$relative && ! -L $STATE_DIR/$group/$relative && $(file_digest "$STATE_DIR/$group/$relative") == "$digest" ]] || { fail 'saved unit bytes changed'; return 1; }
    done < "$STATE_DIR/$group/snapshot.tsv"
}
service_instances() {
    /usr/bin/systemctl list-units --all --plain --no-legend 'verdaxis-monitor-alert@*.service' | awk '{print $1}'
}
drain_units() {
    group_units "$1"
    if [[ $1 == local-monitor || $1 == external-recovery ]]; then
        local unit instances
        instances=$(service_instances) || return
        while IFS= read -r unit; do
            [[ -n $unit ]] || continue
            [[ $unit =~ ^verdaxis-monitor-alert@[a-z0-9-]+\.service$ ]] || { fail 'unexpected event instance'; return 1; }
            printf '%s\n' "$unit"
        done <<< "$instances"
    fi
}
quiesce_group() {
    local group=$1 unit state job attempt units
    while IFS= read -r unit; do
        [[ -z $unit ]] || service_ctl stop "$unit" || return
    done < <(group_timers "$group")
    # Do not kill an admitted oneshot. A bounded wait leaves the fence in place.
    for ((attempt=0; attempt<120; attempt++)); do
        local busy=0
        units=$(drain_units "$group") || return
        while IFS= read -r unit; do
            [[ -n $unit ]] || continue
            state=$(service_value "$unit" ActiveState)
            job=$(service_value "$unit" Job)
            case $state in active|activating|deactivating|reloading) busy=1 ;; inactive|failed) ;; *) fail "unknown service state: $unit"; return 1 ;; esac
            [[ $job == 0 || -z $job ]] || busy=1
        done <<< "$units"
        (( busy )) || return 0
        sleep 1
    done
    fail 'oneshots or pending jobs did not drain; timers remain stopped'
}
check_order() {
    local candidate
    for candidate in "${GROUPS_ORDER[@]}"; do
        [[ $candidate != "$1" ]] || return 0
        [[ $(group_phase "$candidate") == accepted ]] || { fail "preceding group is not accepted: $candidate"; return 1; }
    done
    fail 'unknown group'
}
unit_source() {
    local unit=$1
    [[ $unit != *@*.service ]] || unit=${unit%@*}@.service
    case $unit in
        verdaxis-news-*|verdaxis-auth-*|verdaxis-product-*|verdaxis-order-*) printf '%s/deploy/systemd/%s\n' "$SOURCE_ROOT" "$unit" ;;
        verdaxis-onboarding-*|verdaxis-monitor.service|verdaxis-monitor.timer|verdaxis-monitor-verify.service) printf '%s/deploy/external_monitor/systemd/%s\n' "$SOURCE_ROOT" "$unit" ;;
        *) printf '%s/deploy/monitor/%s\n' "$SOURCE_ROOT" "$unit" ;;
    esac
}
verify_native_unit() {
    local unit=$1 source destination
    source=$(unit_source "$unit")
    destination=$UNIT_DIR/$unit
    [[ $unit != *@*.service ]] || destination=$UNIT_DIR/${unit%@*}@.service
    [[ -f $source && -f $destination && ! -L $destination ]] || { fail 'missing reviewed unit'; return 1; }
    cmp -s -- "$source" "$destination" || { fail "installed bytes differ: $unit"; return 1; }
    local dropin group saved
    for dropin in $(service_value "$unit" DropInPaths); do
        saved=''
        for group in "${GROUPS_ORDER[@]}"; do
            if [[ -f $STATE_DIR/$group/dropins/${dropin#"$UNIT_DIR/"} ]]; then
                verify_snapshot "$group" || return
                saved=$STATE_DIR/$group/dropins/${dropin#"$UNIT_DIR/"}
            fi
        done
        if [[ -z $saved || ! -f $dropin || -L $dropin ]] || ! cmp -s "$saved" "$dropin"; then
            fail 'unexpected or changed unit drop-in'; return 1
        fi
        ! grep -Eq '^(Exec|OnFailure|OnSuccess|User=|Group=|Environment|Condition|Assert)' "$dropin" || { fail 'command or identity drop-in needs separate native review'; return 1; }
    done
    [[ $(service_value "$unit" LoadState) == loaded ]] || { fail 'unit is not loaded'; return 1; }
    if [[ $unit == *.service ]]; then
        grep -Eq '^ExecStart=.*verdaxis-read (job|monitor) ' "$source" || { fail 'unit has no native command'; return 1; }
        ! grep -Eiq '^(ExecStart|ExecStartPre|ExecStartPost)=.*(python|venv)' "$source" || { fail 'unit still invokes retained runtime'; return 1; }
    fi
}
verify_ready() {
    local port=8000
    [[ $ENVIRONMENT == production ]] || port=8001
    /usr/bin/curl --fail --silent --show-error --max-time 5 "http://127.0.0.1:$port/health/ready" |
        jq -e --arg environment "$ENVIRONMENT" --arg sha "$SOURCE_REF" 'keys == ["db","environment","release_sha","status"] and .status == "ok" and .db == "ok" and .environment == $environment and .release_sha == $sha' >/dev/null
}
verify_fence_inventory() {
    local group=$1 path digest metadata actual
    # Exact paths originate in the fresh W0 inventory. This helper neither runs
    # their contents nor broadens permissions. Disabled launchers must be root
    # owned and unreadable/unexecutable by every runtime identity (mode 000).
    jq -e --arg group "$group" '.groups[$group].direct_launchers_fenced == true and .groups[$group].pending_callers_reconciled == true
      and (.groups[$group].fences | type == "array" and length > 0)
      and all(.groups[$group].fences[]; keys == ["original_gid","original_mode","original_uid","path","sha256"]
        and (.path | type == "string" and startswith("/") and (contains("..") | not))
        and (.sha256 | type == "string" and test("^[a-f0-9]{64}$")))' "$EVIDENCE" >/dev/null || { fail 'bound direct-launcher fence inventory missing'; return 1; }
    while IFS=$'\t' read -r path digest; do
        case $path in /usr/local/sbin/verdaxis-*|/usr/local/libexec/verdaxis-*.py|"$SOURCE_ROOT/"*.py|"$SOURCE_ROOT/venv/bin/python"*) ;; *) fail 'fence path outside owned launcher namespaces'; return 1 ;; esac
        [[ -f $path && ! -L $path ]] || { fail 'fenced launcher is redirected or absent'; return 1; }
        actual=$path
        while [[ $actual != / ]]; do [[ ! -L $actual ]] || { fail 'redirected fence ancestor'; return 1; }; actual=${actual%/*}; [[ -n $actual ]] || actual=/; done
        metadata=$(stat -c '%u:%a' "$path")
        [[ $metadata == 0:0 && $(file_digest "$path") == "$digest" ]] || { fail 'launcher fence bytes, ownership or mode differ'; return 1; }
    done < <(jq -r --arg group "$group" '.groups[$group].fences[] | [.path,.sha256] | @tsv' "$EVIDENCE")
}
verify_direct_fences() { verify_fence_inventory "$1"; }
capture_launcher_permissions() {
    local group=$1 path digest uid gid mode
    [[ ! -f $STATE_DIR/$group/launchers.tsv ]] || return 0
    : > "$STATE_DIR/$group/launchers.tsv" || return
    jq -e --arg group "$group" 'all(.groups[$group].fences[];
      (.original_uid | type == "number" and . >= 0) and (.original_gid | type == "number" and . >= 0)
      and (.original_mode | type == "string" and test("^[0-7]{3}$")))' "$EVIDENCE" >/dev/null || { fail 'original launcher permission inventory missing'; return 1; }
    while IFS=$'\t' read -r path digest uid gid mode; do
        printf '%s\t%s\t%s\t%s\t%s\n' "$path" "$digest" "$uid" "$gid" "$mode" >> "$STATE_DIR/$group/launchers.tsv" || return
    done < <(jq -r --arg group "$group" '.groups[$group].fences[] | [.path,.sha256,.original_uid,.original_gid,.original_mode] | @tsv' "$EVIDENCE")
    file_digest "$STATE_DIR/$group/launchers.tsv" > "$STATE_DIR/$group/launchers.sha256" || return
}
restore_launcher_permissions() {
    local group=$1 path digest uid gid mode
    [[ -f $STATE_DIR/$group/launchers.tsv ]] || return 0
    [[ -f $STATE_DIR/$group/launchers.sha256 && $(file_digest "$STATE_DIR/$group/launchers.tsv") == $(cat "$STATE_DIR/$group/launchers.sha256") ]] || { fail 'saved permission inventory changed'; return 1; }
    while IFS=$'\t' read -r path digest uid gid mode; do
        [[ -f $path && ! -L $path && $(file_digest "$path") == "$digest" && $uid =~ ^[0-9]+$ && $gid =~ ^[0-9]+$ && $mode =~ ^[0-7]{3}$ ]] || { fail 'saved launcher permission identity differs'; return 1; }
        chown "$uid:$gid" "$path" || return
        chmod "$mode" "$path" || return
    done < "$STATE_DIR/$group/launchers.tsv"
}
refence_launchers() {
    local group=$1 path digest uid gid mode
    [[ -f $STATE_DIR/$group/launchers.tsv ]] || return 0
    [[ -f $STATE_DIR/$group/launchers.sha256 && $(file_digest "$STATE_DIR/$group/launchers.tsv") == $(cat "$STATE_DIR/$group/launchers.sha256") ]] || { fail 'saved permission inventory changed'; return 1; }
    while IFS=$'\t' read -r path digest uid gid mode; do
        [[ -f $path && ! -L $path && $(file_digest "$path") == "$digest" ]] || { fail 'launcher identity differs before native ownership'; return 1; }
        chown 0:0 "$path" || return
        chmod 000 "$path" || return
    done < "$STATE_DIR/$group/launchers.tsv"
}
release_alert_ownership() {
    # The native command projects confirmed receipts and refuses pending or
    # uncertain heads. Never chmod the stable lock or copy old receipts here.
    /usr/sbin/runuser -u verdaxis-monitor-alert -- /usr/local/libexec/verdaxis/verdaxis-read monitor alert --release health || return
    if ! /usr/sbin/runuser -u verdaxis-monitor-alert -- /usr/local/libexec/verdaxis/verdaxis-read monitor alert --release backup; then
        # Re-enter native ownership of the already-empty health check. The
        # inventoried direct-launcher permission fence remains held throughout.
        /usr/sbin/runuser -u verdaxis-monitor-alert -- /usr/local/libexec/verdaxis/verdaxis-read monitor alert --event health || return
        fail 'backup release refused; health returned to native ownership'
        return 1
    fi
}
verify_recovery_retirement() {
    local unit state source destination rust_unit=verdaxis-rust.service
    [[ $ENVIRONMENT == production ]] || rust_unit=verdaxis-rust-staging.service
    for unit in verdaxis-recover.service verdaxis-codex-diagnose.service; do
        state=$(service_value "$unit" LoadState)
        [[ $state == masked || $state == not-found ]] || { fail 'retired recovery or diagnosis is loadable'; return 1; }
        state=$(service_value "$unit" ActiveState)
        [[ $state == inactive || $state == failed ]] || { fail 'retired recovery or diagnosis is active'; return 1; }
    done
    for unit in caddy.service postgresql@17-main.service; do
        source=$SOURCE_ROOT/deploy/systemd/overrides/$unit.d/10-verdaxis-restart.conf
        destination=$UNIT_DIR/$unit.d/10-verdaxis-restart.conf
        if [[ ! -f $source || ! -f $destination || -L $destination ]] || ! cmp -s "$source" "$destination"; then
            fail 'reviewed restart replacement is not installed'; return 1
        fi
        [[ $(service_value "$unit" Restart) == on-failure ]] || { fail 'restart policy is not loaded'; return 1; }
    done
    [[ $(service_value "$rust_unit" WatchdogUSec) == '1min' && $(service_value "$rust_unit" NotifyAccess) == main && $(service_value "$rust_unit" Restart) == on-failure ]] || { fail 'serve watchdog replacement is not loaded'; return 1; }
    jq -e '.recovery_retirement.accepted == true and (.recovery_retirement.proof_sha256 | test("^[a-f0-9]{64}$"))' "$EVIDENCE" >/dev/null || { fail 'recovery retirement acceptance missing'; return 1; }
}
publish_native_group() {
    local group=$1 unit source destination
    while IFS= read -r unit; do
        [[ -n $unit ]] || continue
        source=$(unit_source "$unit")
        destination=$UNIT_DIR/$unit
        [[ $unit != *@*.service ]] || destination=$UNIT_DIR/${unit%@*}@.service
        [[ -f $source && ! -L $source ]] || { fail 'native source missing'; return 1; }
        install -m 0644 -o root -g root "$source" "$destination.w2-pending" || return
        cmp -s "$source" "$destination.w2-pending" || return
        mv -- "$destination.w2-pending" "$destination" || return
    done < <(group_units "$group"; group_timers "$group")
    service_ctl daemon-reload
}
run_group() {
    local group=$1 unit before after result status
    if [[ $group == local-monitor || $group == external-recovery ]]; then
        [[ ${HOST_SCOPE:-environment} == host ]] || { fail 'group requires separately inventoried host ownership'; return 1; }
        [[ -f /usr/local/libexec/verdaxis/verdaxis-read && ! -L /usr/local/libexec/verdaxis/verdaxis-read && $(file_digest /usr/local/libexec/verdaxis/verdaxis-read) == "$BINARY_SHA256" ]] || { fail 'installed host executable digest differs'; return 1; }
    fi
    if [[ $group == external-recovery ]]; then verify_recovery_retirement || return; fi
    if [[ $group == demo ]]; then
        jq -e '.groups.demo.market_groups_accepted == true' "$EVIDENCE" >/dev/null || { fail 'demo requires market-group acceptance before any run'; return 1; }
    fi
    check_order "$group" || return
    [[ $(group_phase "$group") == captured || $(group_phase "$group") == rolled-back ]] || { fail 'group is not ready for a run'; return 1; }
    quiesce_group "$group" || return
    if [[ $(group_phase "$group") == rolled-back ]]; then refence_launchers "$group" || return; fi
    verify_direct_fences "$group" || return
    capture_launcher_permissions "$group" || return
    if [[ $(group_phase "$group") == rolled-back ]]; then publish_native_group "$group" || return; fi
    set_phase "$group" running || return
    : > "$STATE_DIR/$group/runs.tsv" || return
    while IFS= read -r unit; do
        [[ -n $unit ]] || continue
        verify_native_unit "$unit" || return
        before=$(service_value "$unit" InvocationID)
        service_ctl start "$unit" || return
        after=$(service_value "$unit" InvocationID)
        result=$(service_value "$unit" Result)
        status=$(service_value "$unit" ExecMainStatus)
        [[ $after =~ ^[0-9a-f]{32}$ && $after != 00000000000000000000000000000000 && $after != "$before" && $result == success && $status == 0 ]] || { fail "no new successful invocation: $unit"; return 1; }
        [[ $(service_value "$unit" ConditionResult) == yes && $(service_value "$unit" AssertResult) == yes ]] || { fail 'unit was skipped'; return 1; }
        printf '%s\t%s\n' "$unit" "$after" >> "$STATE_DIR/$group/runs.tsv"
    done < <(group_units "$group")
    verify_ready || return
    set_phase "$group" awaiting-acceptance || return
}
accept_group() {
    local group=$1 unit invocation
    [[ $(group_phase "$group") == awaiting-acceptance ]] || { fail 'group has no completed run'; return 1; }
    verify_direct_fences "$group" || return
    jq -e --arg group "$group" '.groups[$group].no_duplicate_effect == true and (if $group == "demo" then .groups[$group].market_groups_accepted == true else true end)' "$EVIDENCE" >/dev/null || { fail 'group acceptance missing'; return 1; }
    while IFS=$'\t' read -r unit invocation; do
        jq -e --arg unit "$unit" --arg invocation "$invocation" '.runs[$unit] == $invocation' "$EVIDENCE" >/dev/null || { fail 'acceptance does not bind observed invocation'; return 1; }
    done < "$STATE_DIR/$group/runs.tsv"
    verify_ready || return
    cp -- "$EVIDENCE" "$STATE_DIR/$group/acceptance.json" || return
    set_phase "$group" accepted || return
    while IFS= read -r unit; do [[ -z $unit ]] || service_ctl start "$unit"; done < "$STATE_DIR/$group/active-timers"
}
rollback_group() {
    local group=$1 file unit
    [[ -d $STATE_DIR/$group/previous ]] || { fail 'saved group is absent'; return 1; }
    local candidate seen=0 phase
    for candidate in "${GROUPS_ORDER[@]}"; do
        if [[ $candidate == "$group" ]]; then seen=1; continue; fi
        if (( seen )); then
            phase=$(group_phase "$candidate")
            case $phase in running|awaiting-acceptance|accepted|rolling-back) fail 'roll back dependent later groups first'; return 1 ;; esac
        fi
    done
    verify_snapshot "$group" || return
    quiesce_group "$group" || return
    verify_direct_fences "$group" || return
    if [[ $group == local-monitor || $group == external-recovery ]]; then
        release_alert_ownership || { fail 'native alert work blocks rollback; retained commands remain fenced'; return 1; }
    fi
    set_phase "$group" rolling-back || return
    for file in "$STATE_DIR/$group/previous/"*; do
        [[ -f $file ]] || continue
        cp -p -- "$file" "$UNIT_DIR/${file##*/}.w2-pending" || return
        mv -- "$UNIT_DIR/${file##*/}.w2-pending" "$UNIT_DIR/${file##*/}" || return
    done
    if [[ -f $STATE_DIR/$group/absent ]]; then
        while IFS= read -r unit; do rm -f -- "$UNIT_DIR/$unit"; done < "$STATE_DIR/$group/absent"
    fi
    local relative
    for file in "$STATE_DIR/$group/dropins/"*/*.conf; do
        [[ -f $file ]] || continue
        relative=${file#"$STATE_DIR/$group/dropins/"}
        [[ ! -L $UNIT_DIR/${relative%/*} ]] || { fail 'redirected rollback drop-in directory'; return 1; }
        mkdir -p "$UNIT_DIR/${relative%/*}" || return
        cp -p "$file" "$UNIT_DIR/$relative.w2-pending" || return
        mv "$UNIT_DIR/$relative.w2-pending" "$UNIT_DIR/$relative" || return
    done
    service_ctl daemon-reload || return
    # Shared database claims and journals are never rewound. Native producers
    # are drained and saved units restored before old permissions return.
    jq -e --arg group "$group" '.groups[$group].rollback.native_launchers_disabled == true
      and .groups[$group].rollback.direct_schedulers_disabled == true
      and .groups[$group].rollback.permission_restore_accepted == true' "$EVIDENCE" >/dev/null || { fail 'rollback ownership transfer acceptance missing'; return 1; }
    restore_launcher_permissions "$group" || return
    verify_ready || return
    set_phase "$group" rolled-back || return
    while IFS= read -r unit; do [[ -z $unit ]] || service_ctl start "$unit"; done < "$STATE_DIR/$group/active-timers"
}
verify_release() {
    local -a source_git=(/usr/bin/git -C "$SOURCE_ROOT" -c "safe.directory=$SOURCE_ROOT" -c core.hooksPath=/dev/null)
    [[ $("${source_git[@]}" rev-parse --show-toplevel) == "$SOURCE_ROOT" && $("${source_git[@]}" rev-parse HEAD) == "$SOURCE_REF" ]] || { fail 'source identity differs'; return 1; }
    [[ -z $("${source_git[@]}" status --porcelain --untracked-files=all) && -z $("${source_git[@]}" replace -l) ]] || { fail 'source is not clean'; return 1; }
    if "${source_git[@]}" ls-files -v -- | awk '$1 ~ /^[a-zS]$/ { found=1 } END { exit found ? 0 : 1 }'; then fail 'hidden tracked-file index flags'; return 1; fi
    [[ ! -e $SOURCE_ROOT/.runtime-deploy/$ENVIRONMENT.state && ! -L $SOURCE_ROOT/.runtime-deploy/$ENVIRONMENT.state ]] || { fail 'deploy state is present'; return 1; }
    local actual
    actual=$(sha256sum "$SOURCE_ROOT/.runtime-rust/verdaxis-read")
    [[ ${actual%% *} == "$BINARY_SHA256" ]] || { fail 'release binary differs'; return 1; }
}
file_digest() {
    local digest
    if command -v sha256sum >/dev/null; then digest=$(sha256sum -- "$1"); else digest=$(shasum -a 256 "$1"); fi
    printf '%s' "${digest%% *}"
}
validate_timer_inventory() {
    local expected_names actual_names timer digest source
    jq -e --arg environment "$ENVIRONMENT" --arg sha "$SOURCE_REF" '
      keys == ["environment","schema_version","source_ref","timers"] and .schema_version == 1
      and .environment == $environment and .source_ref == $sha
      and (.timers | type == "array")
      and ([.timers[].name] | length == (unique | length))
      and all(.timers[]; keys == ["disposition","name","sha256"] and .disposition == "active"
          and (.name | type == "string") and (.sha256 | type == "string" and test("^[a-f0-9]{64}$")))' "$INVENTORY" >/dev/null || { fail 'invalid source-bound timer inventory'; return 1; }
    expected_names=$(printf '%s\n' "${JOB_TIMERS[@]}" | sort)
    actual_names=$(jq -r '.timers[].name' "$INVENTORY" | sort)
    [[ $actual_names == "$expected_names" ]] || { fail 'active timer names differ from reviewed source set'; return 1; }
    for timer in "${JOB_TIMERS[@]}"; do
        source=$(unit_source "$timer")
        digest=$(jq -r --arg name "$timer" '.timers[] | select(.name == $name) | .sha256' "$INVENTORY")
        [[ -f $source && ! -L $UNIT_DIR/$timer ]] || { fail 'timer source unavailable or installed path redirected'; return 1; }
        [[ $(file_digest "$source") == "$digest" ]] || { fail 'timer digest does not bind reviewed source'; return 1; }
        local candidate saved='' was_absent=0
        for candidate in "${GROUPS_ORDER[@]}"; do
            if [[ $(group_phase "$candidate") == rolled-back && -f $STATE_DIR/$candidate/previous/$timer ]]; then
                verify_snapshot "$candidate" || return
                saved=$STATE_DIR/$candidate/previous/$timer
            elif [[ $(group_phase "$candidate") == rolled-back && -f $STATE_DIR/$candidate/absent ]] && grep -Fxq "$timer" "$STATE_DIR/$candidate/absent"; then
                verify_snapshot "$candidate" || return
                was_absent=1
            fi
        done
        if (( was_absent )); then
            [[ ! -e $UNIT_DIR/$timer ]] || { fail 'previously absent timer reappeared'; return 1; }
        elif [[ -n $saved ]]; then
            cmp -s "$saved" "$UNIT_DIR/$timer" || { fail 'rolled-back timer differs from saved bytes'; return 1; }
        else
            [[ $(file_digest "$UNIT_DIR/$timer") == "$digest" ]] || { fail 'timer digest does not bind installed bytes'; return 1; }
        fi
    done
}
validate_evidence() {
    [[ -f $EVIDENCE && ! -L $EVIDENCE && $(stat -c '%u:%a' "$EVIDENCE") == 0:600 ]] || { fail 'evidence must be root-owned regular 0600'; return 1; }
    jq -e --arg environment "$ENVIRONMENT" --arg sha "$SOURCE_REF" --arg digest "$BINARY_SHA256" --argjson now "$(date +%s)" '
      .schema_version == 1 and .environment == $environment and .source_ref == $sha and .binary_sha256 == $digest
      and (.recorded_at | type == "number") and .recorded_at <= $now and .recorded_at >= ($now - 900)
      and (.inventory_sha256 | type == "string" and test("^[a-f0-9]{64}$"))
      and (.groups | type == "object") and (.runs | type == "object")' "$EVIDENCE" >/dev/null || { fail 'evidence identity, inventory or freshness differs'; return 1; }
}
main() {
    local action='' group='' arg candidate expected
    ENVIRONMENT=''; SOURCE_REF=''; BINARY_SHA256=''; EVIDENCE=''; INVENTORY=''; HOST_SCOPE=environment
    while (( $# )); do
        [[ $# -ge 2 ]] || usage
        arg=$1; candidate=$2; shift 2
        case $arg in
            --action) [[ -z $action ]] || usage; action=$candidate ;;
            --group) [[ -z $group ]] || usage; group=$candidate ;;
            --environment) [[ -z $ENVIRONMENT ]] || usage; ENVIRONMENT=$candidate ;;
            --source-ref) [[ -z $SOURCE_REF ]] || usage; SOURCE_REF=$candidate ;;
            --binary-sha256) [[ -z $BINARY_SHA256 ]] || usage; BINARY_SHA256=$candidate ;;
            --evidence) [[ -z $EVIDENCE ]] || usage; EVIDENCE=$candidate ;;
            --timer-inventory) [[ -z $INVENTORY ]] || usage; INVENTORY=$candidate ;;
            --host-scope) [[ $candidate =~ ^(environment|host)$ ]] || usage; HOST_SCOPE=$candidate ;;
            *) usage ;;
        esac
    done
    [[ $ENVIRONMENT =~ ^(production|staging)$ && $SOURCE_REF =~ ^[a-f0-9]{40}$ && $BINARY_SHA256 =~ ^[a-f0-9]{64}$ ]] || usage
    case $action in install|advance|accept|rollback|status) ;; *) usage ;; esac
    case $group in ''|news|auth-prune|reminders|onboarding|demo|local-monitor|external-recovery) ;; *) usage ;; esac
    export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin LC_ALL=C
    for arg in "${!GIT_@}"; do unset "$arg"; done
    export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_NO_REPLACE_OBJECTS=1
    SOURCE_ROOT=/home/verdaxis-prod/verdaxis/prod/be
    [[ $ENVIRONMENT == production ]] || SOURCE_ROOT=/home/verdaxis-prod/verdaxis/staging/be
    [[ $(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P) == "$SOURCE_ROOT" ]] || { fail 'use the fixed environment checkout'; return 1; }
    (( EUID == 0 )) || { fail 'run with sudo -n from the fixed checkout'; return 1; }
    STATE_DIR=/var/lib/verdaxis/w2/$ENVIRONMENT
    UNIT_DIR=/etc/systemd/system
    [[ ! -L /var/lib/verdaxis && ! -L /var/lib/verdaxis/w2 && ! -L $STATE_DIR ]] || { fail 'redirected state path'; return 1; }
    install -d -m 0700 -o root -g root "$STATE_DIR"
    exec 9>"$STATE_DIR/lock"
    /usr/bin/flock -n 9 || { fail 'handover is busy'; return 1; }
    local lock_path
    for lock_path in "$SOURCE_ROOT/.runtime-deploy/$ENVIRONMENT.lock" "$SOURCE_ROOT/.runtime-deploy/$ENVIRONMENT.restart.lock"; do
        [[ -f $lock_path && ! -L $lock_path ]] || { fail 'canonical deploy locks must already exist'; return 1; }
    done
    exec 8>>"$SOURCE_ROOT/.runtime-deploy/$ENVIRONMENT.lock"
    /usr/bin/flock -n 8 || { fail 'deployment is busy'; return 1; }
    exec 7>>"$SOURCE_ROOT/.runtime-deploy/$ENVIRONMENT.restart.lock"
    /usr/bin/flock -n 7 || { fail 'restart window is busy'; return 1; }
    lock_path="/run/lock/verdaxis-systemd-units-${ENVIRONMENT}.lock"
    [[ ! -L $lock_path ]] || { fail 'redirected installer lock'; return 1; }
    exec 6>>"$lock_path"
    /usr/bin/flock -n 6 || { fail 'unit installer is busy'; return 1; }
    export VERDAXIS_SYSTEMD_INSTALL_LOCK_FD=6
    verify_release || return
    load_groups
    if [[ $action == status ]]; then
        expected=7; [[ $ENVIRONMENT == production ]] || expected=5
        printf 'active_job_timers=%s requested_job_timers=%s\n' "${#JOB_TIMERS[@]}" "$expected"
        printf '%s\n' "${JOB_TIMERS[@]}"
        printf 'window=NO_GO timer_inventory_scope_unresolved; recovery_retirement_requires_installed_acceptance\n'
        for candidate in "${GROUPS_ORDER[@]}"; do printf '%s=%s\n' "$candidate" "$(group_phase "$candidate")"; done
        return 0
    fi
    [[ -n $EVIDENCE ]] || usage
    validate_evidence || return
    if [[ $HOST_SCOPE == host ]]; then
        jq -e --arg host "$(hostname)" '.host_ownership.host == $host and .host_ownership.accepted == true and (.host_ownership.inventory_sha256 | test("^[a-f0-9]{64}$"))' "$EVIDENCE" >/dev/null || { fail 'host ownership is not bound to this host'; return 1; }
    fi
    if [[ $action != install ]]; then
        [[ -n $INVENTORY && -f $INVENTORY && ! -L $INVENTORY ]] || { fail 'source-bound timer inventory is required'; return 1; }
        [[ $(file_digest "$INVENTORY") == $(jq -r '.inventory_sha256' "$EVIDENCE") ]] || { fail 'timer inventory is not bound to evidence'; return 1; }
        validate_timer_inventory || return
    fi
    if [[ -f $STATE_DIR/identity ]]; then
        [[ $(cat "$STATE_DIR/identity") == "$SOURCE_REF:$BINARY_SHA256:$HOST_SCOPE" ]] || { fail 'saved handover identity differs'; return 1; }
    else
        [[ $action == install ]] || { fail 'install must run first'; return 1; }
        printf '%s:%s:%s\n' "$SOURCE_REF" "$BINARY_SHA256" "$HOST_SCOPE" > "$STATE_DIR/identity" || return
    fi
    if [[ $action == install ]]; then
        [[ -z $group ]] || usage
        # Capture every group and stop/drain before any installer changes units
        # or shared identity/ACL policy. External recovery is never started.
        [[ ! -f $STATE_DIR/install-phase || $(cat "$STATE_DIR/install-phase") != installed ]] || { fail 'installation already completed'; return 1; }
        for candidate in "${GROUPS_ORDER[@]}"; do
            if [[ $HOST_SCOPE == environment && ( $candidate == local-monitor || $candidate == external-recovery ) ]]; then continue; fi
            if [[ $(group_phase "$candidate") == absent ]]; then snapshot_group "$candidate" || return; fi
            [[ $(group_phase "$candidate") == captured ]] || { fail 'incomplete snapshot needs operator review'; return 1; }
            verify_snapshot "$candidate" || return
        done
        for candidate in "${GROUPS_ORDER[@]}"; do
            if [[ $HOST_SCOPE == environment && ( $candidate == local-monitor || $candidate == external-recovery ) ]]; then continue; fi
            quiesce_group "$candidate" || return
            verify_direct_fences "$candidate" || return
            capture_launcher_permissions "$candidate" || return
        done
        printf 'installing\n' > "$STATE_DIR/install-phase" || return
        /usr/bin/env VERDAXIS_SYSTEMD_INSTALL_LOCK_FD=6 "$SOURCE_ROOT/scripts/install_systemd_units.sh" --dry-run --environment "$ENVIRONMENT" --source-ref "$SOURCE_REF" --binary-sha256 "$BINARY_SHA256" --host-scope "$HOST_SCOPE" || return
        /usr/bin/env VERDAXIS_SYSTEMD_INSTALL_LOCK_FD=6 "$SOURCE_ROOT/scripts/install_systemd_units.sh" --apply --environment "$ENVIRONMENT" --source-ref "$SOURCE_REF" --binary-sha256 "$BINARY_SHA256" --host-scope "$HOST_SCOPE" || return
        printf 'installed\n' > "$STATE_DIR/install-phase" || return
        return
    fi
    [[ -n $group ]] || usage
    [[ -f $STATE_DIR/install-phase && $(cat "$STATE_DIR/install-phase") == installed ]] || { fail 'installation has not completed'; return 1; }
    case $action in advance) run_group "$group" ;; accept) accept_group "$group" ;; rollback) rollback_group "$group" ;; esac
}
if [[ ${BASH_SOURCE[0]} == "$0" ]]; then main "$@"; fi
