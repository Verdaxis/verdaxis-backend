#!/usr/bin/env bash
# Portable shell fixture. No installed paths or service manager are used.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
[[ -f $root/scripts/rust_w2_handover.sh ]] || { printf 'missing shell handover helper\n' >&2; exit 1; }
# Source exposes fixed operations; only this fixture replaces the service adapter.
source "$root/scripts/rust_w2_handover.sh"
stage=$(mktemp -d "${TMPDIR:-/tmp}/w2.XXXXXXXX")
trap 'rm -rf "$stage"' EXIT
export SOURCE_REF=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
# The selectable retry matrix uses real publication operations in a child shell.
# Only service state and privileged ownership/mode 000 are adapted for macOS.
rollback_interruption_contract() (
    local point=$1 saved_timer=$2 timer=verdaxis-news-refresh-staging.timer
    local service=verdaxis-news-refresh-staging.service file status
    local direction=${5:-rollback} retry_action=${6:-rollback} restoring_permissions=0
    local retry_stage=$stage/$saved_timer-$point
    mkdir -p "$retry_stage"
    STATE_DIR=$retry_stage/retry-state
    UNIT_DIR=$retry_stage/retry-units
    SOURCE_ROOT=$root
    ENVIRONMENT=staging
    EVIDENCE=$retry_stage/retry-evidence.json
    INVENTORY=$retry_stage/retry-inventory.json
    load_groups
    mkdir -p "$STATE_DIR" "$UNIT_DIR"
    printf 'old service\n' > "$UNIT_DIR/$service"
    if [[ $saved_timer == present ]]; then printf 'old timer\n' > "$UNIT_DIR/$timer"; fi
    mkdir -p "$UNIT_DIR/$service.d"
    printf '[Service]\nTimeoutStartSec=90s\n' > "$UNIT_DIR/$service.d/10-window.conf"
    printf 'saved launcher\n' > "$retry_stage/launcher"
    chmod 700 "$retry_stage/launcher"
    : > "$retry_stage/active"
    if [[ ${4:-active} == active ]]; then printf '%s\n' "$timer" > "$retry_stage/active"; fi
    service_value() {
        case $2 in
            ActiveState) if grep -Fxq "$1" "$retry_stage/active"; then printf active; else printf inactive; fi ;;
            InvocationID) printf '%032d' "$(cat "$retry_stage/invocation")" ;;
            Result) printf success ;;
            ExecMainStatus) printf 0 ;;
            ConditionResult|AssertResult) printf yes ;;
            Job) printf 0 ;;
            DropInPaths) printf '' ;;
            *) return 1 ;;
        esac
    }
    service_ctl() {
        case $1 in
            stop) grep -Fxv "$2" "$retry_stage/active" > "$retry_stage/next-active" || :; command mv "$retry_stage/next-active" "$retry_stage/active" ;;
            start)
                if [[ $2 == *.service ]]; then
                    printf '%s\n' "$(( $(cat "$retry_stage/invocation") + 1 ))" > "$retry_stage/invocation"
                else
                    printf '%s\n' "$2" >> "$retry_stage/active"
                fi ;;
            daemon-reload) checkpoint daemon-reload ;;
            *) return 1 ;;
        esac
    }
    # Publication durability is covered by the main fixture's sync assertions.
    sync_path() {
        if [[ $1 == "$STATE_DIR/news" ]]; then checkpoint "phase-$(group_phase news)"; fi
    }
    verify_ready() { checkpoint readiness; }
    verify_native_unit() { cmp "$(unit_source "$1")" "$UNIT_DIR/$1"; }
    install() {
        # Keep real publication bytes without requiring root in the fixture.
        command install -m 0644 "${@: -2}" || return
        case ${@: -1} in
            "$UNIT_DIR/$service.w2-pending") checkpoint service-copy ;;
            "$UNIT_DIR/$timer.w2-pending") checkpoint timer-copy ;;
        esac
    }
    verify_direct_fences() {
        [[ $(cat "$retry_stage/launcher-owner") == 0:0 && $(cat "$retry_stage/launcher-mode") == 000 ]] || {
            fail 'launcher fence bytes, ownership or mode differ'; return 1;
        }
    }
    checkpoint() {
        [[ ${armed:-0} != 1 || $point != "$1" ]] || {
            printf '%s\n' "$1" > "$retry_stage/interrupted"
            exit 77
        }
    }
    # Preserve real copies and renames, including each pending-file boundary.
    cp() {
        command cp "$@" || return
        case ${@: -1} in
            "$UNIT_DIR/$service.w2-pending") checkpoint service-copy ;;
            "$UNIT_DIR/$timer.w2-pending") checkpoint timer-copy ;;
            "$UNIT_DIR/$service.d/10-window.conf.w2-pending") checkpoint dropin-copy ;;
        esac
    }
    mv() {
        command mv "$@" || return
        case ${@: -1} in
            "$UNIT_DIR/$service") checkpoint service-publish ;;
            "$UNIT_DIR/$timer") checkpoint timer-publish ;;
            "$UNIT_DIR/$service.d/10-window.conf") checkpoint dropin-publish ;;
        esac
    }
    rm() { command rm "$@" || return; [[ ${*: -1} != "$UNIT_DIR/$timer" ]] || checkpoint absent-remove; }
    mkdir() { command mkdir "$@" || return; [[ ${*: -1} != "$UNIT_DIR/$service.d" ]] || checkpoint dropin-directory; }
    jq() {
        command jq "$@" || return
        if [[ $* == *rollback.native_launchers_disabled* ]]; then checkpoint ownership-acceptance; fi
    }
    chown() {
        [[ ${*: -1} == "$retry_stage/launcher" ]] || return 1
        printf '%s\n' "$1" > "$retry_stage/launcher-owner"
        [[ $1 == 0:0 ]] || checkpoint launcher-owner
    }
    chmod() {
        [[ $2 == "$retry_stage/launcher" ]] || return 1
        printf '%s\n' "$1" > "$retry_stage/launcher-mode"
        # A non-root fixture must still read the bytes to check their digest.
        if [[ $1 == 000 ]]; then command chmod 600 "$2"; else command chmod "$@"; checkpoint launcher-mode; fi
    }
    snapshot_group news
    printf '%s\t%s\t%s\t%s\t700\n' "$retry_stage/launcher" "$(file_digest "$retry_stage/launcher")" "$(id -u)" "$(id -g)" > "$STATE_DIR/news/launchers.tsv"
    file_digest "$STATE_DIR/news/launchers.tsv" > "$STATE_DIR/news/launchers.sha256"
    refence_launchers news
    command jq -n --arg sha "$SOURCE_REF" '{schema_version:1,environment:"staging",source_ref:$sha,timers:[]}' > "$INVENTORY"
    for file in "${JOB_TIMERS[@]}"; do
        command cp "$(unit_source "$file")" "$UNIT_DIR/$file"
        command jq --arg name "$file" --arg digest "$(file_digest "$UNIT_DIR/$file")" '.timers += [{name:$name,sha256:$digest,disposition:"active"}]' "$INVENTORY" > "$retry_stage/next"
        command mv "$retry_stage/next" "$INVENTORY"
    done
    printf 'new service\n' > "$UNIT_DIR/$service"
    printf 'new drop-in\n' > "$UNIT_DIR/$service.d/10-window.conf"
    printf '%s\n' '{"groups":{"news":{"rollback":{"native_launchers_disabled":true,"direct_schedulers_disabled":true,"permission_restore_accepted":true}}}}' > "$EVIDENCE"
    set_phase news accepted
    validate_timer_inventory
    printf '1\n' > "$retry_stage/invocation"
    if [[ $direction == advance ]]; then
        rollback_group news
        restoring_permissions=0
    fi
    status=0
    if [[ $direction == advance ]]; then
        (armed=1; run_group news) || status=$?
    else
        (armed=1; rollback_group news) || status=$?
    fi
    [[ $status == 77 && $(cat "$retry_stage/interrupted") == "$point" ]] || {
        printf 'interruption not reached: %s (exit %s)\n' "$point" "$status" >&2; exit 1;
    }
    # Match the non-install command's validation order on the retry.
    case ${3:-} in
        timer) printf 'unreviewed timer\n' > "$UNIT_DIR/$timer" ;;
        snapshot) printf 'changed snapshot\n' >> "$STATE_DIR/news/previous/$service" ;;
        launcher) printf 'changed launcher\n' >> "$retry_stage/launcher" ;;
        redirected) command rm -f "$UNIT_DIR/$timer"; ln -s "$(unit_source "$timer")" "$UNIT_DIR/$timer" ;;
    esac
    validate_timer_inventory
    if [[ $retry_action == advance ]]; then
        run_group news
        [[ $(group_phase news) == awaiting-acceptance ]]
        [[ $(cat "$retry_stage/invocation") == 2 ]]
        cmp "$(unit_source "$service")" "$UNIT_DIR/$service"
        cmp "$(unit_source "$timer")" "$UNIT_DIR/$timer"
        verify_direct_fences news
        [[ ! -s $retry_stage/active ]]
        printf 'PASS advance retry %s %s\n' "$saved_timer" "$point"
        exit
    fi
    rollback_group news
    [[ $(group_phase news) == rolled-back ]]
    cmp "$STATE_DIR/news/previous/$service" "$UNIT_DIR/$service"
    if [[ $saved_timer == present ]]; then
        cmp "$STATE_DIR/news/previous/$timer" "$UNIT_DIR/$timer"
    else
        [[ ! -e $UNIT_DIR/$timer ]]
    fi
    cmp "$STATE_DIR/news/dropins/$service.d/10-window.conf" "$UNIT_DIR/$service.d/10-window.conf"
    [[ $(cat "$retry_stage/launcher-owner") == "$(id -u):$(id -g)" && $(cat "$retry_stage/launcher-mode") == 700 && -x $retry_stage/launcher ]]
    cmp "$STATE_DIR/news/active-timers" "$retry_stage/active"
    printf 'PASS rollback retry %s %s\n' "$saved_timer" "$point"
)
if [[ ${1:-} == --advance-interruption ]]; then
    rollback_interruption_contract "$2" "$3" "${4:-}" "${5:-active}" advance "${6:-advance}"
    exit
fi
if [[ ${1:-} == --rollback-interruption ]]; then
    rollback_interruption_contract "$2" "$3" "${4:-}" "${5:-active}"
    exit
fi
for saved_timer in present absent; do
    points=(phase-rolling-back service-copy service-publish dropin-directory dropin-copy dropin-publish daemon-reload ownership-acceptance launcher-owner launcher-mode readiness phase-rolled-back)
    if [[ $saved_timer == present ]]; then points+=(timer-copy timer-publish); else points+=(absent-remove); fi
    for point in "${points[@]}"; do rollback_interruption_contract "$point" "$saved_timer"; done
done
STATE_DIR=$stage/state
UNIT_DIR=$stage/units
mkdir -p "$STATE_DIR" "$UNIT_DIR"
ENVIRONMENT=staging
export SOURCE_REF=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
export BINARY_SHA256=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
export ENVIRONMENT
LOG=$stage/log
: > "$LOG"
printf '1\n' > "$stage/run"
service_ctl() {
    printf '%s\n' "$*" >> "$LOG"
    if [[ $1 == start && $2 == *.service ]]; then
        printf '%s\n' "$(( $(cat "$stage/run") + 1 ))" > "$stage/run"
    fi
}
service_value() {
    case $2 in
        ActiveState) printf inactive ;;
        Job) printf 0 ;;
        InvocationID) printf '%032d' "$(cat "$stage/run")" ;;
        DropInPaths) printf '' ;;
        Result) printf success ;;
        ExecMainStatus) printf 0 ;;
        ConditionResult|AssertResult) printf yes ;;
        LoadState) printf loaded ;;
        *) return 1 ;;
    esac
}
verify_ready() { :; }
verify_direct_fences() { :; }
verify_native_unit() { :; }
verify_release() { :; }
capture_launcher_permissions() { :; }
EVIDENCE=$stage/evidence.json
printf '%s\n' '{"groups":{"news":{"rollback":{"native_launchers_disabled":true,"direct_schedulers_disabled":true,"permission_restore_accepted":true}}}}' > "$EVIDENCE"
load_groups
[[ ${#JOB_TIMERS[@]} == 4 ]]
[[ ${JOB_TIMERS[*]} != *onboarding* && ${JOB_TIMERS[*]} != *reminders* ]]
ENVIRONMENT=production
load_groups
[[ ${#JOB_TIMERS[@]} == 6 ]]
ENVIRONMENT=staging
load_groups
# Unaccepted work cannot advance. Successful acceptance is a separate operation.
printf 'old news bytes\n' > "$UNIT_DIR/verdaxis-news-refresh-staging.service"
snapshot_group news
[[ -f $STATE_DIR/news/previous/verdaxis-news-refresh-staging.service ]]
run_group news
[[ -f $STATE_DIR/news/runs.tsv ]]
[[ $(group_phase news) == awaiting-acceptance ]]
if (run_group auth-prune) 2>/dev/null; then exit 1; fi
printf 'new news bytes\n' > "$UNIT_DIR/verdaxis-news-refresh-staging.service"
rollback_group news
cmp "$STATE_DIR/news/previous/verdaxis-news-refresh-staging.service" "$UNIT_DIR/verdaxis-news-refresh-staging.service"
[[ $(group_phase news) == rolled-back ]]
verify_snapshot news
# Dynamic state-bearing old wakeups are included in the drain set.
service_instances() { printf 'verdaxis-monitor-alert@health-failure.service\n'; }
if ! drain_units local-monitor | grep -Fxq 'verdaxis-monitor-alert@health-failure.service'; then exit 1; fi
# A native alert release refusal leaves saved Python bytes untouched.
HOST_SCOPE=host
printf 'old health bytes\n' > "$UNIT_DIR/verdaxis-health.service"
snapshot_group local-monitor
printf 'native health bytes\n' > "$UNIT_DIR/verdaxis-health.service"
release_alert_ownership() { return 5; }
if (rollback_group local-monitor) 2>/dev/null; then exit 1; fi
grep -Fxq 'native health bytes' "$UNIT_DIR/verdaxis-health.service"
# External recovery does not own the health/backup journals. Its rollback must
# leave those journals native while local-monitor timers can still produce work.
(
    set_phase local-monitor accepted
    snapshot_group external-recovery
    set_phase external-recovery accepted
    service_value() {
        case $2 in
            ActiveState)
                case $1 in verdaxis-health.*|verdaxis-backup-verify.*) printf active ;; *) printf inactive ;; esac ;;
            Job) printf 0 ;;
            DropInPaths) printf '' ;;
            *) return 1 ;;
        esac
    }
    release_alert_ownership() { printf 'released health backup\n' >> "$LOG"; }
    jq '.groups["external-recovery"].rollback = {native_launchers_disabled:true,direct_schedulers_disabled:true,permission_restore_accepted:true}' "$EVIDENCE" > "$stage/external-evidence"
    # The redirected evidence applies to this rollback attempt only.
    # shellcheck disable=SC2030
    EVIDENCE=$stage/external-evidence
    # shellcheck disable=SC2031
    rollback_group external-recovery
    if grep -Fxq 'released health backup' "$LOG"; then
        printf 'external rollback released active local-monitor journals\n' >&2
        exit 1
    fi
    [[ $(group_phase local-monitor) == accepted ]]
    [[ $(service_value verdaxis-health.timer ActiveState) == active ]]
    [[ $(service_value verdaxis-backup-verify.timer ActiveState) == active ]]
)
rm -rf "$STATE_DIR/external-recovery"
# Retired recovery is never started; missing installed replacement proof refuses.
if (run_group external-recovery) 2>/dev/null; then exit 1; fi
if grep -q 'start verdaxis-recover' "$LOG"; then exit 1; fi
# Rollback restores only captured launcher permissions after native drain.
launcher=$stage/old-launcher
printf 'saved launcher\n' > "$launcher"
chmod 600 "$launcher"
printf '%s\t%s\t%s\t%s\t%s\n' "$launcher" "$(file_digest "$launcher")" "$(id -u)" "$(id -g)" 700 > "$STATE_DIR/news/launchers.tsv"
file_digest "$STATE_DIR/news/launchers.tsv" > "$STATE_DIR/news/launchers.sha256"
restore_launcher_permissions news
[[ -x $launcher ]]
# Inventoried root unit drop-ins are captured with their exact bytes.
mkdir -p "$UNIT_DIR/verdaxis-demo-activity-staging.service.d"
printf '[Service]\nTimeoutStartSec=90s\n' > "$UNIT_DIR/verdaxis-demo-activity-staging.service.d/10-window.conf"
cp "$UNIT_DIR/verdaxis-demo-activity-staging.service.d/10-window.conf" "$stage/dropin"
snapshot_dropins demo verdaxis-demo-activity-staging.service
cmp "$stage/dropin" "$STATE_DIR/demo/dropins/verdaxis-demo-activity-staging.service.d/10-window.conf"
rm -rf "$STATE_DIR/demo"
# Failed byte capture must never publish a captured phase.
cp() { return 1; }
printf 'old demo bytes\n' > "$UNIT_DIR/verdaxis-demo-activity-staging.service"
if (snapshot_group demo) 2>/dev/null; then exit 1; fi
[[ $(group_phase demo) == absent ]]
unset -f cp
# Snapshot validation detects changes before installation or rollback.
cp "$STATE_DIR/news/previous/verdaxis-news-refresh-staging.service" "$stage/saved-news"
printf 'changed saved bytes\n' >> "$STATE_DIR/news/previous/verdaxis-news-refresh-staging.service"
if (verify_snapshot news) 2>/dev/null; then exit 1; fi
cp "$stage/saved-news" "$STATE_DIR/news/previous/verdaxis-news-refresh-staging.service"
# Actual named timer inventory is source-bound, never a count-only approval.
set_phase news captured
SOURCE_ROOT=$root
INVENTORY=$stage/inventory.json
jq -n --arg environment "$ENVIRONMENT" --arg sha "$SOURCE_REF" '{schema_version:1,environment:$environment,source_ref:$sha,timers:[]}' > "$INVENTORY"
if (validate_timer_inventory) 2>/dev/null; then exit 1; fi
for timer in "${JOB_TIMERS[@]}"; do
    source=$(unit_source "$timer")
    cp "$source" "$UNIT_DIR/$timer"
    digest=$(shasum -a 256 "$source"); digest=${digest%% *}
    jq --arg name "$timer" --arg digest "$digest" '.timers += [{name:$name,sha256:$digest,disposition:"active"}]' "$INVENTORY" > "$stage/next"
    mv "$stage/next" "$INVENTORY"
done
validate_timer_inventory
# A skipped unit is not a successful run; a failed group does not advance.
set_phase news captured
service_value() {
    case $2 in
        ActiveState) printf inactive ;;
        Job) printf 0 ;;
        InvocationID) printf '%032d' "$(cat "$stage/run")" ;;
        Result) printf success ;;
        ExecMainStatus) printf 0 ;;
        ConditionResult) printf no ;;
        AssertResult) printf yes ;;
        DropInPaths) printf '' ;;
        *) return 1 ;;
    esac
}
if (run_group news) 2>/dev/null; then exit 1; fi
[[ $(group_phase news) == running ]]
# A failed timer restart is resumable without another job invocation.
STATE_DIR=$stage/resume
mkdir -p "$STATE_DIR/news"
printf 'awaiting-acceptance\n' > "$STATE_DIR/news/phase"
printf 'verdaxis-news-refresh-staging.timer\n' > "$STATE_DIR/news/active-timers"
printf 'verdaxis-news-refresh-staging.service\tnew-run\n' > "$STATE_DIR/news/runs.tsv"
# Top-level rewrite of the shared evidence file for the news acceptance block.
# shellcheck disable=SC2031
printf '%s\n' '{"groups":{"news":{"no_duplicate_effect":true}},"runs":{"verdaxis-news-refresh-staging.service":"new-run"}}' > "$EVIDENCE"
service_ctl() { printf '%s\n' "$*" >> "$LOG"; return 1; }
if (accept_group news) 2>/dev/null; then exit 1; fi
[[ $(group_phase news) == accepted ]]
service_ctl() { printf '%s\n' "$*" >> "$LOG"; }
accept_group news
[[ $(group_phase news) == accepted ]]
# A completed rollback retries timer restoration without releasing alerts or
# changing permissions again. The saved snapshot is still checked.
verify_snapshot() { printf 'checked snapshot\n' >> "$LOG"; }
mkdir -p "$STATE_DIR/news/previous"
set_phase news rolled-back
service_ctl() { printf '%s\n' "$*" >> "$LOG"; return 1; }
if (rollback_group news) 2>/dev/null; then exit 1; fi
[[ $(group_phase news) == rolled-back ]]
service_ctl() { printf '%s\n' "$*" >> "$LOG"; }
rollback_group news
[[ $(group_phase news) == rolled-back ]]
# Phase publication must sync the new file and its containing directory.
sync_path() { printf '%s\n' "$1" >> "$stage/sync-log"; }
set_phase news accepted
grep -Fxq "$STATE_DIR/news/phase.tmp" "$stage/sync-log"
grep -Fxq "$STATE_DIR/news" "$stage/sync-log"
printf 'PASS W2 shell state, order, rollback and pending-alert fence\n'
