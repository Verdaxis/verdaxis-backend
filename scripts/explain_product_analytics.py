#!/usr/bin/env python3
"""Read-only EXPLAIN (ANALYZE, BUFFERS) report for the Product Analytics
Overview and Marketplace SQL (plan §2.6).

Runs every statement builder the two heaviest tabs execute, inside a
READ ONLY transaction, and writes redacted JSON plans (UUIDs, timestamps, and
dates are replaced before writing). The report — not an assumption — decides
whether the optional composite indexes land.

Usage:
    venv/bin/python scripts/explain_product_analytics.py --days 90 \
        --output /tmp/product-analytics-explain.json

Requires DATABASE_URL (or app settings) pointing at PostgreSQL; refuses
anything else. Never mutates data.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import re
import sys
from datetime import UTC, datetime, timedelta
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from sqlalchemy import text  # noqa: E402
from sqlalchemy.ext.asyncio import create_async_engine  # noqa: E402

from app.config import settings  # noqa: E402
from app.services import product_analytics as pa  # noqa: E402

_UUID_RE = re.compile(
    r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
)
_TIMESTAMP_RE = re.compile(r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:\+\d{2}:?\d{2})?")
_DATE_RE = re.compile(r"\d{4}-\d{2}-\d{2}")


def _redact(plan_text: str) -> str:
    plan_text = _UUID_RE.sub("<uuid>", plan_text)
    plan_text = _TIMESTAMP_RE.sub("<timestamp>", plan_text)
    return _DATE_RE.sub("<date>", plan_text)


def _statements(days: int, dialect: str):
    end = datetime.now(UTC)
    start = end - timedelta(days=days)
    previous_start = start - (end - start)
    previous_end = start
    return {
        "overview.users_facts": pa.users_facts_stmt(start, end, previous_start, previous_end),
        "overview.orders_aggregate": pa.orders_aggregate_stmt(start, end, previous_start, previous_end),
        "overview.orders_daily": pa.orders_daily_stmt(start, end, dialect),
        "overview.trades_aggregate": pa.trades_aggregate_stmt(start, end, previous_start, previous_end),
        "overview.trades_daily": pa.trades_daily_stmt(start, end, dialect),
        "overview.login_day_facts": pa.login_day_facts_stmt(start, end, previous_start, previous_end),
        "overview.status_transition_facts": pa.status_transition_facts_stmt(
            start, end, previous_start, previous_end
        ),
        "overview.org_activity_buckets": pa.org_activity_buckets_stmt(
            start, end, previous_start, previous_end, dialect
        ),
        "overview.drop_off": pa.drop_off_stmt(),
        "marketplace.eligible_quotes": pa.eligible_quotes_stmt(end),
        "marketplace.matrix_window": pa.matrix_window_stmt(start, end),
        "marketplace.balance_trend": pa.balance_trend_stmt(start, end, dialect),
        "marketplace.status_distribution": pa.status_distribution_stmt(start, end),
        "marketplace.commissions": pa.commissions_stmt(start, end, previous_start, previous_end),
        "marketplace.time_to_fill": pa.time_to_fill_stmt(start, end),
    }


async def run(days: int, output: Path) -> int:
    database_url = settings.DATABASE_URL
    if not database_url.startswith("postgresql"):
        print("EXPLAIN report requires a PostgreSQL DATABASE_URL", file=sys.stderr)
        return 2

    engine = create_async_engine(database_url, hide_parameters=True)
    report: dict[str, object] = {
        "generated_at": datetime.now(UTC).isoformat(),
        "window_days": days,
        "statements": {},
    }
    try:
        async with engine.connect() as conn:
            await conn.execute(text("SET TRANSACTION READ ONLY"))
            for name, stmt in _statements(days, "postgresql").items():
                # Literal-rendered SQL keeps EXPLAIN free of prepared-statement
                # parameter plumbing; every literal is redacted from the
                # report before it is written.
                compiled = stmt.compile(
                    dialect=engine.dialect,
                    compile_kwargs={"literal_binds": True, "render_postcompile": True},
                )
                explain_sql = "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) " + str(compiled)
                result = await conn.exec_driver_sql(explain_sql)
                plan = result.scalar()
                report["statements"][name] = json.loads(_redact(json.dumps(plan)))
            # Never commit: the transaction is read-only and rolled back.
            await conn.rollback()
    finally:
        await engine.dispose()

    output.write_text(json.dumps(report, indent=2, sort_keys=True))
    print(f"wrote {output} ({len(report['statements'])} plans)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--days", type=int, default=90)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    return asyncio.run(run(args.days, args.output))


if __name__ == "__main__":
    sys.exit(main())
