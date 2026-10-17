"""Seed the integration suite's deterministic identities for the mixed run.

The corpus seed builds the market fixtures but not the `itest-*` accounts the
unchanged integration suite logs in as, so the mixed Caddy run had no user to
authenticate. This adds them from the Phase 0 HTTP runner's own definitions,
inserted with REAL provenance -- the trusted route an administrator-invited
organization takes -- so market writes pass the provenance guard without a
privileged update or a `*_market_integrity_test` database name.

It runs only inside the disposable runner's own database, after
`scripts.rust_contract.seed_corpus` and the migrations.
"""

from __future__ import annotations

import asyncio
import os

from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from tests.phase0.http_seed import add_itest_identities


async def main() -> None:
    engine = create_async_engine(os.environ["DATABASE_URL"])
    try:
        async with async_sessionmaker(engine, expire_on_commit=False)() as session:
            await add_itest_identities(
                session,
                os.environ["DISPOSABLE_ITEST_PASSWORD"],
                real_provenance_on_insert=True,
            )
    finally:
        await engine.dispose()


if __name__ == "__main__":
    asyncio.run(main())
