#!/usr/bin/env bash
# Superseded schema helper (fixture data only; never run).
set -euo pipefail
cd "$(dirname "$0")/.."
venv/bin/alembic -c alembic.ini upgrade head
venv/bin/python -m app.cli.verify_schema
