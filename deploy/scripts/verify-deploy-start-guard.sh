#!/usr/bin/env bash
# Deployment start guard: allow a backend start only outside a deploy or inside
# the exact restart window.
#
# The unit runs this file before any Rust artifact exists, so it must never call
# the binary or an interpreter. It uses bash, stat, dd and util-linux flock only.
# Every input it cannot read, or does not expect, denies the start. Owner
# controlled directories and files remove other identities from the path; the
# guard is not a defence against the deploy identity itself.
set -euo pipefail
export LC_ALL=C
umask 077

deny() {
    printf 'Backend start denied by deployment guard: %s\n' "$1" >&2
    exit 1
}

if (( $# != 4 )) || [[ $1 != --environment || $3 != --expected-release-sha ]]; then
    deny 'invalid arguments'
fi
environment=$2
expected_release_sha=$4
case $environment in
    production | staging) ;;
    *) deny 'unsupported environment' ;;
esac
if [[ ! $expected_release_sha =~ ^[0-9a-f]{40}$ ]]; then
    deny 'invalid release sha'
fi

# GNU coreutils and BSD stat differ only in their format flag and names.
if stat -c %d / >/dev/null 2>&1; then
    stat_style=gnu
else
    stat_style=bsd
fi
if [[ -d /proc/self/fd ]]; then
    fd_root=/proc/self/fd
else
    fd_root=/dev/fd
fi

# Identity of a path or of an open descriptor: device, inode, owner, mode, links.
# The descriptor form must follow its link.
stat_identity() {
    if [[ $stat_style == gnu ]]; then
        stat -Lc '%d:%i:%u:%a:%h' -- "$1" 2>/dev/null
    else
        stat -f '%d:%i:%u:%Lp:%l' -- "$1" 2>/dev/null
    fi
}

stat_mode() {
    if [[ $stat_style == gnu ]]; then
        stat -Lc '%a' -- "$1" 2>/dev/null
    else
        stat -f '%Lp' -- "$1" 2>/dev/null
    fi
}

stat_size() {
    if [[ $stat_style == gnu ]]; then
        stat -Lc '%s' -- "$1" 2>/dev/null
    else
        stat -f '%z' -- "$1" 2>/dev/null
    fi
}

# A private entry is owned by this identity and writable by no group or other.
private_entry() {
    local mode
    [[ -O $1 ]] || return 1
    mode=$(stat_mode "$1") || return 1
    (( (8#$mode & 8#022) == 0 ))
}

# The helper's own location is its authority: <runtime-root>/scripts/<name>.
helper=${BASH_SOURCE[0]}
[[ $helper == /* ]] || helper=$PWD/$helper
helper_directory=$(dirname -- "$helper")
if [[ ${helper_directory##*/} != scripts ]]; then
    deny 'misplaced helper'
fi
runtime_root=$(dirname -- "$helper_directory")
if [[ ! -d $runtime_root || -L $runtime_root || ! -O $runtime_root ]]; then
    deny 'untrusted runtime root'
fi

deploy_directory=$runtime_root/.runtime-deploy
if [[ ! -e $deploy_directory && ! -L $deploy_directory ]]; then
    exit 0
fi
if [[ ! -d $deploy_directory || -L $deploy_directory ]]; then
    deny 'deploy state is not a private directory'
fi
private_entry "$deploy_directory" || deny 'deploy state is not a private directory'
# The state and lock entries can only be resolved with owner read and search
# permission. Without them the state is unknown, which denies, never "absent".
deploy_mode=$(stat_mode "$deploy_directory") || deny 'deploy state is unsearchable'
(( (8#$deploy_mode & 8#500) == 8#500 )) || deny 'deploy state is unsearchable'

# A lock is busy only when flock reports its conflict code. Every other result,
# including a missing or unopenable file and a kernel error, denies.
check_lock() {
    local path=$1 label=$2 descriptor=$fd_root/8 opened current mode owner links status
    if ! exec 8<"$path" 2>/dev/null; then
        deny "$label is not a regular file"
    fi
    if [[ -L $path || ! -f $descriptor ]]; then
        deny "$label is not a regular file"
    fi
    opened=$(stat_identity "$descriptor") || deny "$label check failed"
    current=$(stat_identity "$path") || deny "$label check failed"
    [[ $opened == "$current" ]] || deny "$label changed during validation"
    IFS=: read -r _device _inode owner mode links <<< "$opened"
    [[ $owner == "$EUID" && $links == 1 ]] || deny "$label is not owner-controlled"
    (( (8#$mode & 8#077) == 0 )) || deny "$label is not owner-controlled"
    if flock -n -E 75 8; then
        exec 8<&-
        return 1
    else
        status=$?
    fi
    exec 8<&-
    (( status == 75 )) || deny "$label check failed"
    return 0
}

state_file=$deploy_directory/$environment.state
if ! exec 7<"$state_file" 2>/dev/null; then
    # Only a path that does not exist at all is a normal start. A dangling
    # symlink or a directory is an unexpected input.
    if [[ -e $state_file || -L $state_file ]]; then
        deny 'deployment state is not a regular file'
    fi
    exit 0
fi

state_descriptor=$fd_root/7
if [[ -L $state_file || ! -f $state_descriptor ]]; then
    deny 'deployment state is not a regular file'
fi
opened=$(stat_identity "$state_descriptor") || deny 'deployment state changed during validation'
current=$(stat_identity "$state_file") || deny 'deployment state changed during validation'
[[ $opened == "$current" ]] || deny 'deployment state changed during validation'
IFS=: read -r _device _inode owner mode links <<< "$opened"
[[ $owner == "$EUID" && $links == 1 ]] || deny 'deployment state is not owner-controlled'
(( (8#$mode & 8#077) == 0 )) || deny 'deployment state is not owner-controlled'
state_size=$(stat_size "$state_descriptor") || deny 'deployment state changed during validation'
# The deploy writes this file atomically, so a grow past the bound means it is
# not deploy state any more.
(( state_size <= 256 )) || deny 'deployment state is too large'
state_text=$(dd bs=257 count=1 2>/dev/null <&7; printf x)
exec 7<&-
current=$(stat_identity "$state_file") || deny 'deployment state changed during validation'
[[ $current == "$opened" ]] || deny 'deployment state changed during validation'
expected_state=$(printf 'DEPLOYMENT_STATE=restart-authorized\nENVIRONMENT=%s\nRELEASE_SHA=%s\nx' \
    "$environment" "$expected_release_sha")
[[ $state_text == "$expected_state" ]] || deny 'deployment state does not authorize a restart'

# Restart authorization needs both the durable deploy lock and the restart
# window lock to be held by the deploy that wrote this state.
if ! check_lock "$deploy_directory/$environment.lock" 'main lock'; then
    deny 'restart authorization is not held'
fi
if ! check_lock "$deploy_directory/$environment.restart.lock" 'restart lock'; then
    deny 'restart authorization is not held'
fi
exit 0
