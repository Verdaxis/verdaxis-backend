"""Task-local migrated fixture comparison, invoked only by the disposable group."""
import asyncio
import importlib.util
import os
from datetime import datetime
from pathlib import Path

from sqlalchemy import text

from app.seeds.safety import seed_session
from scripts.rust_contract.record import differences, record


async def fixtures():
    path = Path("rs/read/tests/market_contract_oracle.py")
    spec = importlib.util.spec_from_file_location("market_oracle", path)
    oracle = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(oracle)
    os.environ["RUST_MARKET_EXPECTED_DATABASE"] = os.environ["SEED_TARGET_DATABASE"]
    async with seed_session() as session:
        cases = await oracle.seed(session, datetime.fromisoformat(os.environ["RUST_CONTRACT_CLOCK"]))
        assert cases["zero_remaining"]["guard_refused"]
        # Give the two provenance classes different weights and prices.
        for name, quantity, price in [("real_scalar_bid", 10, 100), ("demo_ownerless_bid", 30, 200)]:
            await session.execute(text("UPDATE orderbook_orders SET quantity_mt=:quantity, remaining_quantity_mt=:quantity, price_per_mt_usd=:price WHERE id=:id"),
                                  dict(quantity=quantity, price=price, id=oracle.fid(f"order:{name}")))
        # A shallow-visible declaration with an invalid required LNG field.
        await session.execute(text("UPDATE orderbook_orders SET lng_terms = jsonb_set(lng_terms::jsonb, '{production_country}', '\"ZZ\"')::json WHERE id=:id"),
                              dict(id=oracle.fid("order:lng_ask_ok")))
        from sqlalchemy import select
        from sqlalchemy.orm import joinedload

        from app.models.orderbook import OrderBookOrder

        # These are shallow-visible but fail full benchmark qualification.
        # Keep this proof separate from the final aggregate comparison.
        from app.services.live_benchmarks import public_slice_order_qualified
        from app.services.public_order_scope import public_order_scope
        instant = datetime.fromisoformat(os.environ["RUST_CONTRACT_CLOCK"])
        filters, joins = public_order_scope(as_of=instant)
        for name in ("typed_invalid_visible", "lng_ask_ok"):
            statement = select(OrderBookOrder).options(joinedload(OrderBookOrder.product))
            for target, condition in joins:
                statement = statement.join(target, condition)
            row = (await session.execute(statement.where(
                *filters, OrderBookOrder.id == oracle.fid(f"order:{name}")
            ))).scalar_one()
            assert not public_slice_order_qualified(row, as_of=datetime.fromisoformat(os.environ["RUST_CONTRACT_CLOCK"]))
        await session.commit()


def main():
    asyncio.run(fixtures())
    cases = [dict(id="matrix-all", method="GET", path="/__contract__/live-benchmarks"),
             dict(id="matrix-sparse", method="GET", path="/__contract__/live-benchmarks?selection=sparse"),
             dict(id="matrix-expiry", method="GET", path="/__contract__/live-benchmarks?as_of=2026-10-16T12:00:00Z")]
    token = os.environ["DISPOSABLE_TEST_TOKEN"]
    expected = record(os.environ["PYTHON_CONTRACT_URL"], token, cases)
    actual = record(os.environ["RUST_CONTRACT_URL"], token, cases)
    assert any(row["price"] is not None for row in expected[0]["exchange"]["body"])
    problems = differences(expected, actual)
    assert not problems, problems
    print("live benchmark matrix: 3 cases matched")


if __name__ == "__main__":
    main()
