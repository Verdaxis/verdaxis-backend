#!/usr/bin/env bash
# Stable read-only projection of backup and staging-pull scheduler bytes.
set -euo pipefail
export LC_ALL=C
paths=()
while IFS= read -r -d '' path; do
    [[ -f $path && ! -L $path ]] || continue
    [[ $path != *backup-verify* ]] || continue
    if [[ $path == *rehearsal-preserved* || $path == */verdaxis-rehearsal-backup || $path == */verdaxis-rehearsal-staging-pull || $path == */verdaxis-local-db-backup.timer || $path == */verdaxis-local-db-backup.service ]]; then
        paths+=("$path")
    fi
done < <(find /etc/cron.d /etc/systemd/system /var/spool/cron/crontabs /usr/local/sbin -type f -print0 2>/dev/null)
(( ${#paths[@]} > 0 )) || { printf 'No unchanged backup/staging-pull scheduler source found\n' >&2; exit 1; }
printf '%s\0' "${paths[@]}" | sort -z | while IFS= read -r -d '' path; do
    digest=$(sha256sum -- "$path"); digest=${digest%% *}
    printf '%s\t%s\t%s\n' "$path" "$(stat -c '%u:%g:%a' "$path")" "$digest"
done | jq -Rn '[inputs | split("\t") | {path:.[0],metadata:.[1],sha256:.[2]}]'
