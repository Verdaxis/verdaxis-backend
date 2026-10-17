"""Seed the Task 14a bulk workload at a declared size.

The seeded differential simulation and the latency/memory measurement run the
public routes over a representative and a large dataset. This module adds those
rows on top of the recorded corpus fixtures, through the attested fixture
writer only, and never disables a database guard or trigger.

Determinism: every added id is derived from the size label and the manifest's
distribution seed, and every timestamp is derived from the frozen corpus clock,
so Python and Rust read exactly the same database and a repetition replays it.

Evidence classes: bulk orders are disclosed REAL (owned) and DEMO (ownerless)
rows. Bulk orders use the scalar fuels only -- the typed FAME and LNG products
keep their corpus fixtures, because a bulk copy of a scalar row would carry the
wrong typed declaration. No LNG demo or benchmark price is seeded.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
from datetime import datetime, timedelta
from uuid import UUID

from scripts.rust_contract import workload
from scripts.rust_contract.seed_corpus import attest_session, matrix_module

# Bulk rows use only the scalar fuels, so no copied scalar declaration lands on
# a typed product. The typed products keep their corpus fixtures.
TYPED_PROFILES = frozenset({"fame_v1", "lng_v1"})

SEED_NAMESPACE = "rust-workload-14a"


def _policy_scalar_products() -> list[UUID]:
    from pathlib import Path

    policy = json.loads(Path("deploy/rust/market-policy.json").read_text())
    products = [
        UUID(p["id"]) for p in policy["products"]
        if p["qualification_profile"] not in TYPED_PROFILES
    ]
    if not products:
        raise RuntimeError("the policy carries no scalar product")
    return products


async def seed_workload(session, size: str, counts: dict[str, int], seed: int) -> dict:
    """Add the bulk rows for one size. Returns a small, redacted summary."""
    from sqlalchemy import text

    await attest_session(session)
    matrix = matrix_module()
    products = _policy_scalar_products()
    clock = datetime.fromisoformat(os.environ["RUST_CONTRACT_CLOCK"])
    label = f"{SEED_NAMESPACE}:{size}:{seed}"

    # The two owned/ownerless fixture identities the corpus already created.
    real = (
        await session.execute(
            text(
                "SELECT organization_id, owner_user_id FROM orderbook_orders "
                "WHERE provenance='REAL' AND owner_user_id IS NOT NULL LIMIT 1"
            )
        )
    ).one()
    demo = (
        await session.execute(
            text(
                "SELECT organization_id FROM orderbook_orders "
                "WHERE provenance='DEMO' AND owner_user_id IS NULL LIMIT 1"
            )
        )
    ).one()
    point_ids = list((await session.execute(
        text("SELECT id FROM delivery_points WHERE is_active ORDER BY name"))).scalars())
    windows = ["SPOT", "2026-11", "2026-Q4", "2027-01", "2027-Q1", "2027-06"]
    categories = ["bunkers", "carbon", "commodities", "markets", "regulation", "shipping"]

    bid_template = matrix.fid("order:real_scalar_bid")
    ask_template = matrix.fid("order:ask_ok")

    # Orders: copy a scalar template row so every guard-required column is
    # present, then override identity, provenance, product, point, window.
    await session.execute(
        text(
            """
            INSERT INTO orderbook_orders (
              id, organization_id, owner_user_id, created_by_actor_user_id,
              creation_method, provenance, side, product_id, delivery_point_id,
              quantity_mt, remaining_quantity_mt, price_per_mt_usd,
              availability_window, status, expires_at, off_spec,
              certification_declared, certification_scheme, specification_standard,
              msds_available, carbon_intensity_gco2_mj, carbon_intensity_method,
              feedstock, origin, certifications, fame_terms, lng_terms,
              created_at, updated_at)
            SELECT
              md5(:label || ':o:' || n::text)::uuid,
              CASE WHEN n % 10 = 0 THEN CAST(:demo_org AS uuid) ELSE CAST(:real_org AS uuid) END,
              CASE WHEN n % 10 = 0 THEN NULL ELSE CAST(:owner AS uuid) END,
              NULL, 'SELF_SERVICE',
              CASE WHEN n % 10 = 0 THEN 'DEMO' ELSE 'REAL' END,
              CASE WHEN n % 2 = 0 THEN 'BID' ELSE 'ASK' END,
              (CAST(:products AS uuid[]))[1 + (n % array_length(CAST(:products AS uuid[]), 1))],
              (CAST(:points AS uuid[]))[1 + (n % array_length(CAST(:points AS uuid[]), 1))],
              (100 + (n % 9900))::numeric(18,2),
              (100 + (n % 9900))::numeric(18,2),
              (500 + (n % 1500))::numeric(18,2),
              (CAST(:windows AS text[]))[1 + (n % array_length(CAST(:windows AS text[]), 1))],
              'OPEN',
              -- A DEMO OPEN row must carry an expiry (ck_orderbook_orders_domain).
              CASE WHEN n % 10 = 0 THEN CAST(:future AS timestamptz) ELSE NULL END,
              false,
              t.certification_declared, t.certification_scheme,
              t.specification_standard, t.msds_available,
              t.carbon_intensity_gco2_mj, t.carbon_intensity_method,
              t.feedstock, t.origin, t.certifications,
              t.fame_terms, t.lng_terms,
              CAST(:clock AS timestamptz), CAST(:clock AS timestamptz)
            FROM generate_series(1, :count) AS n
            LEFT JOIN orderbook_orders t ON t.id = CASE WHEN n % 2 = 0
              THEN CAST(:bid_template AS uuid) ELSE CAST(:ask_template AS uuid) END
            ON CONFLICT (id) DO NOTHING
            """
        ),
        {
            "label": label,
            "count": counts["orders"],
            "real_org": str(real[0]),
            "demo_org": str(demo[0]),
            "owner": str(real[1]),
            "products": products,
            "points": point_ids,
            "windows": windows,
            "bid_template": bid_template,
            "ask_template": ask_template,
            "clock": clock,
            # A future expiry for the DEMO rows, relative to the frozen clock.
            "future": clock + timedelta(days=30),
        },
    )

    # Trades: REAL/REAL and DEMO/DEMO templates, spread inside the trade-tape
    # seven-day window so the measured route is nonempty.
    await session.execute(
        text(
            """
            INSERT INTO trades (
              id, product_id, product_name, fuel_type, fuel_grade, market_product,
              delivery_point_id, delivery_point_name, delivery_point_region,
              availability_window, buyer_id, seller_id, buyer_provenance,
              seller_provenance, initiated_by, initiator_org_id, quantity_mt,
              price_per_mt_usd, status, confirmed_at, market_snapshot_version,
              delivered_at, paid_at, final_quantity_mt, final_price_per_mt,
              final_total_usd, created_at)
            SELECT
              md5(:label || ':t:' || n::text)::uuid,
              t.product_id, t.product_name, t.fuel_type, t.fuel_grade, t.market_product,
              t.delivery_point_id, t.delivery_point_name, t.delivery_point_region,
              (CAST(:windows AS text[]))[1 + (n % array_length(CAST(:windows AS text[]), 1))],
              t.buyer_id, t.seller_id, t.buyer_provenance, t.seller_provenance,
              t.initiated_by, t.initiator_org_id,
              (50 + (n % 500))::numeric(18,2), (400 + (n % 1200))::numeric(18,2),
              'CONFIRMED',
              CAST(:clock AS timestamptz) - make_interval(secs => (n % 604000)),
              1, NULL, NULL, NULL, NULL, NULL, CAST(:clock AS timestamptz)
            FROM generate_series(1, :count) AS n
            JOIN trades t ON t.id = CASE WHEN n % 8 = 0
              THEN CAST(:demo_template AS uuid) ELSE CAST(:real_template AS uuid) END
            ON CONFLICT (id) DO NOTHING
            """
        ),
        {
            "label": label,
            "count": counts["trades"],
            "clock": clock,
            "windows": windows,
            "real_template": matrix.fid("trade:real_real"),
            "demo_template": matrix.fid("trade:demo_demo"),
        },
    )

    # News: rotate the six categories, spread published times across the corpus
    # day so the tie-group rule is exercised, keep the optional fields null.
    await session.execute(
        text(
            """
            INSERT INTO news_items (
              id, title, summary, source, source_url, url, category, relevance,
              published_at, fetched_at)
            SELECT
              md5(:label || ':n:' || n::text)::uuid,
              'Workload news ' || n, NULL, 'Workload', 'https://example.test',
              'https://example.test/workload/' || :label || '/' || n,
              (CAST(:categories AS text[]))[1 + (n % array_length(CAST(:categories AS text[]), 1))],
              5,
              CAST(:clock AS timestamptz) - make_interval(secs => (n % 86000)),
              CAST(:clock AS timestamptz)
            FROM generate_series(1, :count) AS n
            ON CONFLICT (url) DO NOTHING
            """
        ),
        {
            "label": label,
            "count": counts["news"],
            "clock": clock,
            "categories": categories,
        },
    )
    await session.commit()

    return {
        "size": size,
        "counts": counts,
        "seed": seed,
        "label": label,
        "scalar_products": len(products),
        "delivery_points": len(point_ids),
        "identification": "ids are md5(label||':o:'||n)::uuid",
    }


async def _main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--size", required=True, choices=sorted(workload.DECLARED_SIZES))
    parser.add_argument("--manifest", default=str(workload.MANIFEST_PATH))
    args = parser.parse_args()
    manifest = workload.load(workload.Path(args.manifest))
    counts = workload.sizes(manifest)[args.size]
    from app.seeds.safety import seed_session

    async with seed_session() as session:
        summary = await seed_workload(session, args.size, counts, manifest["distribution_seed"])
    print(json.dumps(summary))


def main(argv: list[str] | None = None) -> int:
    try:
        asyncio.run(_main())
    except Exception as error:  # noqa: BLE001 - a seeding failure must exit non-zero
        import traceback

        print(f"workload seed failed: {error}", file=sys.stderr)
        traceback.print_exc()
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
