#!/usr/bin/env bash
# Install only the external monitor binary and its two services. Never read env values.
set -euo pipefail
umask 077

fail() { printf '%s\n' "$*" >&2; exit 1; }
[[ $(id -u) == 0 ]] || fail 'Run as root.'
source_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
binary=/usr/local/libexec/verdaxis/verdaxis-read-monitor
units=/etc/systemd/system
records=/var/lib/verdaxis-monitor-install
names=(verdaxis-monitor.service verdaxis-monitor-verify.service)
paths=("$binary" "$units/${names[0]}" "$units/${names[1]}")
for destination in "$records" "${paths[@]}"; do
    parent=$destination
    while [[ $parent != / ]]; do
        [[ ! -L $parent ]] || fail 'Refusing a symlink destination or parent.'
        parent=$(dirname -- "$parent")
    done
done
install -d -o root -g root -m 0700 -- "$records"
exec 9>"$records/lock"
flock -n 9 || fail 'Another monitor install is active.'

# The operator owns timer activation. Refuse to race a scheduled or manual run.
for unit in verdaxis-monitor.timer "${names[@]}"; do
    state=$(systemctl show "$unit" --property=ActiveState --value)
    [[ $state == inactive || $state == failed ]] || fail "Stop $unit first."
done
[[ $(systemctl is-enabled verdaxis-recover.service 2>/dev/null || true) == masked ]] ||
    fail 'Recovery must remain masked.'
for path in "${paths[@]}"; do
    [[ ! -L $path && ( ! -e $path || -f $path ) ]] || fail 'Refusing a non-regular destination.'
done

replace() {
    local source=$1 destination=$2 mode=$3 temporary
    mkdir -p -- "$(dirname -- "$destination")"
    temporary=$(mktemp "${destination}.tmp.XXXXXXXX")
    if ! install -o root -g root -m "$mode" -- "$source" "$temporary" ||
       ! mv -fT -- "$temporary" "$destination"; then
        rm -f -- "$temporary"
        return 1
    fi
}

case ${1:-} in
    install)
        [[ $# == 3 && $3 =~ ^[a-f0-9]{64}$ ]] || fail 'Usage: install.sh install BINARY SHA256'
        [[ -f $2 && ! -L $2 ]] || fail 'A regular CI release binary is required.'
        # Copy before hashing, so the checked bytes are the installed bytes.
        staged=$(mktemp "$records/binary.XXXXXXXX")
        trap 'rm -f -- "$staged"' EXIT
        cp -- "$2" "$staged"
        printf '%s  %s\n' "$3" "$staged" | sha256sum --check --status || fail 'SHA256 mismatch.'
        for name in "${names[@]}"; do
            [[ -f $source_dir/systemd/$name ]] || fail 'Missing service source.'
        done
        if [[ -f $binary ]] && cmp -s -- "$staged" "$binary" &&
           cmp -s -- "$source_dir/systemd/${names[0]}" "${paths[1]}" &&
           cmp -s -- "$source_dir/systemd/${names[1]}" "${paths[2]}" &&
           [[ $(stat -c '%u:%g:%a' "$binary") == 0:0:755 ]] &&
           [[ $(stat -c '%u:%g:%a' "${paths[1]}") == 0:0:644 ]] &&
           [[ $(stat -c '%u:%g:%a' "${paths[2]}") == 0:0:644 ]]; then
            systemctl daemon-reload
            printf '%s\n' 'Already installed; no files replaced.'
            exit 0
        fi
        stamp=$(date -u +%Y%m%dT%H%M%S.%NZ)
        record=$records/$stamp
        mkdir -- "$record"
        for index in "${!paths[@]}"; do
            path=${paths[$index]}
            if [[ -e $path ]]; then
                cp -a -- "$path" "$path.bak.$stamp"
                touch "$record/$index.present"
            else
                touch "$record/$index.absent"
            fi
        done
        # Print the rollback key before the first replacement, including on failure.
        printf 'Rollback key: %s\n' "$stamp"
        replace "$staged" "$binary" 0755
        for index in 0 1; do
            replace "$source_dir/systemd/${names[$index]}" "${paths[$((index + 1))]}" 0644
        done
        systemctl daemon-reload
        ;;
    rollback)
        [[ $# == 2 && $2 =~ ^[0-9]{8}T[0-9]{6}\.[0-9]{9}Z$ ]] || fail 'Usage: install.sh rollback TIMESTAMP'
        stamp=$2
        record=$records/$stamp
        [[ -d $record ]] || fail 'Unknown rollback key.'
        # Validate the complete backup set before restoring any file.
        for index in "${!paths[@]}"; do
            path=${paths[$index]}
            if [[ -f $record/$index.present ]]; then
                [[ -f $path.bak.$stamp && ! -L $path.bak.$stamp ]] || fail 'Missing backup.'
            else
                [[ -f $record/$index.absent ]] || fail 'Incomplete rollback record.'
            fi
        done
        for index in "${!paths[@]}"; do
            path=${paths[$index]}
            if [[ -f $record/$index.present ]]; then
                temporary=$(mktemp "$path.tmp.XXXXXXXX")
                cp -a -- "$path.bak.$stamp" "$temporary"
                mv -fT -- "$temporary" "$path"
            else
                rm -f -- "$path"
            fi
        done
        systemctl daemon-reload
        printf '%s\n' 'Rollback restored; timer remains stopped.'
        ;;
    *) fail 'Usage: install.sh {install BINARY SHA256|rollback TIMESTAMP}' ;;
esac
