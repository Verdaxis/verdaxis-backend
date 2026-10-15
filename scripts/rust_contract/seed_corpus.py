"""Add the disposable corpus fixtures and attest the write target.

This runs only against an attested disposable `_test` database with a
non-superuser writer. It never grants application writes and never runs on
staging. The ordinary seed (`scripts/seed.py`) runs first; this adds the fixed
fixtures the contract cases need.

The fixture families below are required by the recorded corpus. Each names the
trusted construction route it must use. Families that depend on the trusted
provenance path refuse to run until that path is wired by the route task that
consumes them, so a missing fixture fails closed instead of recording against
an unintended database state.
"""

from __future__ import annotations

import argparse
import asyncio
import os
import sys

from app.seeds.safety import seed_session

ATTESTATION_SQL = (
    "SELECT current_database(), current_user, rol.rolsuper "
    "FROM pg_catalog.pg_roles AS rol WHERE rol.rolname = current_user"
)

# Fixture family -> (target cases, trusted construction route).
CORPUS_FIXTURES: dict[str, tuple[tuple[str, ...], str]] = {
    "ports_intelligence_news": (
        ("R01-ports", "R02-port", "R05-news"),
        "catalog and market seeds, plus fixed news rows",
    ),
    "admitted_real_owners": (
        ("R16-bids", "R17-asks", "R15-snapshot"),
        "administrator organization approval, then owner users",
    ),
    "demo_ownerless_with_expiry": (
        ("R15-snapshot", "R16-bids"),
        "ownerless DEMO orders with a future expiry",
    ),
    "test_canary_unknown_rows": (
        ("R16-bids", "R17-asks", "R19-aggregated"),
        "orders carrying TEST, CANARY, and UNKNOWN provenance",
    ),
    "rejected_moved_unverified_owners": (
        ("R16-bids", "R17-asks"),
        "owner accounts in rejected, moved, and unverified states",
    ),
    "kyc_grace_boundary": (
        ("R16-bids", "R17-asks"),
        "organization-bound grace before, at, and after expiry",
    ),
    "private_fame_lng_terms": (
        ("R15-snapshot", "R16-bids"),
        "synthetic REAL-class FAME and LNG orders; no LNG benchmark",
    ),
    "real_demo_mixed_trades": (
        ("R06-trade-tape", "R13-reference"),
        "REAL/REAL and DEMO/DEMO snapshots plus one mismatched pair",
    ),
    "missing_benchmark": (
        ("R19-aggregated", "R11-board"),
        "a slice with no qualifying benchmark",
    ),
    "trusted_untrusted_signals": (
        ("R05-news",),
        "trusted and untrusted signal rows",
    ),
}


class CorpusSeedError(RuntimeError):
    """The corpus seed target or fixture state is not acceptable."""


def attest_target(
    *,
    environment: str,
    expected_database: str,
    connected_database: str,
    connected_user: str,
    is_superuser: bool,
) -> None:
    """Refuse anything that is not an attested disposable test database."""
    if environment != "test":
        raise CorpusSeedError("the corpus seed requires ENVIRONMENT=test")
    if not expected_database.endswith("_test") or expected_database != connected_database:
        raise CorpusSeedError("the corpus seed requires the attested _test database")
    if not connected_user:
        raise CorpusSeedError("the corpus seed requires a named fixture writer")
    if is_superuser:
        raise CorpusSeedError("the corpus seed writer must not be a superuser")


async def seed_corpus(session) -> None:
    """Add every corpus fixture family. Fails closed on a missing family."""
    from sqlalchemy import text

    row = (await session.execute(text(ATTESTATION_SQL))).one()
    attest_target(
        environment=os.environ.get("ENVIRONMENT", ""),
        expected_database=os.environ.get("SEED_TARGET_DATABASE", ""),
        connected_database=row[0],
        connected_user=row[1],
        is_superuser=row[2],
    )
    unimplemented = sorted(set(CORPUS_FIXTURES))
    raise CorpusSeedError(
        "corpus fixture families are not yet constructed by this task: "
        + ", ".join(unimplemented)
        + "; they land with the route tasks that consume them"
    )


async def _main() -> None:
    async with seed_session() as session:
        await seed_corpus(session)
        await session.commit()


def main(argv: list[str] | None = None) -> int:
    argparse.ArgumentParser(description=__doc__).parse_args(argv)
    try:
        asyncio.run(_main())
    except CorpusSeedError as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
