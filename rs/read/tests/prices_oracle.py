"""Task 9 fixtures through current guards; historical quarantine belongs to 4b."""
import asyncio
import importlib.util
import os
import sys
from datetime import datetime, timedelta
from pathlib import Path
from uuid import NAMESPACE_URL, uuid5

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from sqlalchemy import text
from sqlalchemy.exc import DBAPIError

from app.market_catalog import CANONICAL_DELIVERY_POINTS, CANONICAL_PRODUCTS
from app.seeds.safety import seed_session
from scripts.rust_contract.record import differences, record

spec = importlib.util.spec_from_file_location("market_oracle", Path(__file__).with_name("market_contract_oracle.py"))
oracle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(oracle)
clock = datetime.fromisoformat(os.environ["RUST_CONTRACT_CLOCK"])
product = next(p for p in CANONICAL_PRODUCTS if p.execution_mode == "ORDERBOOK" and p.fuel_type == "Methanol")
point = next(p for p in CANONICAL_DELIVERY_POINTS if p.name == "Singapore")
uid = lambda name: uuid5(NAMESPACE_URL, "task9:" + name)

async def seed():
    async with seed_session() as session:
        for name in ("buyer", "seller"):
            await session.execute(text(oracle.INSERT_ORGANIZATION), dict(id=uid(name), name="Task 9 " + name, verification_status="APPROVED", provenance="REAL"))
        base = dict(product_id=product.id, product_name=product.name, fuel_type=product.fuel_type, fuel_grade=product.fuel_grade, market_product=product.market_product.value, delivery_point_id=point.id, delivery_point_name=point.name, delivery_point_region=point.region, availability_window="SPOT", buyer_id=uid("buyer"), seller_id=uid("seller"), buyer_provenance="REAL", seller_provenance="REAL", initiated_by="BUYER", initiator_org_id=uid("buyer"), quantity_mt="2.00", price_per_mt_usd="12.50", status="CONFIRMED", confirmed_at=clock, market_snapshot_version=1, delivered_at=None, paid_at=None, final_quantity_mt=None, final_price_per_mt=None, final_total_usd=None)
        fixtures = [
            ("latest", timedelta(seconds=1), "2.00", "12.50"),
            ("average", timedelta(hours=1), "9.000", "14.51"),
            ("hour-equality", timedelta(hours=24), "3.00", "18.00"),
            ("hour-outside", timedelta(hours=24, microseconds=1), "4.00", "21.01"),
            ("week-equality", timedelta(days=7), "5.00", "10.01"),
            ("week-outside", timedelta(days=7, microseconds=1), "6.00", "11.01"),
        ]
        for name, age, quantity, price in fixtures:
            await session.execute(text(oracle.INSERT_TRADE), {**base, "id": uid(name), "confirmed_at": clock-age, "quantity_mt": quantity, "price_per_mt_usd": price})
        for name, overrides in [("unknown", dict(buyer_provenance="UNKNOWN")), ("mixed", dict(seller_provenance="DEMO")), ("legacy",dict(market_snapshot_version=0))]:
            try:
                async with session.begin_nested():
                    await session.execute(text(oracle.INSERT_TRADE), {**base, "id":uid(name), **overrides})
            except DBAPIError:
                pass
            else:
                raise AssertionError("current guard unexpectedly accepted " + name)
        await session.commit()


def compare_populated():
    cases = [dict(id="fixture-"+str(i),method="GET",path=path,query=query) for i,(path,query) in enumerate([
        ("/api/prices", ""), ("/api/prices", "hours=168"),
        ("/api/prices/reference", "visibility=internal"),
        ("/api/prices/reference", "date_from=2026-10-15&date_to=2026-10-15"),
        ("/api/prices/reference/export", "format=other"),
        ("/api/trade-tape", ""), ("/api/trade-tape", "skip=1000000"),
        ("/api/trade-tape", "delivery_point_id="+str(point.id)),
        ("/api/trade-tape", "fuel_type="+product.fuel_type),
        ("/api/trade-tape", "fuel_type="+product.fuel_type.lower()),
        ("/api/trade-tape", "demo_only=true"),
    ])]
    # Keep each probe independent of earlier AD-01 counters.
    for case in cases:
        case["headers"]={"X-Forwarded-For":"198.51.100.91"}
    expected=record(os.environ["PYTHON_CONTRACT_URL"],os.environ["DISPOSABLE_TEST_TOKEN"],cases)
    actual=record(os.environ["RUST_CONTRACT_URL"],os.environ["DISPOSABLE_TEST_TOKEN"],cases)
    problems=differences(expected,actual)
    assert not problems, problems
    print(f"Task 9 populated probes matched: {len(cases)}; current guards refused UNKNOWN/mixed/version-0")

async def compare_shared_relations():
    import re

    from sqlalchemy import select

    from app.models.orderbook import Trade
    from app.services.market_provenance import formal_trade_snapshot_clause
    async with seed_session() as session:
        core = (ROOT / "rs/read/sql/formal_trade_snapshots.sql").read_text()
        metadata = (ROOT / "rs/read/sql/price_quarantine.sql").read_text()
        query = core + metadata
        query = re.sub(r"\$(\d+)", r":p\1", query)
        # SQLAlchemy's text parser needs whitespace before PostgreSQL casts.
        query = re.sub(r"(:p\d+)::", r"\1 ::", query)
        parameters = dict(p1=(ROOT / "deploy/rust/market-policy.json").read_text(), p2=None, p3=None, p4=None, p5=None, p6=None)
        rows = (await session.execute(text(query), parameters)).mappings().all()
        assert not rows, "current guards must leave quarantine empty"
        assert "price_per_mt_usd" not in metadata and "quantity_mt" not in metadata
        formal = re.sub(r"\$(\d+)", r":p\1", core + "SELECT id FROM formal_trades")
        formal = re.sub(r"(:p\d+)::", r"\1 ::", formal)
        actual = set((await session.execute(text(formal), parameters)).scalars())
        expected = set((await session.execute(select(Trade.id).where(formal_trade_snapshot_clause(Trade), Trade.status.in_(['CONFIRMED','DELIVERED','PAID']), Trade.confirmed_at.is_not(None)))).scalars())
        assert actual == expected
        print(f"Shared formal core compared: {len(actual)} rows; quarantine metadata: {len(rows)}")

asyncio.run(seed())
asyncio.run(compare_shared_relations())
compare_populated()
