#!/usr/bin/env bash
# No external database URL is accepted. The Python runner owns its container.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec "$ROOT/venv/bin/python" "$ROOT/tests/phase0/runner.py" "$@"
