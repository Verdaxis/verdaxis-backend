#!/usr/bin/env bash
# Phase 10 Task 34: the one final measurement wrapper (plan 10.6).
#
# Report-only gate: this wrapper validates the reservation and the pinned
# seed/release/resources, refuses a prior started-run marker, records the
# single run start durably, and calls the retained measurement tool once. It
# never changes service or deploy state, never starts load on its own, and
# stops only its own disposable processes in the cleanup traps. A failure is
# recorded and never auto-retried.
#
# In this lane only `--offline-check` is exercised: it validates the config
# and the offline fixtures without contacting a host or starting anything.
set -euo pipefail

usage() {
    echo "usage: $0 --config <measure-final.toml> --output <dir> --rust-only" \
         "(--offline-check | --reservation <signed-reservation.json>)" >&2
}

config=""
output=""
offline_check=false
reservation=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --config) config=${2:?}; shift 2 ;;
        --output) output=${2:?}; shift 2 ;;
        --rust-only) rust_only=true; shift ;;
        --offline-check) offline_check=true; shift ;;
        --reservation) reservation=${2:?}; shift 2 ;;
        *) usage; exit 2 ;;
    esac
done

root=$(git -C "$(dirname "$0")/.." rev-parse --show-toplevel)
cd "$root"

[ -n "$config" ] || { usage; exit 2; }
[ -n "$output" ] || { usage; exit 2; }
[ "${rust_only:-}" = true ] || { echo "refused: the final measurement is Rust-only" >&2; exit 2; }

if ! $offline_check; then
    if [ -z "$reservation" ]; then
        echo "refused: give --offline-check or an explicit signed --reservation file" >&2
        exit 3
    fi
    [ -f "$reservation" ] || { echo "refused: reservation file not found: $reservation" >&2; exit 3; }
fi

# The offline check validates the config and its pinned fixtures, then stops.
if $offline_check; then
    python3 - "$config" <<'PY'
import hashlib
import sys
import tomllib
from pathlib import Path

config_path = Path(sys.argv[1])
document = tomllib.loads(config_path.read_text())
assert document.get("report_only") is True, "the final measurement is report-only"
assert document.get("requires_cutover_complete") is True, "cutover evidence required"
seed = Path(document["seed"])
digest = hashlib.sha256(seed.read_bytes()).hexdigest()
assert document.get("seed_sha256") == digest, "the config seed pin does not match the seed bytes"
baseline = Path("tests/rust_contract/python-baseline-small.json")
assert document.get("baseline_sha256") == hashlib.sha256(baseline.read_bytes()).hexdigest(), (
    "the frozen baseline pin does not match"
)
memory = Path(document["python_memory_baseline"])
memory_document = __import__("json").loads(memory.read_text())
raw = memory_document["raw_report"]
assert len(raw["sha256"]) == 64 and len(raw["harness_sha"]) == 40, (
    "the Python memory evidence is unpinned (placeholders refuse before the run)"
)
print("OFFLINE_CHECK_OK")
PY
    echo "offline check passed; nothing was run, nothing was contacted"
    exit 0
fi

# Reservation mode: validate the reservation file minimally. The exact signed
# schema is checked by the PM's reservation tooling before the invocation.
python3 - "$reservation" <<'PY'
import json
import sys

document = json.loads(Path(sys.argv[1]).read_text())
for field in ("owner", "started_at", "ends_at", "exclusive", "signature"):
    if not document.get(field):
        raise SystemExit(f"refused: reservation is missing {field}")
if document.get("exclusive") is not True:
    raise SystemExit("refused: the reservation must be exclusive")
PY

# Refuse a prior started-run marker, then record this run's start durably.
marker="$output/.started-run"
if [ -e "$marker" ]; then
    echo "refused: a started-run marker exists at $marker; a second attempt needs a new founder decision" >&2
    exit 4
fi
mkdir -p "$output"
printf '{"started_at": "%s", "host": "%s"}\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$(uname -n)" > "$marker"
sync "$marker" 2>/dev/null || true

python=${PYTHON:-venv/bin/python}
cleanup() {
    status=$?
    if [ "$status" -ne 0 ]; then
        printf '{"failed_at": "%s", "status": %d}\n' \
            "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$status" >> "$output/.run-failures"
    fi
    # Only this wrapper's own disposable files are removed; the marker and the
    # report stay as evidence. No service or deploy state is touched.
    exit "$status"
}
trap cleanup EXIT

"$python" -m scripts.rust_contract.measure --config "$config" --output "$output" --rust-only
