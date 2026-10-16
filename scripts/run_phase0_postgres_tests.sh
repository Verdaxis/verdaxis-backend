#!/usr/bin/env bash
# No external database URL is accepted. The Python runner owns its container.
# With no arguments, require every Phase 0 suite and local Review Focus case.
# Commands, counts and artifact hashes are saved beside the synthetic fixtures.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec "$ROOT/venv/bin/python" "$ROOT/tests/phase0/runner.py" "$@"
