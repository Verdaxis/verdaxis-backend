#!/usr/bin/env bash
# Final disposable-only deletion. Never remove the system interpreter used by
# unchanged backup automation. Remove only captured application package paths.
rehearsal_w5() {
    local checkout=$REHEARSAL_CHECKOUT path unit suffix='' before after command
    [[ $REHEARSAL_ENVIRONMENT == production ]] || suffix=-staging
    [[ -f /run/verdaxis-disposable-rehearsal && ! -L /run/verdaxis-disposable-rehearsal &&
       $(stat -c '%u:%a' /run/verdaxis-disposable-rehearsal) == 0:644 ]] || return 1
    grep -Fxq "$GITHUB_RUN_ID:$checkout" /run/verdaxis-disposable-rehearsal || return 1
    before=$(rehearsal_hash "$PRIVATE/schedules-before.json")
    # W5 follows completed rollback and forward acceptance. Retire only the
    # inventoried old companion/retained backend units; never start them again.
    local companion_suffix='-staging'
    [[ $REHEARSAL_ENVIRONMENT != staging ]] || companion_suffix=''
    jq -n --arg environment "$REHEARSAL_ENVIRONMENT" --arg url "http://127.0.0.1:$([[ $REHEARSAL_ENVIRONMENT == production ]] && printf 8010 || printf 8011)/health/ready" \
        '{version:1,endpoints:[{name:$environment,url:$url,environment:$environment}]}' > /etc/verdaxis-monitor/health-endpoints.json
    chmod 0644 /etc/verdaxis-monitor/health-endpoints.json
    systemctl stop "verdaxis-rust$companion_suffix.service"
    rm -f -- "/etc/systemd/system/verdaxis-rust$companion_suffix.service"
    for unit in verdaxis-backend.service verdaxis-backend-staging.service verdaxis-recover.service verdaxis-diagnose.service verdaxis-codex-diagnose.service verdaxis-order-expiry-reminders-staging.service; do
        systemctl stop "$unit" || return 1
        rm -f -- "/etc/systemd/system/$unit"
        systemctl mask "$unit"
    done
    # The other environment exists only to exercise the actual old two-host
    # monitor. Retire its exact old app job ABI before deleting its package.
    for unit in "verdaxis-news-refresh$companion_suffix" "verdaxis-auth-maintenance$companion_suffix" "verdaxis-product-analytics-prune$companion_suffix" "verdaxis-demo-activity$companion_suffix"; do
        systemctl stop "$unit.timer" "$unit.service"
        rm -f -- "/etc/systemd/system/$unit.timer" "/etc/systemd/system/$unit.service"
    done
    systemctl daemon-reload
    # Native units must be the installed exact source bytes before deletion.
    while IFS= read -r unit; do
        [[ -n $unit ]] || continue
        if systemctl cat "$unit" | grep -Ei '^(ExecStart|ExecStartPre|ExecStartPost)=.*(python|venv)' >/dev/null; then
            printf 'FAIL W5: installed unit retains application interpreter %s\n' "$unit" >&2
            return 1
        fi
    done < <(systemctl list-unit-files --no-legend 'verdaxis-*.service' | cut -d ' ' -f 1)
    for unit in "verdaxis-backend$suffix.service" verdaxis-recover.service verdaxis-diagnose.service verdaxis-codex-diagnose.service verdaxis-order-expiry-reminders-staging.service; do
        if [[ $(systemctl show --value --property=ActiveState "$unit") == active ]]; then
            printf 'FAIL W5: retained service active %s\n' "$unit" >&2
            return 1
        fi
    done
    : > "$PUBLIC/w5.txt"
    # These are exclusively disposable checkouts created by this run, never
    # runtime data, backups, source checkout inputs, /usr, or global packages.
    local companion_checkout=/home/verdaxis-prod/verdaxis/staging/be
    [[ $REHEARSAL_ENVIRONMENT != staging ]] || companion_checkout=/home/verdaxis-prod/verdaxis/prod/be
    for path in "$checkout/app" "$checkout/venv" "$companion_checkout/app" "$companion_checkout/venv"; do
        [[ ( $path == "$checkout/app" || $path == "$checkout/venv" || $path == "$companion_checkout/app" || $path == "$companion_checkout/venv" ) && -d $path && ! -L $path ]] || return 1
        printf 'deleted_path=%s tree_inventory_sha256=' "$path" >> "$PUBLIC/w5.txt"
        find "$path" -type f -print0 | sort -z | xargs -0 -r sha256sum | sha256sum | cut -d ' ' -f 1 >> "$PUBLIC/w5.txt"
        rm -rf --one-file-system -- "$path"
        [[ ! -e $path && ! -L $path ]] || return 1
    done
    # Delete exact retained direct-launcher payloads captured before W2. No
    # glob can escape the two documented disposable launcher namespaces.
    while IFS= read -r path; do
        case $path in
            /usr/local/libexec/verdaxis-monitor/*.py|/usr/local/sbin/verdaxis-monitor|/usr/local/sbin/verdaxis-recover|/usr/local/sbin/verdaxis-autodiag)
                [[ ! -L $path ]] || return 1
                rm -f -- "$path" ;;
            "$checkout/"*.py) [[ ! -L $path ]] || return 1; rm -f -- "$path" ;;
            *) printf 'FAIL W5: launcher deletion path is not disposable %s\n' "$path" >&2; return 1 ;;
        esac
    done < <(jq -r 'keys[]' "$PRIVATE/w2/old-launchers.json")
    # Preserve inert migration history. Remove only its executable entrypoints.
    for path in "$checkout/alembic/env.py" "$checkout/alembic/script.py.mako" "$checkout/alembic.ini" "$companion_checkout/alembic/env.py" "$companion_checkout/alembic/script.py.mako" "$companion_checkout/alembic.ini"; do
        [[ ! -L $path && ( $path == "$checkout/"* || $path == "$companion_checkout/"* ) ]] || return 1
        rm -f -- "$path"
    done
    systemctl restart "verdaxis-rust$suffix.service"
    systemctl start verdaxis-health.service verdaxis-backup-verify.service
    [[ $(systemctl show --value --property=Result verdaxis-health.service) == success &&
       $(systemctl show --value --property=Result verdaxis-backup-verify.service) == success ]] || return 1
    # Observe after the interpreter-free restart, not while a Python verifier
    # is itself running. Use proc names only; command arguments may be private.
    : > "$PUBLIC/processes.txt"
    for path in /proc/[0-9]*/comm; do
        [[ -r $path ]] || continue
        command=$(< "$path")
        case $command in
            python*|uvicorn*|gunicorn*) printf 'FAIL remaining_process=%s\n' "$command" >> "$PUBLIC/processes.txt"; return 1 ;;
        esac
    done
    printf 'No Python application or interpreter process remains.\n' > "$PUBLIC/processes.txt"
    # Schedules are captured by the native shell reader, not regenerated from
    # desired source. Byte differences remain a hard acceptance failure.
    bash "$ROOT/deploy/tests/rehearsal/schedules.sh" > "$PRIVATE/schedules-after.json"
    after=$(rehearsal_hash "$PRIVATE/schedules-after.json")
    [[ $before == "$after" ]] || return 1
    /usr/local/sbin/verdaxis-rehearsal-backup
    /usr/local/sbin/verdaxis-rehearsal-staging-pull
    printf 'before_sha256=%s\nafter_sha256=%s\nunchanged=true\n' "$before" "$after" > "$PUBLIC/schedules.txt"
}
