#!/usr/bin/env bash
# Deployment start guard: allow a backend start only outside a deploy or inside
# the exact restart window.
#
# The unit runs this file before any Rust artifact exists, so it must never call
# the binary or an interpreter. It uses bash, stat, dd and util-linux flock only.
# Every input it cannot read, or does not expect, denies the start. Owner
# controlled directories and files remove other identities from the path; the
# guard is not a defence against the deploy identity itself.
#
# Each file is opened once. The open, the descriptor checks and the read happen
# inside one subshell, so a failed open returns a status instead of ending the
# guard, and every check runs on the descriptor that was actually opened.
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
root_mode=$(stat_mode "$runtime_root") || deny 'untrusted runtime root'
(( (8#$root_mode & 8#500) == 8#500 )) || deny 'untrusted runtime root'

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

# Reads the state through one descriptor and prints its exact bytes. The
# redirection on the group opens the file once; every check and the read use
# that descriptor. A failed open returns 1 instead of ending the guard.
read_state() {
    local path=$1 descriptor=$fd_root/7 opened current owner mode links size text
    if ! {
        if [[ ! -f $descriptor ]]; then
            exit 10
        fi
        [[ ! -L $path ]] || exit 10
        opened=$(stat_identity "$descriptor") || exit 11
        current=$(stat_identity "$path") || exit 11
        [[ $opened == "$current" ]] || exit 11
        IFS=: read -r _device _inode owner mode links <<< "$opened"
        [[ $owner == "$EUID" && $links == 1 ]] || exit 12
        (( (8#$mode & 8#077) == 0 )) || exit 12
        size=$(stat_size "$descriptor") || exit 11
        # The deploy writes this file atomically, so a grow past the bound means
        # it is not deploy state any more.
        (( size <= 256 )) || exit 13
        text=$(dd bs=257 count=1 2>/dev/null <&7; printf x)
        current=$(stat_identity "$path") || exit 11
        [[ $current == "$opened" ]] || exit 11
        printf '%s' "$text"
    } 7<"$path" 2>/dev/null; then
        # Only a path that does not exist at all is a normal start. A dangling
        # symlink or a directory is an unexpected input.
        if [[ -e $path || -L $path ]]; then
            exit 10
        fi
        exit 15
    fi
}

# Busy and refused are separate outcomes: only flock's conflict code (75) is a
# held lock. Every other result, including a missing or unopenable file and a
# kernel error, denies.
lock_state() {
    local path=$1 descriptor=$fd_root/8 opened current owner mode links status
    if ! {
        if [[ ! -f $descriptor ]]; then
            exit 20
        fi
        [[ ! -L $path ]] || exit 20
        opened=$(stat_identity "$descriptor") || exit 22
        current=$(stat_identity "$path") || exit 22
        [[ $opened == "$current" ]] || exit 22
        IFS=: read -r _device _inode owner mode links <<< "$opened"
        [[ $owner == "$EUID" && $links == 1 ]] || exit 21
        (( (8#$mode & 8#077) == 0 )) || exit 21
        if flock -n -E 75 8; then
            exit 1
        else
            status=$?
        fi
        (( status == 75 )) || exit 22
        exit 0
    } 8<"$path" 2>/dev/null; then
        exit 20
    fi
}

# require_lock LABEL PATH: a held lock passes; a free lock is the caller's
# decision; every refusal denies with the reason for that outcome. The check
# runs in a subshell so its outcome codes never become the guard's exit status.
require_lock() {
    local label=$1 path=$2 status=0
    ( lock_state "$path" ) || status=$?
    case $status in
        0) return 0 ;;
        1) return 1 ;;
        20) deny "$label is not a regular file" ;;
        21) deny "$label is not owner-controlled" ;;
        *) deny "$label check failed" ;;
    esac
}

state_file=$deploy_directory/$environment.state
state_status=0
state_text=$(read_state "$state_file") || state_status=$?
case $state_status in
    0) ;;
    15) exit 0 ;;
    10) deny 'deployment state is not a regular file' ;;
    11) deny 'deployment state changed during validation' ;;
    12) deny 'deployment state is not owner-controlled' ;;
    13) deny 'deployment state is too large' ;;
    *) deny 'deployment state changed during validation' ;;
esac
expected_state=$(printf 'DEPLOYMENT_STATE=restart-authorized\nENVIRONMENT=%s\nRELEASE_SHA=%s\nx' \
    "$environment" "$expected_release_sha")
[[ $state_text == "$expected_state" ]] || deny 'deployment state does not authorize a restart'

# Restart authorization needs both the durable deploy lock and the restart
# window lock to be held by the deploy that wrote this state.
if ! require_lock 'main lock' "$deploy_directory/$environment.lock"; then
    deny 'restart authorization is not held'
fi
if ! require_lock 'restart lock' "$deploy_directory/$environment.restart.lock"; then
    deny 'restart authorization is not held'
fi
exit 0
