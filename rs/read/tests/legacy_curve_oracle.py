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
from scripts.rust_contract.record import differences
from scripts.rust_contract.record import record as record_cases


def record(url, token, cases):
    # These test semantic parity, not admission load. Give Python's streamed
    # response cleanup time to release its permit between requests.
    import time
    results = []
    for case in cases:
        result = record_cases(url, token, [case])
        assert result[0]["exchange"]["status"] == 200, result
        results.extend(result)
        time.sleep(0.05)
    return results


async def main():
    spec = importlib.util.spec_from_file_location(
        "market_oracle", Path(__file__).with_name("market_contract_oracle.py")
    )
    oracle = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(oracle)
    os.environ["RUST_MARKET_EXPECTED_DATABASE"] = os.environ["SEED_TARGET_DATABASE"]
    async with seed_session() as session:
        present = await session.scalar(text("SELECT count(*) FROM orderbook_orders WHERE id=:id"),
                                       {"id": oracle.fid("order:real_scalar_bid")})
        if not present:
            await oracle.seed(session, datetime.fromisoformat(os.environ["RUST_CONTRACT_CLOCK"]))
        else:
            # Corpus owns the shared matrix. Reuse it rather than inserting its users twice.
            for name in ("real_scalar_bid", "ask_ok", "pending_no_grace", "assisted_advisory", "inactive_product_bid"):
                assert await session.scalar(text("SELECT count(*) FROM orderbook_orders WHERE id=:id"),
                                            {"id": oracle.fid("order:" + name)}) == 1
        # Separate ordinary and assisted advisory admissions so the HTTP result
        # proves each runtime switch branch rather than hiding it in a total.
        async def copy_order(name, window):
            # Insert a new fixture through normal guards: slice identity cannot
            # be changed on an existing order, even by the attested writer.
            columns = (await session.execute(text(
                "SELECT column_name FROM information_schema.columns "
                "WHERE table_schema='public' AND table_name='orderbook_orders' "
                "ORDER BY ordinal_position"
            ))).scalars().all()
            columns.remove("acceptance_ordinal")  # Use the normal sequence default.
            projections = ["CAST(:id AS uuid)" if c == "id" else
                           ":window" if c == "availability_window" else
                           "NULL" if c in ("idempotency_key", "idempotency_operation",
                                          "idempotency_request_hash") else
                           f'"{c}"' for c in columns]
            new_id = oracle.fid("curve:" + name)
            await session.execute(text(
                "INSERT INTO orderbook_orders (" + ",".join(f'"{c}"' for c in columns) +
                ") SELECT " + ",".join(projections) +
                " FROM orderbook_orders WHERE id=:source"
            ), {"id": new_id, "window": window, "source": oracle.fid("order:" + name)})
            return new_id

        for name, window in (("pending_no_grace", "2030-02"),
                             ("assisted_advisory", "2030-03"),
                             ("real_scalar_bid", "2030-01"), ("ask_ok", "2030-01")):
            await copy_order(name, window)
        await session.commit()
        products = (await session.execute(text(
            "SELECT DISTINCT product_id::text FROM orderbook_orders ORDER BY 1"
        ))).scalars().all()
        points = (await session.execute(text(
            "SELECT DISTINCT delivery_point_id::text FROM orderbook_orders ORDER BY 1"
        ))).scalars().all()
        inactive = (await session.execute(text(
            "SELECT o.product_id::text FROM orderbook_orders o JOIN products p ON p.id=o.product_id "
            "WHERE o.id=:id AND NOT p.is_active AND o.provenance='REAL'"
        ), {"id": oracle.fid("order:inactive_product_bid")})).scalar_one()
    cases = [dict(id=f"matrix-{index}-{kind}", method="GET",
                  path=f"/api/curves/forward{suffix}", query=f"product_id={product}")
             for index, product in enumerate(products)
             for kind, suffix in [("json", ""), ("csv", "/export")]]
    # Cover matching, other and absent points without changing catalog admission.
    points.append(str(oracle.fid("absent-delivery-point")))
    cases.extend(dict(id=f"point-{product_index}-{point_index}-{kind}", method="GET",
                      path=f"/api/curves/forward{suffix}",
                      query=f"product_id={product}&delivery_point_id={point}")
                 for product_index, product in enumerate(products)
                 for point_index, point in enumerate(points)
                 for kind, suffix in [("json", ""), ("csv", "/export")])
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

    # Use legal stored NUMERIC values, not HTTP input validation. Database checks
    # constrain the value to cents but permit additional trailing zeroes.
    async with seed_session() as session:
        product = (await session.execute(text(
            "SELECT product_id::text FROM orderbook_orders WHERE id=:id"
        ), {"id": oracle.fid("order:real_scalar_bid")})).scalar_one()
        for scale in (2, 27, 28, 30):
            bid = "1." + "0" * scale
            ask = "1.01" + "0" * (scale - 2)
            for name, price in (("real_scalar_bid", bid), ("ask_ok", ask)):
                await session.execute(text(
                    "UPDATE orderbook_orders SET price_per_mt_usd=CAST(:price AS numeric) "
                    "WHERE id=:id"
                ), {"id": oracle.fid("curve:" + name), "price": price})
            await session.commit()
            price_cases = [dict(id=f"scale-{scale}-{kind}", method="GET",
                                path=f"/api/curves/forward{suffix}",
                                query=f"product_id={product}")
                           for kind, suffix in [("json", ""), ("csv", "/export")]]
            expected = record(os.environ["PYTHON_CONTRACT_URL"], token, price_cases)
            actual = record(os.environ["RUST_CONTRACT_URL"], token, price_cases)
            problems = differences(expected, actual)
            assert not problems, problems
            for server in ("PYTHON_CONTRACT_URL", "RUST_CONTRACT_URL"):
                with urllib.request.urlopen(os.environ[server] +
                        "/api/curves/forward?product_id=" + product) as response:
                    body = json.load(response)
                point = next(p for p in body["curve"] if p["availability_window"] == "2030-01")
                assert point["best_bid"] == bid, point
                assert point["best_ask"] == ask, point
                assert point["mid_price"] == "1.00", point
                assert point["spread"] == "0.01", point
                gate_enabled = os.environ["KYC_EXECUTION_GATE_ENABLED"].lower() == "true"
                windows = {p["availability_window"] for p in body["curve"]}
                for window in ("2030-02", "2030-03"):
                    assert (window in windows) == (not gate_enabled), body
                with urllib.request.urlopen(os.environ[server] +
                        "/api/curves/forward/export?product_id=" + product) as response:
                    csv = response.read().decode()
                assert f"2030-01,{bid},{ask},1.00,0.01,200.00,2\n" in csv, csv
    print("legacy price scale: 8 PostgreSQL JSON/CSV comparisons (2, 27, 28, 30 places)")


asyncio.run(main())
