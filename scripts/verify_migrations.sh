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

VERIFY_URL="${MIGRATOR_DATABASE_URL:-$DATABASE_URL}"
VERIFY_PATH="${VERIFY_URL%%\?*}"
DATABASE_NAME="${VERIFY_PATH##*/}"
if [[ "$DATABASE_NAME" != *_test ]]; then
  echo "refusing migration verification database '$DATABASE_NAME': name must end with _test" >&2
  exit 2
fi
if [[ "$VERIFY_URL" == *"api.verdaxis.exchange"* ]]; then
  echo "refusing migration verification against the Verdaxis production host" >&2
  exit 2
fi

cd "$BACKEND_ROOT"
ALEMBIC="${ALEMBIC_BIN:-alembic}"
HEAD_COUNT="$(PYTHONDONTWRITEBYTECODE=1 "$ALEMBIC" heads | awk '/\(head\)/ { count += 1 } END { print count + 0 }')"
if [[ "$HEAD_COUNT" != "1" ]]; then
  echo "refusing migration verification: expected exactly one standalone Alembic head, found $HEAD_COUNT" >&2
  exit 2
fi
PYTHONDONTWRITEBYTECODE=1 "$ALEMBIC" upgrade head
PYTHONDONTWRITEBYTECODE=1 "$ALEMBIC" current --check-heads
PYTHONDONTWRITEBYTECODE=1 "$ALEMBIC" check
