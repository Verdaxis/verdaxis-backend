"""Compare legacy scope on the existing Task 6 matrix, never a live database."""
import asyncio
import importlib.util
import os
import sys
from datetime import datetime
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))

from sqlalchemy import text

from app.seeds.safety import seed_session
from scripts.rust_contract.record import differences, record


async def main():
    spec = importlib.util.spec_from_file_location(
        "market_oracle", Path(__file__).with_name("market_contract_oracle.py")
    )
    oracle = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(oracle)
    os.environ["RUST_MARKET_EXPECTED_DATABASE"] = os.environ["SEED_TARGET_DATABASE"]
    async with seed_session() as session:
        await oracle.seed(session, datetime.fromisoformat(os.environ["RUST_CONTRACT_CLOCK"]))
        await session.commit()
        products = (await session.execute(text(
            "SELECT DISTINCT product_id::text FROM orderbook_orders ORDER BY 1"
        ))).scalars().all()
        inactive = (await session.execute(text(
            "SELECT o.product_id::text FROM orderbook_orders o JOIN products p ON p.id=o.product_id "
            "WHERE o.id=:id AND NOT p.is_active AND o.provenance='REAL'"
        ), {"id": oracle.fid("order:inactive_product_bid")})).scalar_one()
    cases = [dict(id=f"matrix-{index}-{kind}", method="GET",
                  path=f"/api/curves/forward{suffix}", query=f"product_id={product}")
             for index, product in enumerate(products)
             for kind, suffix in [("json", ""), ("csv", "/export")]]
    token = os.environ["DISPOSABLE_TEST_TOKEN"]
    expected = record(os.environ["PYTHON_CONTRACT_URL"], token, cases)
    actual = record(os.environ["RUST_CONTRACT_URL"], token, cases)
    problems = differences(expected, actual)
    assert not problems, problems
    # A separate HTTP assertion ensures an empty result cannot pass the inactive regression.
    import json
    import urllib.request
    with urllib.request.urlopen(os.environ["RUST_CONTRACT_URL"] +
                                "/api/curves/forward?product_id=" + inactive) as response:
        body = json.load(response)
    assert sum(point["order_count"] for point in body["curve"]) >= 1, body
    print(f"legacy matrix: {len(cases)} JSON/CSV comparisons; inactive REAL input retained")


asyncio.run(main())
