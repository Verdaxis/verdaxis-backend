#!/usr/bin/env bash
# Destructive rehearsal is confined to a fresh GitHub-hosted Linux runner.
set -euo pipefail

if [[ ${1:-} != --disposable || $# != 1 ]]; then
    printf 'usage: %s --disposable\n' "$0" >&2
    exit 2
fi
# This gate must precede directory creation, traps, sourcing, or tool execution
# that can mutate the machine. In particular, a Mac invocation has no effects.
if [[ $(uname -s) != Linux || $EUID != 0 || ${CI:-} != true ||
      ${GITHUB_ACTIONS:-} != true || ${RUNNER_ENVIRONMENT:-} != github-hosted ]]; then
    printf 'FAIL disposable_environment: require root on a GitHub-hosted Linux runner\n' >&2
    exit 2
fi
if [[ $(</proc/1/comm) != systemd || ! -f /etc/os-release ]]; then
    printf 'FAIL disposable_environment: real systemd PID 1 required\n' >&2
    exit 2
fi
# shellcheck disable=SC1091
source /etc/os-release
if [[ $ID != ubuntu || ( $VERSION_ID != 22.04 && $VERSION_ID != 24.04 ) ]]; then
    printf 'FAIL disposable_environment: Ubuntu 22.04 or 24.04 required\n' >&2
    exit 2
fi
for name in REHEARSAL_ENVIRONMENT REHEARSAL_CURRENT_ARTIFACT_DIR REHEARSAL_PREVIOUS_ARTIFACT_DIR REHEARSAL_EVIDENCE_DIR REHEARSAL_SOURCE_DIR; do
    [[ -n ${!name:-} ]] || { printf 'FAIL disposable_environment: missing %s\n' "$name" >&2; exit 2; }
done
[[ $REHEARSAL_ENVIRONMENT == staging || $REHEARSAL_ENVIRONMENT == production ]] || exit 2
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)
[[ $(realpath -e -- "$REHEARSAL_SOURCE_DIR") == "$ROOT" ]] || { printf 'FAIL source path differs\n' >&2; exit 2; }
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin LC_ALL=C
unset BASH_ENV ENV CDPATH HTTP_PROXY HTTPS_PROXY ALL_PROXY http_proxy https_proxy all_proxy
export NO_PROXY='*' no_proxy='*'
umask 077
# All support code comes from the same reviewed source tree, never artifacts.
# shellcheck source=deploy/tests/rehearsal/driver.sh
source "$ROOT/deploy/tests/rehearsal/driver.sh"
rehearsal_main
