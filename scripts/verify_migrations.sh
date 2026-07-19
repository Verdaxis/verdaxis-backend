#!/usr/bin/env bash
# Verify a database can reach Alembic head and has no model/schema drift.
# The caller owns the database lifecycle; the disposable PostGIS runner is
# scripts/run_product_analytics_postgres_tests.sh.
set -euo pipefail

BACKEND_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ -z "${DATABASE_URL:-}" ]]; then
  echo "DATABASE_URL is required; refusing to verify an implicit database" >&2
  exit 2
fi

DATABASE_NAME="${DATABASE_URL##*/}"
DATABASE_NAME="${DATABASE_NAME%%\?*}"
if [[ "$DATABASE_NAME" != *_test ]]; then
  echo "refusing migration verification database '$DATABASE_NAME': name must end with _test" >&2
  exit 2
fi
if [[ "$DATABASE_URL" == *"api.verdaxis.exchange"* ]]; then
  echo "refusing migration verification against the Verdaxis production host" >&2
  exit 2
fi

cd "$BACKEND_ROOT"
PYTHONDONTWRITEBYTECODE=1 "${ALEMBIC_BIN:-alembic}" upgrade head
PYTHONDONTWRITEBYTECODE=1 "${ALEMBIC_BIN:-alembic}" current --check-heads
PYTHONDONTWRITEBYTECODE=1 "${ALEMBIC_BIN:-alembic}" check
