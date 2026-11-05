#!/usr/bin/env bash
# Phase 10 Task 33: post-deletion evidence checker (fixture mode only).
#
#   bash deploy/tests/post-deletion-contract.sh --fixture DIR --public-key KEY
#
# Reads synthetic JSON evidence files from the fixture directory and asserts
# the eight named checks from the evidence manifest's Task 31/33 cells. It
# contacts no host, unit, timer, cron file or network: every assertion is
# decided from the fixture JSON. Each check prints exactly one line,
# `PASS <name>` or `FAIL <name>: <reason>`, and the exit status is 0 only when
# every check passes. Fixture GREEN proves only the checker; signed installed
# evidence is collected separately at the authorized window (plan Task 33).
#
# Expected evidence files (all relative to the fixture directory):
#   deleted-paths.json        every path the removal deleted, with digests
#   inventory.json            post-deletion host inventory covering both hosts
#   inventory.json.sig        detached SHA-256 signature over inventory.json
#   hosts/<host>/state.json   installed-state scan results per host
#   schedules.json            current backup and staging-pull schedules
#   schedules-w0.json         the W0 register's pinned schedule digests
#   passive-checks.json       passive harm observations per group
#   review.json               independent review record with finding closure
#   removal-ledger.json       copy of tests/rust_contract/phase10/removal-ledger.json
#   nightly-verify.json       the Rust backup verify monitor's nightly verdict
#
# "Signed" means `openssl dgst -sha256 -verify PUBKEY -signature SIG FILE`
# with the public key path given by --public-key. The fixtures are exercised
# by tests/rust_contract/test_phase10_post_deletion.py, which generates a
# throwaway key pair per run.

# The jq programs below are single-quoted on purpose: they must not expand.
# shellcheck disable=SC2016
set -euo pipefail
export LC_ALL=C

# Passive harm limits: the nightly error budget and the peak resident memory
# a group may report while still counting as clean. Named here so a fixture
# cannot relax them.
FIVE_XX_MAX_RATE=0
GROUP_PEAK_RSS_MIB_MAX=1024

usage() {
    printf 'usage: %s --fixture DIR --public-key KEY\n' "$0" >&2
    exit 2
}

FIXTURE=""
PUBLIC_KEY=""
while [[ $# -gt 0 ]]; do
    case $1 in
        --fixture)
            [[ $# -ge 2 ]] || usage
            FIXTURE=$2
            shift 2
            ;;
        --public-key)
            [[ $# -ge 2 ]] || usage
            PUBLIC_KEY=$2
            shift 2
            ;;
        *) usage ;;
    esac
done
[[ -n "$FIXTURE" && -n "$PUBLIC_KEY" ]] || usage
[[ -d "$FIXTURE" ]] || { printf 'missing fixture directory: %s\n' "$FIXTURE" >&2; exit 2; }
[[ -f "$PUBLIC_KEY" ]] || { printf 'missing public key: %s\n' "$PUBLIC_KEY" >&2; exit 2; }

failures=0

# report NAME EXIT_STATUS REASON: exactly one output line per check name.
report() {
    local name=$1 status=$2 reason=$3
    if [[ "$status" -eq 0 ]]; then
        printf 'PASS %s\n' "$name"
    else
        printf 'FAIL %s: %s\n' "$name" "$reason"
        failures=$((failures + 1))
    fi
}

# json_accepts FILE JQ_FILTER: succeeds when the file is valid JSON and the
# filter evaluates to a truthy value under `jq -e`.
json_accepts() {
    local file=$1 filter=$2
    shift 2
    [[ -f "$file" ]] || return 1
    jq -e "$filter" "$@" "$file" > /dev/null 2>&1
}

# sign_ok FILE SIG: detached SHA-256 signature verification with openssl.
sign_ok() {
    local file=$1 sig=$2
    [[ -f "$sig" ]] || return 1
    openssl dgst -sha256 -verify "$PUBLIC_KEY" -signature "$sig" "$file" > /dev/null 2>&1
}

# ---- check 1: host inventory is complete and signed ------------------------------

check_inventory() {
    local name=host_inventory_is_complete_and_signed
    local inventory="$FIXTURE/inventory.json"
    if [[ ! -f "$inventory" ]]; then
        report "$name" 1 'missing inventory.json'
        return
    fi
    if ! sign_ok "$inventory" "$FIXTURE/inventory.json.sig"; then
        report "$name" 1 'signature over inventory.json does not verify'
        return
    fi
    # Completeness: both hosts, every W0 acquired surface observed, a digest
    # for every deleted path, no path still present, and the recorded file
    # digest matching the inventory bytes that were signed.
    if json_accepts "$inventory" '
        (keys | sort) == ["collected_at", "hosts", "paths", "signature"] and
        ([.hosts[].id] | sort) == ["production", "staging"] and
        ([.hosts[].surfaces | keys_unsorted[]] | unique | sort) ==
            ["cron", "processes", "timers", "unit_dropins", "unit_files", "usr_local"] and
        ([.hosts[].surfaces[]] | all(.observed_at | type == "string" and length > 0)) and
        ([.paths[].path] | sort) == ($paths | sort) and
        ([.paths[]] | all(.sha256 | test("^[0-9a-f]{64}$"))) and
        ([.paths[] | select(.present == true)] | length) == 0 and
        .signature.deleted_paths_sha256 == $paths_digest and
        (.signature.key_id | length) > 0
    ' --argjson paths "$(jq -c '[.[].path]' "$FIXTURE/deleted-paths.json")" \
      --arg paths_digest "$(sha256sum "$FIXTURE/deleted-paths.json" | cut -d' ' -f1)"; then
        report "$name" 0 ''
    else
        report "$name" 1 'inventory is incomplete, a deleted path is present or the digest record mismatches'
    fi
}

# ---- check 2: no python process, unit, timer or cron on either host --------------

check_no_python() {
    local name=no_python_process_unit_timer_or_cron_on_either_host
    local state
    for state in "$FIXTURE/hosts/production/state.json" "$FIXTURE/hosts/staging/state.json"; do
        # One scan file per host; every unit, timer, cron entry and process
        # name it lists must be free of any Python reference.
        if ! json_accepts "$state" '
            (keys | sort) ==
                ["cron", "paths_absent", "processes", "references", "timers", "units"] and
            ([.units[] + .timers[] + .cron[] + .processes[]] |
                all(test("python"; "i") | not))
        '; then
            report "$name" 1 "$state is missing, incomplete or still references Python"
            return
        fi
    done
    report "$name" 0 ''
}

# ---- check 3: backup and staging pull schedules are unchanged --------------------

check_schedules() {
    local name=backup_and_staging_pull_schedules_are_unchanged
    # Every W0 retained schedule must appear with its pinned digests (pinned
    # equality covers both the expression and the target identity), and no
    # schedule may have been added or removed after the deletion.
    if json_accepts "$FIXTURE/schedules.json" '
        (.retained | sort_by(.id) | map({id, expression_sha256, target_sha256})) ==
            ($w0[0].retained | sort_by(.id) | map({id, expression_sha256, target_sha256})) and
        ((.added // []) | length) == 0 and
        ((.removed // []) | length) == 0
    ' --slurpfile w0 "$FIXTURE/schedules-w0.json"; then
        report "$name" 0 ''
    else
        report "$name" 1 'a retained schedule changed or a schedule was added or removed'
    fi
}

# ---- check 4: passive harm checks are clean for each group ------------------------

check_passive() {
    local name=passive_harm_checks_are_clean_for_each_group
    # Each group must carry all four passive observations and every one must
    # sit inside the named limits: the 5xx budget, no new error class, no
    # restart, and peak resident memory within the ceiling.
    if json_accepts "$FIXTURE/passive-checks.json" '
        (.groups | length) > 0 and
        ([.groups[].id] | unique | length) == (.groups | length) and
        ([.groups[].observations | keys_unsorted | sort] |
            all(. == ["error_classes", "five_xx", "memory_rss_mib", "restarts"])) and
        ([.groups[].observations.five_xx.rate] | all(. <= $five_xx_max)) and
        ([.groups[].observations.error_classes.new_count] | all(. == 0)) and
        ([.groups[].observations.restarts.count] | all(. == 0)) and
        ([.groups[].observations.memory_rss_mib.peak] | all(. <= $rss_max_mib))
    ' --argjson five_xx_max "$FIVE_XX_MAX_RATE" \
      --argjson rss_max_mib "$GROUP_PEAK_RSS_MIB_MAX"; then
        report "$name" 0 ''
    else
        report "$name" 1 'a group has a dirty passive observation or a missing observation kind'
    fi
}

# ---- check 5: independent review findings are closed -----------------------------

check_review() {
    local name=independent_review_findings_are_closed
    # Every critical and high finding must be closed with a test reference.
    # An accepted exception may survive only when it names a scope and reason
    # (plan Task 33: unrelated shared-host work or off-host tools only).
    if json_accepts "$FIXTURE/review.json" '
        ([.findings[] | select(.severity == "critical" or .severity == "high")] |
            all(.status == "closed" and (.test | length > 0))) and
        ([.findings[] | select(.status == "accepted-exception")] |
            all((.exception.scope | length) > 0 and (.exception.reason | length) > 0))
    '; then
        report "$name" 0 ''
    else
        report "$name" 1 'a critical or high finding is open or an exception is unscoped'
    fi
}

# ---- check 6: removal ledger has no gate or ownerless rows -----------------------

check_ledger() {
    local name=removal_ledger_has_no_gate_or_ownerless_rows
    # No row may be ownerless or claim to be a gate row; every end state must
    # be an accepted disposition and an accepted gone row needs evidence.
    if json_accepts "$FIXTURE/removal-ledger.json" '
        ([.artifacts[] | select(.owner == null or (.owner | gsub("^\\s+|\\s+$"; "")) == "")] | length) == 0 and
        ([.artifacts[] | select(.gate == true or .gate_required == true)] | length) == 0 and
        ([.artifacts[].end_state] |
            all(. == "rust" or . == "shell" or . == "gone" or . == "retired-offhost")) and
        ([.artifacts[] | select(.status == "accepted" and .end_state == "gone")] |
            all((.deletion_evidence.path // "" | length) > 0))
    '; then
        report "$name" 0 ''
    else
        report "$name" 1 'a ledger row is ownerless, gate-claimed or lacks deletion evidence'
    fi
}

# ---- check 7: deleted paths are absent on both hosts -----------------------------

check_paths_absent() {
    local name=deleted_paths_are_absent_on_both_hosts
    local state
    for state in "$FIXTURE/hosts/production/state.json" "$FIXTURE/hosts/staging/state.json"; do
        # The host scan must list every deleted path as absent, with no
        # stray entries and no installed reference still present.
        if ! json_accepts "$state" '
            (.paths_absent | sort) == ($paths[0] | map(.path) | sort) and
            (.paths_absent | unique | length) == ($paths[0] | length) and
            ([.references[] | select(.present == true)] | length) == 0
        ' --slurpfile paths "$FIXTURE/deleted-paths.json"; then
            report "$name" 1 "$state does not list every deleted path as absent"
            return
        fi
    done
    report "$name" 0 ''
}

# ---- check 8: the Rust backup verify monitor accepts the nightly artifact --------

check_monitor() {
    local name=the_rust_backup_verify_monitor_accepts_the_nightly_artifact
    # The nightly verdict must come from the Rust monitor and its four checks
    # must all hold for a digest-named artifact.
    if json_accepts "$FIXTURE/nightly-verify.json" '
        .monitor.kind == "rust" and
        .artifact.kind == "nightly" and
        (.artifact.sha256 | test("^[0-9a-f]{64}$")) and
        .verdict == "accepted" and
        .checks.archive_digest_matches_manifest and
        .checks.manifest_member_digests and
        .checks.retention_window_named and
        .checks.dump_restores_in_disposable_database
    '; then
        report "$name" 0 ''
    else
        report "$name" 1 'the nightly verdict is missing, refused or not from the Rust monitor'
    fi
}

check_inventory
check_no_python
check_schedules
check_passive
check_review
check_ledger
check_paths_absent
check_monitor

if [[ "$failures" -gt 0 ]]; then
    exit 1
fi
