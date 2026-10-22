# Print the source-state and gate markers that scripts/rust_contract/phase6_record.py reads.
#
# Source this file in a test-host command, inside the repository checkout:
#
#   . scripts/rust_contract/phase6_markers.sh
#   phase6_source                          # after every checkout
#   phase6_gate RED_UNIT cargo test ...    # one gate section with the command's own exit
#   phase6_control /private/one.patch      # apply one control mutation to a clean checkout
#
# The functions print identities, digests, UTC times and exit codes only. They need
# bash 3.2 (macOS) or later and Git.

phase6_utc() {
    date -u +%Y-%m-%dT%H:%M:%SZ
}

phase6_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum | cut -d ' ' -f 1
    else
        shasum -a 256 | cut -d ' ' -f 1
    fi
}

# Describe HEAD and every local change of the checkout.
phase6_source() {
    printf 'SOURCE_SHA=%s\n' "$(git rev-parse HEAD)"
    printf 'SOURCE_TREE=%s\n' "$(git rev-parse 'HEAD^{tree}')"
    printf 'SOURCE_DIFF_SHA256=%s\n' "$(git diff --binary HEAD | phase6_sha256)"
    printf 'SOURCE_UNTRACKED=%s\n' \
        "$(git ls-files --others --exclude-standard | wc -l | tr -d ' ')"
}

# Apply exactly one control mutation to a clean checkout, then describe the result.
phase6_control() {
    if ! git diff --quiet HEAD; then
        echo "phase6_control: the checkout already has local changes" >&2
        return 2
    fi
    git apply --binary "$1" || return
    phase6_source
    printf 'CONTROL_PATCH_SHA256=%s\n' "$(phase6_sha256 < "$1")"
}

# Run one command as a named gate section and return the command's own exit code.
phase6_gate() {
    local name=$1 code
    shift
    printf 'GATE=%s UTC=%s\n' "$name" "$(phase6_utc)"
    printf 'COMMAND=%s\n' "$*"
    "$@"
    code=$?
    printf '%s_EXIT=%s UTC=%s\n' "$name" "$code" "$(phase6_utc)"
    return "$code"
}
