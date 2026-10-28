#!/usr/bin/env python3
"""Idempotent daily prune for the ``user_login_days`` fact table.

Retains 800 UTC calendar dates — today plus the previous 799 — so a maximum
365-day analytics range and its equivalent previous period always remain
available with buffer (plan §2.4). Rows with
``activity_date < current_utc_date - 799`` are deleted.

``user_status_transitions`` is durable business history and is NEVER pruned.

Run by the verdaxis-product-analytics-prune systemd timer (see
deploy/systemd/); a nonzero exit leaves the oneshot unit failed for the
existing monitor to alert on.
"""

from __future__ import annotations

import argparse
import asyncio
import re
import sys
from datetime import UTC, date, datetime, timedelta
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from sqlalchemy import delete  # noqa: E402
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine  # noqa: E402

RETAINED_DATES = 800
_DEPLOYED_ENVIRONMENTS = ("production", "staging")
_FULL_RELEASE_SHA = re.compile(r"[0-9a-f]{40}")


def _release_sha(value: str) -> str:
    if _FULL_RELEASE_SHA.fullmatch(value) is None:
        raise argparse.ArgumentTypeError(
            "release SHA must be a full lowercase 40-hex commit SHA"
        )
    return value


def parse_cli_args(argv: list[str] | None = None) -> argparse.Namespace:
    """Require an explicit deployed identity for this destructive command."""
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--environment",
        choices=_DEPLOYED_ENVIRONMENTS,
        required=True,
    )
    parser.add_argument("--release-sha", type=_release_sha, required=True)
    return parser.parse_args(argv)


def assert_runtime_identity(
    *,
    configured_environment: str,
    configured_release_sha: str,
    expected_environment: str,
    expected_release_sha: str,
) -> None:
    """Bind a prune invocation to the validated deployed runtime config."""
    if expected_environment not in _DEPLOYED_ENVIRONMENTS:
        raise RuntimeError("prune target environment must be production or staging")
    if _FULL_RELEASE_SHA.fullmatch(expected_release_sha) is None:
        raise RuntimeError("prune target release must be a full commit SHA")
    if configured_environment != expected_environment:
        raise RuntimeError("runtime environment does not match prune target")
    if configured_release_sha != expected_release_sha:
        raise RuntimeError("runtime release does not match prune target")


def compute_cutoff(today: date) -> date:
    """First retained date is ``today - 799``; anything older is deleted."""
    return today - timedelta(days=RETAINED_DATES - 1)


async def prune_login_days(session, *, today: date | None = None) -> int:
    """Delete login-day rows older than the retention window. Idempotent."""
    from app.models.product_analytics import UserLoginDay

    cutoff = compute_cutoff(today or datetime.now(UTC).date())
    result = await session.execute(
        delete(UserLoginDay).where(UserLoginDay.activity_date < cutoff)
    )
    await session.commit()
    return result.rowcount or 0


async def main(*, expected_environment: str, expected_release_sha: str) -> int:
    from app.config import settings

    assert_runtime_identity(
        configured_environment=settings.ENVIRONMENT,
        configured_release_sha=settings.RELEASE_SHA,
        expected_environment=expected_environment,
        expected_release_sha=expected_release_sha,
    )
    engine = create_async_engine(settings.DATABASE_URL, hide_parameters=True)
    try:
        factory = async_sessionmaker(engine)
        async with factory() as session:
            deleted = await prune_login_days(session)
        print(f"pruned {deleted} login-day rows older than {RETAINED_DATES} dates")
        return 0
    finally:
        await engine.dispose()


def cli(argv: list[str] | None = None) -> int:
    args = parse_cli_args(argv)
    return asyncio.run(
        main(
            expected_environment=args.environment,
            expected_release_sha=args.release_sha,
        )
    )


if __name__ == "__main__":
    sys.exit(cli())
