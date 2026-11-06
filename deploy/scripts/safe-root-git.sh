#!/usr/bin/env bash
# Source from the trusted tooling directory, never from an archived candidate.
# Every Git operation uses an isolated environment and an inert local config.
# Subshell isolation is intentional: no Git environment changes reach callers.
# shellcheck disable=SC2030,SC2031
safe_root_git() (
    local repository=$1 variable configuration name
    shift
    for variable in "${!GIT_@}"; do unset "$variable"; done
    export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_NO_REPLACE_OBJECTS=1
    export GIT_TERMINAL_PROMPT=0 GIT_EDITOR=/bin/false GIT_SEQUENCE_EDITOR=/bin/false
    unset SSH_ASKPASS
    if [[ -d $repository/.git && ! -L $repository/.git ]]; then
        local metadata=$repository/.git
    elif [[ -d $repository/objects && -f $repository/HEAD && ! -L $repository ]]; then
        local metadata=$repository
    else
        printf 'Git requires a real metadata directory\n' >&2; return 1
    fi
    for name in config index HEAD objects refs; do
        [[ ! -L $metadata/$name ]] || { printf 'Git metadata must not be redirected\n' >&2; return 1; }
    done
    [[ ! -e $metadata/commondir && ! -L $metadata/commondir && ! -e $metadata/objects/info/alternates && ! -L $metadata/objects/info/alternates ]] || {
        printf 'Git metadata indirection is unsupported\n' >&2; return 1;
    }
    # --file and --no-includes parse data without loading checkout directives.
    configuration=$(/usr/bin/git config --file "$metadata/config" --no-includes --name-only --list) || return
    while IFS= read -r name; do
        case $name in
            core.repositoryformatversion|core.filemode|core.bare|core.logallrefupdates|core.ignorecase|core.precomposeunicode|remote.origin.url|remote.origin.fetch|branch.staging.remote|branch.staging.merge|branch.prod.remote|branch.prod.merge) ;;
            *) printf 'Git configuration outside the inert tooling allowlist\n' >&2; return 1 ;;
        esac
    done <<< "$configuration"
    /usr/bin/git -C "$repository" -c "safe.directory=$repository" \
        -c core.hooksPath=/dev/null -c core.fsmonitor=false "$@"
)

# A private scratch repository has no checkout config to inspect until init.
safe_root_git_init() (
    local repository=$1 variable
    for variable in "${!GIT_@}"; do unset "$variable"; done
    export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_NO_REPLACE_OBJECTS=1
    [[ ! -e $repository && ! -L $repository ]] || return 1
    /usr/bin/git -c core.hooksPath=/dev/null -c core.fsmonitor=false init --bare --template= "$repository"
)
