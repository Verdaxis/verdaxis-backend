#!/usr/bin/env bash
# Deployment start guard: allow a backend start only outside a deploy or inside
# the exact restart window.
#
# The unit runs this file before any Rust artifact exists, so it must never call
# the binary or an interpreter. It uses bash, GNU/BSD stat, dd and util-linux
# flock only. Owner-controlled directories and files remove other identities
# from the path; the guard is not a defence against the deploy identity itself.
set -euo pipefail
export LC_ALL=C
umask 077

deny() {
    printf 'Backend start denied by deployment guard.\n' >&2
    exit 1
}

if (( $# != 4 )) || [[ $1 != --environment || $3 != --expected-release-sha ]]; then
    deny
fi
environment=$2
expected_release_sha=$4
case $environment in
    production | staging) ;;
    *) deny ;;
esac
if [[ ! $expected_release_sha =~ ^[0-9a-f]{40}$ ]]; then
    deny
fi

# GNU coreutils and BSD stat differ only in their format flag and names.
if stat -c %d / >/dev/null 2>&1; then
    stat_style=gnu
else
    stat_style=bsd
fi
stat_field() {
    if [[ $stat_style == gnu ]]; then
        stat -c "$1" -- "$3" 2>/dev/null
    else
        stat -f "$2" -- "$3" 2>/dev/null
    fi
}

# A private entry is owned by this identity and writable by no group or other.
private_entry() {
    [[ -O $1 ]] || return 1
    local mode
    mode=$(stat_field '%A' '%Sp' "$1") || return 1
    [[ ${mode:5:1} != w && ${mode:8:1} != w ]]
}

single_regular_file() {
    [[ -f $1 && ! -L $1 ]] || return 1
    private_entry "$1" || return 1
    [[ $(stat_field '%h' '%l' "$1") == 1 ]]
}

lock_is_busy() {
    local lock_status
    single_regular_file "$1" || return 1
    exec 8<"$1" || return 1
    if flock -n 8; then
        exec 8<&-
        return 1
    else
        lock_status=$?
    fi
    exec 8<&-
    # util-linux flock exits 1 on a held lock and above 1 on an error. Any other
    # outcome must refuse the start, never authorize it.
    (( lock_status == 1 ))
}

# The helper's own location is its authority: <runtime-root>/scripts/<name>.
helper=${BASH_SOURCE[0]}
[[ $helper == /* ]] || helper=$PWD/$helper
helper_directory=$(dirname -- "$helper")
if [[ ${helper_directory##*/} != scripts ]]; then
    deny
fi
runtime_root=$(dirname -- "$helper_directory")
if [[ ! -d $runtime_root || -L $runtime_root || ! -O $runtime_root ]]; then
    deny
fi

deploy_directory=$runtime_root/.runtime-deploy
if [[ ! -e $deploy_directory && ! -L $deploy_directory ]]; then
    exit 0
fi
if [[ ! -d $deploy_directory || -L $deploy_directory ]]; then
    deny
fi
private_entry "$deploy_directory" || deny

state_file=$deploy_directory/$environment.state
if [[ -e $state_file || -L $state_file ]]; then
    single_regular_file "$state_file" || deny
    state_size=$(stat_field '%s' '%z' "$state_file") || deny
    # The deploy writes this file atomically, so a grow past the bound means it
    # is not deploy state any more.
    (( state_size <= 256 )) || deny
    exec 7<"$state_file" || deny
    state_text=$(dd bs=257 count=1 2>/dev/null <&7; printf x)
    exec 7<&-
    expected_state=$(printf 'DEPLOYMENT_STATE=restart-authorized\nENVIRONMENT=%s\nRELEASE_SHA=%s\nx' \
        "$environment" "$expected_release_sha")
    [[ $state_text == "$expected_state" ]] || deny
else
    # A normal start: no deploy is in progress for this environment.
    exit 0
fi

# Restart authorization needs both the durable deploy lock and the restart
# window lock to be held by the deploy that wrote this state.
if ! lock_is_busy "$deploy_directory/$environment.lock" \
    || ! lock_is_busy "$deploy_directory/$environment.restart.lock"; then
    deny
fi
exit 0
