"""Add the disposable corpus fixtures and attest the write target.

This runs only against an attested numeric-loopback `_test` database with the
non-superuser fixture writer. Corpus mode replaces the ordinary seed. Historical
rows are inserted at the legal pre-guard revision and upgraded normally. Fresh
rows use the existing market matrix; guard refusals are evidence, not bypasses.
"""

from __future__ import annotations

import argparse
import asyncio
import os
import sys
import json
import importlib.util
from pathlib import Path
from datetime import datetime
from uuid import uuid5, NAMESPACE_URL
from ipaddress import ip_address

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
        "registered TEST and UNKNOWN rows; CANARY guard refusal",
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
        "REAL/REAL and DEMO/DEMO snapshots; pre-guard quarantined history",
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
    host: str = "",
) -> None:
    """Refuse anything that is not an attested disposable test database."""
    if environment != "test":
        raise CorpusSeedError("the corpus seed requires ENVIRONMENT=test")
    if (
        not expected_database.endswith("_test")
        or expected_database != connected_database
    ):
        raise CorpusSeedError("the corpus seed requires the attested _test database")
    try:
        local = ip_address(host).is_loopback
    except ValueError:
        local = False
    if not local:
        raise CorpusSeedError("numeric loopback is required")
    if connected_user != "rust_owner":
        raise CorpusSeedError("the corpus seed requires a named fixture writer")
    if is_superuser:
        raise CorpusSeedError("the corpus seed writer must not be a superuser")


EXPECTED_COUNTS = {
    "ports": 3,
    "port_intelligence": 3,
    "news": 6,
    "market_orders": 46,
    "market_trades": 6,
    "small_decimal_orders": 4,
    "signals": 2,
    "ingestion_runs": 1,
    "legacy_trades": 2,
    "legacy_orders": 1,
}
FAMILY_CHECKS = {
    "ports_intelligence_news": ("ports", "port_intelligence", "news"),
    "admitted_real_owners": ("market_orders",),
    "demo_ownerless_with_expiry": ("market_orders",),
    "test_canary_unknown_rows": ("market_orders",),
    "rejected_moved_unverified_owners": ("market_orders",),
    "kyc_grace_boundary": ("market_orders",),
    "private_fame_lng_terms": ("small_decimal_orders",),
    "real_demo_mixed_trades": ("market_trades", "legacy_trades"),
    "missing_benchmark": ("market_orders",),
    "trusted_untrusted_signals": ("signals", "ingestion_runs"),
}
PRE_GUARD_REVISION = "miq_20260720_market_quarantine"


def fid(name):
    return uuid5(NAMESPACE_URL, "rust-corpus-4b:" + name)


def validate_counts(counts):
    if counts != EXPECTED_COUNTS:
        raise CorpusSeedError(
            f"fixture counts differ: {counts!r}; expected {EXPECTED_COUNTS!r}"
        )


def matrix_module():
    path = (
        Path(__file__).resolve().parents[2] / "rs/read/tests/market_contract_oracle.py"
    )
    spec = importlib.util.spec_from_file_location("corpus_market_matrix", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


async def attest_session(session):
    from sqlalchemy import text
    from sqlalchemy.engine import make_url

    url = make_url(os.environ["SEED_DATABASE_URL"])
    # Check before any fixture writes; the CLI also checks before connecting.
    row = (await session.execute(text(ATTESTATION_SQL))).one()
    attest_target(
        environment=os.environ.get("ENVIRONMENT", ""),
        expected_database=os.environ.get("SEED_TARGET_DATABASE", ""),
        connected_database=row[0],
        connected_user=row[1],
        is_superuser=row[2],
        host=url.host,
    )


async def seed_legacy(session):
    """Insert legal parent-schema history; the real migration assigns its version."""
    from sqlalchemy import text

    await attest_session(session)
    revision = await session.scalar(text("SELECT version_num FROM alembic_version"))
    if revision != PRE_GUARD_REVISION:
        raise CorpusSeedError("legacy fixtures require the exact pre-guard revision")
    now = datetime.fromisoformat(os.environ["RUST_CONTRACT_CLOCK"])
    for name in ("legacy_buyer", "legacy_seller"):
        await session.execute(
            text(
                "INSERT INTO organizations (id,name,type,verification_status) VALUES (:id,:name,'FUEL_SUPPLIER','PENDING')"
            ),
            {"id": fid(name), "name": name},
        )
    from app.demo_identities import DEMO_SEED_SUPPLIERS

    demo_seller = DEMO_SEED_SUPPLIERS[0][0]
    await session.execute(
        text(
            "INSERT INTO organizations (id,name,type,verification_status) VALUES (:id,'Corpus legacy demo','FUEL_SUPPLIER','APPROVED') ON CONFLICT DO NOTHING"
        ),
        {"id": demo_seller},
    )
    from app.market_catalog import PRODUCT_IDS, CANONICAL_DELIVERY_POINTS

    point = next(p for p in CANONICAL_DELIVERY_POINTS if p.name == "Singapore")
    await session.execute(
        text(
            "INSERT INTO delivery_points (id,name,region,timezone,is_active) VALUES (:id,'Singapore','Asia','Asia/Singapore',true) ON CONFLICT DO NOTHING"
        ),
        {"id": point.id},
    )
    await session.execute(
        text("""INSERT INTO orderbook_orders
        (id,organization_id,side,product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,
         price_per_mt_usd,availability_window,status,created_at,updated_at)
        VALUES (:id,:org,'ASK',:product,:point,100,100,600,'SPOT','OPEN',:now,:now)"""),
        {
            "id": fid("legacy_order"),
            "org": fid("legacy_seller"),
            "product": PRODUCT_IDS["BIO_METHANOL"],
            "point": point.id,
            "now": now,
        },
    )
    for name, order in (
        ("legacy_unlinked", None),
        ("legacy_quarantine", fid("legacy_order")),
    ):
        await session.execute(
            text("""INSERT INTO trades
            (id,buyer_id,seller_id,ask_order_id,initiated_by,quantity_mt,price_per_mt_usd,
             status,confirmed_at,commission_rate_pct,created_at)
            VALUES (:id,:buyer,:seller,:order,'BUYER',10,600,'CONFIRMED',:now,0,:now)"""),
            {
                "id": fid(name),
                "buyer": fid("legacy_buyer"),
                "seller": demo_seller
                if name == "legacy_quarantine"
                else fid("legacy_seller"),
                "order": order,
                "now": now,
            },
        )


async def seed_corpus(session):
    """Build deterministic fixtures, retaining all database guards."""
    from sqlalchemy import text, select, func
    from app.models.port import Port, PortIntelligence
    from app.models.news import NewsItem
    from app.models.orderbook import OrderBookOrder, Trade
    from app.models.forward_monitoring import MarketIndication, MarketSignalIngestionRun
    from app.market_catalog import CANONICAL_DELIVERY_POINTS
    from app.schemas.fame_order import FameAskTerms
    from app.schemas.lng_order import LngAskTerms

    await attest_session(session)
    now = datetime.fromisoformat(os.environ["RUST_CONTRACT_CLOCK"])
    matrix = matrix_module()
    os.environ["RUST_MARKET_EXPECTED_DATABASE"] = os.environ["SEED_TARGET_DATABASE"]
    guards = await matrix.seed(session, now)
    # Keep the intentional malformed declaration out of positive collections.
    await session.execute(
        text("UPDATE orderbook_orders SET status='CANCELLED' WHERE id=:id"),
        {"id": matrix.fid("order:typed_invalid_visible")},
    )
    # Stable timestamps prevent wall-clock sort order and response drift.
    await session.execute(
        text("UPDATE orderbook_orders SET created_at=:now, updated_at=:now"),
        {"now": now},
    )
    for name in ("BARE", "ONE", "TWO"):
        session.add(
            Port(
                id="T4B_" + name,
                name="Corpus " + name,
                country="Singapore",
                location="SRID=4326;POINT(103.123456789123 1.234567891234)",
                timezone="Asia/Singapore",
                is_active=True,
            )
        )
    await session.flush()
    for index, port in enumerate(("ONE", "TWO", "TWO")):
        session.add(
            PortIntelligence(
                id=fid(f"intelligence:{index}"),
                port_id="T4B_" + port,
                congestion_level=("Low" if index < 2 else "High"),
                methanol_price_avg=600 + index,
                biofuel_price_avg=None,
                captured_at=now,
            )
        )
        await session.flush()  # fixed insertion order for the duplicate relationship
    for category in (
        "bunkers",
        "carbon",
        "commodities",
        "markets",
        "regulation",
        "shipping",
    ):
        session.add(
            NewsItem(
                id=fid("news:" + category),
                title="Corpus " + category,
                summary=None,
                source="Corpus",
                source_url="https://example.test",
                url="https://example.test/" + category,
                category=category,
                relevance=5,
                published_at=now,
                fetched_at=now,
            )
        )
        await session.flush()
    for profile, source, model in (
        ("fame", "fame_ask_ok", FameAskTerms),
        ("lng", "lng_ask_ok", LngAskTerms),
    ):
        original = await session.get(OrderBookOrder, matrix.fid("order:" + source))
        for index, value in enumerate(("1E-29", "0E-100")):
            terms = dict(getattr(original, profile + "_terms"))
            terms["cfpp_c" if profile == "fame" else "min_methane_number"] = value
            terms = model.model_validate(terms).model_dump(mode="json")
            columns = {
                column.name: getattr(original, column.name)
                for column in OrderBookOrder.__table__.columns
            }
            columns.update(
                id=fid(f"small:{profile}:{index}"),
                acceptance_ordinal=None,
                created_at=now,
                updated_at=now,
            )
            columns[profile + "_terms"] = terms
            session.add(OrderBookOrder(**columns))
    point = next(p.id for p in CANONICAL_DELIVERY_POINTS if p.name == "Singapore")
    run = MarketSignalIngestionRun(
        id=fid("ingestion"),
        signal_family="MARKET_INDICATION",
        source="CORPUS",
        source_kind="MARKET_INDICATION",
        started_at=now,
        verified_at=now,
        created_at=now,
    )
    session.add(run)
    await session.flush()
    for trusted in (True, False):
        session.add(
            MarketIndication(
                id=fid(f"signal:{trusted}"),
                market_product="BIO_METHANOL",
                delivery_point_id=point,
                availability_window="SPOT",
                side="ASK",
                price_per_mt_usd=610,
                source="CORPUS",
                trusted_ingestion_run_id=run.id if trusted else None,
                is_demo=False,
                is_verified_real=trusted,
                verified_real_at=now if trusted else None,
                observed_at=now,
                created_at=now,
            )
        )
    await session.flush()
    counts = {
        "ports": await session.scalar(
            select(func.count()).select_from(Port).where(Port.id.like("T4B_%"))
        ),
        "port_intelligence": await session.scalar(
            select(func.count())
            .select_from(PortIntelligence)
            .where(PortIntelligence.port_id.like("T4B_%"))
        ),
        "news": await session.scalar(select(func.count()).select_from(NewsItem)),
        "market_orders": (
            await session.scalar(select(func.count()).select_from(OrderBookOrder))
        )
        - 5,
        "market_trades": (await session.scalar(select(func.count()).select_from(Trade)))
        - 2,
        "small_decimal_orders": await session.scalar(
            select(func.count())
            .select_from(OrderBookOrder)
            .where(
                OrderBookOrder.id.in_(
                    [fid(f"small:{p}:{i}") for p in ("fame", "lng") for i in range(2)]
                )
            )
        ),
        "signals": await session.scalar(
            select(func.count()).select_from(MarketIndication)
        ),
        "ingestion_runs": await session.scalar(
            select(func.count()).select_from(MarketSignalIngestionRun)
        ),
        "legacy_trades": await session.scalar(
            select(func.count())
            .select_from(Trade)
            .where(Trade.id.in_([fid("legacy_unlinked"), fid("legacy_quarantine")]))
        ),
        "legacy_orders": await session.scalar(
            select(func.count())
            .select_from(OrderBookOrder)
            .where(OrderBookOrder.id == fid("legacy_order"))
        ),
    }
    validate_counts(counts)
    for name in (
        "unknown_ownerless_bid",
        "inactive_product_bid",
        "inactive_point_bid",
        "demo_ownerless_bid",
        "test_ownerless_bid",
        "rejected_owner_bid",
        "moved_owner_bid",
        "unverified_owner_bid",
        "grace_edge_bid",
    ):
        if await session.get(OrderBookOrder, matrix.fid("order:" + name)) is None:
            raise CorpusSeedError(f"missing named fixture {name}")
    if not guards["canary_org"].get("guard_refused"):
        raise CorpusSeedError("CANARY guard refusal was not recorded")
    from app.services.market_provenance import public_trade_evidence_clause

    legacy_ids = [fid("legacy_unlinked"), fid("legacy_quarantine")]
    if await session.scalar(
        select(func.count())
        .select_from(Trade)
        .where(Trade.id.in_(legacy_ids), public_trade_evidence_clause(Trade))
    ):
        raise CorpusSeedError("legacy history leaked into economics")
    legacy = (
        await session.execute(
            select(
                Trade.id,
                Trade.market_snapshot_version,
                Trade.buyer_provenance,
                Trade.seller_provenance,
            ).where(Trade.id.in_([fid("legacy_unlinked"), fid("legacy_quarantine")]))
        )
    ).all()
    if any(row.market_snapshot_version is not None for row in legacy):
        raise CorpusSeedError("legacy history unexpectedly became formal evidence")
    # Bio-LNG has no benchmark or demo price. Empty benchmark table is deliberate.
    if await session.scalar(text("SELECT count(*) FROM benchmarks")):
        raise CorpusSeedError("missing-benchmark fixture is not empty")
    manifest = {
        "counts": counts,
        "guards": guards,
        "legacy_snapshot_version": None,
        "legacy_note": "pre-guard history migrates to SQL NULL, never literal version 0",
    }
    output = os.environ.get("RUST_CORPUS_MANIFEST")
    if output:
        Path(output).write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest


async def _main() -> None:
    from sqlalchemy.engine import make_url

    url = make_url(os.environ["SEED_DATABASE_URL"])
    attest_target(
        environment=os.environ.get("ENVIRONMENT", ""),
        expected_database=os.environ.get("SEED_TARGET_DATABASE", ""),
        connected_database=url.database,
        connected_user=url.username,
        is_superuser=False,
        host=url.host,
    )
    async with seed_session() as session:
        if os.environ.get("RUST_CORPUS_STAGE") == "legacy":
            await seed_legacy(session)
        else:
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
