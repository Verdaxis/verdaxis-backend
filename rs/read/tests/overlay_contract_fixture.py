"""Private disposable overlay matrix; run after the market fixture seed.

After market_contract_oracle.py seeds the disposable market database, this adds
the unequal tenant vessel counts, clones public and hidden orders from the
accepted oracle fixtures, and writes the manifest phase5_compliance_overlay.rs
reads from RUST_OVERLAY_TEST_CASES. It refuses a database that already carries
vessels, so it cannot duplicate rows on an already-seeded market database.
"""
import asyncio
import json
import os
from pathlib import Path
from uuid import NAMESPACE_URL, uuid5

from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine


def fid(name):
    return str(uuid5(NAMESPACE_URL, f"rust-market-contract:{name}"))


async def main():
    engine = create_async_engine(os.environ["RUST_MARKET_WRITER_URL"])
    async with engine.begin() as connection:
        async def execute(sql, **values):
            return await connection.execute(text(sql), values)

        assert (await execute("SELECT count(*) FROM vessels")).scalar_one() == 0
        orgs = [fid(f"org:{name}") for name in ("real", "real_alt", "foreign_customer")]
        for org, count in [(orgs[0], 2), (orgs[1], 3), (None, 1)]:
            for number in range(count):
                identity = fid(f"overlay:vessel:{org}:{number}")
                await execute(
                    "INSERT INTO vessels (id, organization_id, name, imo_number) "
                    "VALUES (CAST(:id AS uuid), CAST(:org AS uuid), :name, :imo)",
                    id=identity, org=org, name=f"Overlay {identity}", imo=identity,
                )
        columns = (await execute(
            "SELECT column_name FROM information_schema.columns "
            "WHERE table_schema='public' AND table_name='orderbook_orders' AND column_name <> 'acceptance_ordinal'"
            "ORDER BY ordinal_position"
        )).scalars().all()
        visible = [fid("order:ask_ok"), fid("overlay:visible:second")]
        clones = [
            (visible[1], {"carbon_intensity_gco2_mj": "27.3", "energy_density_mj_kg": "21.5"}),
            (fid("overlay:hidden:owner"), {"owner_user_id": f"'{fid('user:rejected')}'::uuid"}),
            (fid("overlay:hidden:expiry"), {"expires_at": "'2026-10-14T12:00:00Z'::timestamptz"}),
            (fid("overlay:hidden:policy"), {"product_id": f"'{fid('product:alias')}'::uuid"}),
            (fid("overlay:hidden:bid"), {"side": "'BID'"}),
        ]
        for identity, replacements in clones:
            replacements = {"id": f"'{identity}'::uuid", **replacements}
            projection = ", ".join(replacements.get(column, f'"{column}"') for column in columns)
            names = ", ".join(f'"{column}"' for column in columns)
            await execute(
                f"INSERT INTO orderbook_orders ({names}) SELECT {projection} "
                "FROM orderbook_orders WHERE id=CAST(:source AS uuid)", source=visible[0],
            )
        counts = (await execute(
            "SELECT organization_id::text, count(*) FROM vessels GROUP BY organization_id"
        )).all()
        assert dict(counts) == {orgs[0]: 2, orgs[1]: 3, None: 1}
        manifest = {
            "visible_ids": visible,
            "hidden_ids": [identity for identity, _ in clones[1:]],
            "scopes": [
                {"name": name, "role": role, "organization_id": org, "count": count}
                for name, role, org, count in [
                    ("administrator", "ADMIN", None, 6),
                    ("member", "BUYER", orgs[0], 2),
                    ("other_member", "BUYER", orgs[1], 3),
                    ("empty_member", "BUYER", orgs[2], 0),
                    ("no_organization", "BUYER", None, 0),
                ]
            ],
        }
    await engine.dispose()
    Path(os.environ["RUST_OVERLAY_TEST_CASES"]).write_text(json.dumps(manifest, indent=2))
    print("OVERLAY_FIXTURE=PASS vessels=6 scopes=5 visible=2 hidden=4")


asyncio.run(main())
