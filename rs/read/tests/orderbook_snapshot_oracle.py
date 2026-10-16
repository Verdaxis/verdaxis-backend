"""R15 edge and concurrency probes on the attested disposable corpus only."""

import asyncio
import importlib.util
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from urllib.parse import urlencode

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))

from sqlalchemy import text

from app.seeds.safety import seed_session
from scripts.rust_contract.record import differences, record
from scripts.rust_contract.run_disposable import (
    MIGRATION_REVISION,
    ORACLE_TIMEZONE,
    attest,
    ephemeral_port,
    wait_for_port,
)


def case(name, product, point, window="2032-01"):
    return dict(
        id=name,
        method="GET",
        path="/api/orderbook/snapshot",
        query=urlencode(
            dict(
                market_product=product,
                delivery_point_id=point,
                availability_window=window,
            )
        ),
    )


def fetch(url, item):
    return record(url, os.environ["DISPOSABLE_TEST_TOKEN"], [item])[0]


def compare(item):
    left = fetch(os.environ["PYTHON_CONTRACT_URL"], item)
    time.sleep(0.05)
    right = fetch(os.environ["RUST_CONTRACT_URL"], item)
    assert not differences([left], [right]), (item, differences([left], [right]))
    return right["exchange"]


async def main():
    for name in ("PYTHON_CONTRACT_URL", "RUST_CONTRACT_URL"):
        attest(os.environ[name], os.environ["DISPOSABLE_TEST_TOKEN"])
    spec = importlib.util.spec_from_file_location(
        "market_oracle", Path(__file__).with_name("market_contract_oracle.py")
    )
    oracle = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(oracle)
    policy = json.loads((ROOT / "deploy/rust/market-policy.json").read_text())
    query_data = json.loads(
        (ROOT / "rs/read/src/market/orderbook_query.json").read_text()
    )
    from app.market_catalog import MarketProduct

    assert query_data["market_product_enum"] == [item.value for item in MarketProduct]
    assert query_data["source_sha"] == policy["source_sha"]
    from app.schemas.fame_order import FameBidTerms, FamePublicAskTerms
    from app.schemas.lng_order import LngBidTerms, LngPublicAskTerms
    from app.schemas.supplier_offer import (
        FameQualityResult,
        FameQualitySummary,
        FameSustainabilitySummary,
    )

    assert query_data["public_terms_fields"] == {
        name: list(model.model_fields)
        for name, model in [
            ("fame_bid", FameBidTerms),
            ("fame_ask", FamePublicAskTerms),
            ("lng_bid", LngBidTerms),
            ("lng_ask", LngPublicAskTerms),
            ("quality_evidence", FameQualitySummary),
            ("sustainability_evidence", FameSustainabilitySummary),
            ("quality_result", FameQualityResult),
        ]
    }

    async with seed_session() as session:
        columns = (
            (
                await session.execute(
                    text(
                        "SELECT column_name FROM information_schema.columns WHERE table_schema='public' AND table_name='orderbook_orders' ORDER BY ordinal_position"
                    )
                )
            )
            .scalars()
            .all()
        )
        columns.remove("acceptance_ordinal")

        async def copy(source, name, window, price):
            replacements = {
                "id": "CAST(:id AS uuid)",
                "availability_window": ":window",
                "price_per_mt_usd": ":price",
                "status": "'OPEN'",
            }
            replacements.update(
                {
                    key: "NULL"
                    for key in (
                        "idempotency_key",
                        "idempotency_operation",
                        "idempotency_request_hash",
                    )
                }
            )
            projection = [replacements.get(column, f'"{column}"') for column in columns]
            identity = oracle.fid("snapshot7a:" + name)
            result = await session.execute(
                text(
                    "INSERT INTO orderbook_orders ("
                    + ",".join(f'"{c}"' for c in columns)
                    + ") SELECT "
                    + ",".join(projection)
                    + " FROM orderbook_orders WHERE id=:source"
                ),
                dict(
                    id=identity,
                    source=oracle.fid("order:" + source),
                    window=window,
                    price=price,
                ),
            )
            assert result.rowcount == 1, source
            return str(identity)

        expected = {"BID": [], "ASK": []}
        for side, source, price in [
            ("BID", "real_scalar_bid", 610),
            ("ASK", "ask_ok", 600),
        ]:
            for index in range(16):
                expected[side].append(
                    await copy(source, f"depth-{side}-{index}", "2032-01", price)
                )
        await copy("demo_ownerless_bid", "demo-cross", "2032-02", 900)
        await copy("ask_ok", "real-not-crossed", "2032-02", 600)
        await copy("fame_ask_ok", "typed-good", "2032-03", 600)
        typed_bad_id = await copy("fame_ask_ok", "typed-bad", "2032-03", 900)
        # Valid public schema, but full execution qualification rejects a
        # future quality sample date. The shallow public relation still admits it.
        await session.execute(
            text(
                "UPDATE orderbook_orders SET fame_terms = jsonb_set(fame_terms::jsonb, '{quality_evidence}', CAST(:quality AS jsonb))::json WHERE id=:id"
            ),
            {
                "id": typed_bad_id,
                "quality": json.dumps(
                    {"status": "PENDING", "sampled_on": "2030-01-01", "results": []}
                ),
            },
        )
        # Reopen a copy of Task 4b's labelled negative declaration, not its
        # cancelled corpus row. Public projection must not validate private fields.
        await copy("fame_ask_ok", "private-good", "2032-06", 600)
        private_bad_id = await copy(
            "typed_invalid_visible", "private-bad", "2032-06", 900
        )
        await copy("lng_ask_ok", "lng-private-good", "2032-07", 600)
        lng_bad_id = await copy("lng_ask_ok", "lng-private-bad", "2032-07", 900)
        await session.execute(
            text("UPDATE orderbook_orders SET lng_terms = jsonb_set(jsonb_set(lng_terms::jsonb, '{scheme_certificate_number}', '\"\"'), '{physical_supplier_name}', '\"\"')::json WHERE id=:id"),
            {"id": lng_bad_id},
        )
        concurrent_id = await copy("ask_ok", "concurrent", "2032-04", 600)
        expiry_id = await copy("ask_ok", "expiry-boundary", "2032-05", 600)
        await session.execute(
            text(
                "UPDATE orderbook_orders SET expires_at=CAST(CAST(:clock AS text) AS timestamptz) + interval '1 microsecond' WHERE id=:id"
            ),
            {"id": expiry_id, "clock": os.environ["RUST_CONTRACT_CLOCK"]},
        )
        await session.commit()
        source_rows = (
            await session.execute(
                text(
                    "SELECT product_id::text, delivery_point_id::text FROM orderbook_orders WHERE id=:id"
                ),
                {"id": oracle.fid("order:ask_ok")},
            )
        ).one()
        typed_rows = (
            await session.execute(
                text(
                    "SELECT product_id::text, delivery_point_id::text FROM orderbook_orders WHERE id=:id"
                ),
                {"id": oracle.fid("order:fame_ask_ok")},
            )
        ).one()

    product = next(p["code"] for p in policy["products"] if p["id"] == source_rows[0])
    typed = next(p["code"] for p in policy["products"] if p["id"] == typed_rows[0])
    depth = compare(case("depth-16-equal-prices", product, source_rows[1]))
    assert depth["status"] == 200
    for side, key in [("BID", "bids"), ("ASK", "asks")]:
        assert [item["id"] for item in depth["body"][key]] == expected[side][:15]
        assert all(item["is_crossed"] for item in depth["body"][key])
    mixed = compare(case("provenance-separation", product, source_rows[1], "2032-02"))[
        "body"
    ]
    assert mixed["demo_status"] == "MIXED"
    assert not any(item["is_crossed"] for item in mixed["bids"] + mixed["asks"])
    typed_body = compare(
        case("typed-benchmark-exclusion", typed, typed_rows[1], "2032-03")
    )["body"]
    assert len(typed_body["asks"]) == 2
    assert {item["benchmark_price_per_mt_usd"] for item in typed_body["asks"]} == {
        "600.00"
    }
    # Private declarations affect the digest, but never appear in a public item.
    forbidden = {
        "organization_id",
        "owner_user_id",
        "created_by_actor_user_id",
        "inventory_item_id",
        "private_digest_input",
        "scheme_certificate_number",
        "certificate_reference",
        "certificate_holder",
        "physical_supplier_name",
        "certificate_holder_name",
    }

    def privacy(value):
        if isinstance(value, dict):
            assert not forbidden.intersection(value), forbidden.intersection(value)
            for child in value.values():
                privacy(child)
        elif isinstance(value, list):
            for child in value:
                privacy(child)

    privacy(typed_body)
    lng_product = next(p for p in policy["products"] if p["qualification_profile"] == "lng_v1")
    for name, code, point, window, bad_id in [
        ("negative-fame-holder", typed, typed_rows[1], "2032-06", private_bad_id),
        ("negative-lng-private", lng_product["code"], lng_product["lanes"][0], "2032-07", lng_bad_id),
    ]:
        exchange = compare(case(name, code, point, window))
        assert exchange["status"] == 200
        asks = exchange["body"]["asks"]
        assert len(asks) == 2 and bad_id in {row["id"] for row in asks}
        assert {row["benchmark_price_per_mt_usd"] for row in asks} == {"600.00"}
        assert all(len(row["terms_digest"]) == 64 for row in asks)
        privacy(exchange["body"])
    for index, catalog_product in enumerate(policy["products"]):
        if catalog_product["execution_mode"] == "ORDERBOOK":
            for point_index, point in enumerate(catalog_product["lanes"]):
                exchange = compare(
                    case(
                        f"catalog-{index}-{point_index}",
                        catalog_product["code"],
                        point,
                        "SPOT",
                    )
                )
                assert exchange["status"] == 200
                privacy(exchange["body"])
    for suffix in ["", "&availability_window=2032-01", "&ignored=anything"]:
        compare(
            case("repeat-unknown" + suffix, product, source_rows[1])
            | {"query": case("", product, source_rows[1])["query"] + suffix}
        )
    compare(case("empty-slice", product, source_rows[1], "2032-12"))
    compare(case("bad-window", product, source_rows[1], "not-a-window"))
    compare(
        dict(
            id="invalid-fields",
            method="GET",
            path="/api/orderbook/snapshot",
            query="market_product=NOPE&delivery_point_id=bad",
        )
    )

    # Count actual statements in this disposable container, not source call sites.
    # Resolve the container by the attested numeric-loopback database port and
    # require the harness's exact image before changing its logging setting.
    from urllib.parse import urlsplit

    from scripts.rust_contract.run_disposable import IMAGE
    from scripts.rust_contract.image_identity import matches_pinned_image

    database_port = urlsplit(os.environ["RUST_DATABASE_URL"]).port
    containers = subprocess.check_output(
        ["docker", "ps", "--format", "{{.ID}}"], text=True
    ).splitlines()
    matching = []
    for container in containers:
        info = json.loads(
            subprocess.check_output(["docker", "inspect", container], text=True)
        )[0]
        bindings = info["NetworkSettings"]["Ports"].get("5432/tcp") or []
        if any(
            binding["HostIp"] == "127.0.0.1"
            and binding["HostPort"] == str(database_port)
            for binding in bindings
        ):
            assert matches_pinned_image(info["Config"]["Image"], IMAGE)
            matching.append(container)
    assert len(matching) == 1, "expected one pinned disposable container"
    container = matching[0]

    def admin(sql):
        return subprocess.check_output(
            ["docker", "exec", container, "psql", "-XAt", "-U", "postgres",
             "-d", "verdaxis_rust_test", "-v", "ON_ERROR_STOP=1", "-c", sql],
            text=True,
        ).strip()

    admin("SELECT pg_stat_statements_reset()")
    fetch(os.environ["RUST_CONTRACT_URL"], case("two-statements", product, source_rows[1]))
    for marker in (", ranked_orders AS (", ", requested_keys AS MATERIALIZED ("):
        count = int(admin(f"""
            SELECT coalesce(sum(calls), 0) FROM pg_stat_statements
            WHERE userid=(SELECT oid FROM pg_roles WHERE rolname='rust_owner')
              AND dbid=(SELECT oid FROM pg_database WHERE datname=current_database())
              AND query LIKE '%{marker}%'
        """))
        assert count == 1, (marker, count)

    # Launch a second test-only server with a barrier after depth. The normal
    # server has no barrier and remains available for the exact HTTP comparisons.
    port = ephemeral_port()
    config = dict(
        environment="test",
        release_sha=os.environ["RELEASE_SHA"],
        migration_revision=MIGRATION_REVISION,
        database_url=os.environ["RUST_DATABASE_URL"],
        db_session_time_zone=ORACLE_TIMEZONE,
        bind=f"127.0.0.1:{port}",
        pool_max=9,
        python_budget=52,
        configured_max_connections=100,
        kyc_execution_gate_enabled=os.environ["KYC_EXECUTION_GATE_ENABLED"].lower()
        == "true",
    )
    barrier = 7071001
    child = subprocess.Popen(
        [str(ROOT / "rs/target/debug/examples/contract_server")],
        env={
            **os.environ,
            "RUST_CONFIG": json.dumps(config),
            "CONTRACT_GROUP": "orderbook_snapshot",
            "RUST_SNAPSHOT_TEST_BARRIER": str(barrier),
        },
    )
    try:
        await asyncio.to_thread(wait_for_port, port)
        url = f"http://127.0.0.1:{port}"
        attest(url, os.environ["DISPOSABLE_TEST_TOKEN"])
        item = case("concurrent-writer", product, source_rows[1], "2032-04")
        async with seed_session() as session:
            await session.execute(
                text("SELECT pg_advisory_lock(:key)"), {"key": barrier}
            )
            pending = asyncio.create_task(asyncio.to_thread(fetch, url, item))
            try:
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    waiting = await session.scalar(
                        text(
                            "SELECT count(*) FROM pg_locks WHERE locktype='advisory' AND objid=:key AND NOT granted"
                        ),
                        {"key": barrier},
                    )
                    if waiting:
                        break
                    await asyncio.sleep(0.05)
                else:
                    raise AssertionError(
                        "snapshot did not reach the between-read barrier"
                    )
                await session.execute(
                    text(
                        "UPDATE orderbook_orders SET price_per_mt_usd=800 WHERE id=:id"
                    ),
                    {"id": concurrent_id},
                )
                await session.commit()
            finally:
                await session.execute(
                    text("SELECT pg_advisory_unlock(:key)"), {"key": barrier}
                )
                await session.commit()
            before = (await pending)["exchange"]["body"]["asks"][0]
            assert (
                before["price_per_mt_usd"],
                before["benchmark_price_per_mt_usd"],
            ) == ("600", "600.00"), before
        after = compare(item)["body"]["asks"][0]
        assert (after["price_per_mt_usd"], after["benchmark_price_per_mt_usd"]) == (
            "800",
            "800.00",
        ), after
        expiry_case = case("captured-expiry", product, source_rows[1], "2032-05")
        active = compare(expiry_case)["body"]["asks"]
        assert len(active) == 1 and active[0]["benchmark_price_per_mt_usd"] == "600.00"
        # Advance only the clock; do not write the expiring order.
        from datetime import datetime, timedelta

        later_port = ephemeral_port()
        config["bind"] = f"127.0.0.1:{later_port}"
        later_clock = (
            datetime.fromisoformat(os.environ["RUST_CONTRACT_CLOCK"])
            + timedelta(microseconds=1)
        ).isoformat()
        later = subprocess.Popen(
            [str(ROOT / "rs/target/debug/examples/contract_server")],
            env={
                **os.environ,
                "RUST_CONFIG": json.dumps(config),
                "CONTRACT_GROUP": "orderbook_snapshot",
                "RUST_CONTRACT_CLOCK": later_clock,
            },
        )
        try:
            await asyncio.to_thread(wait_for_port, later_port)
            expired = fetch(f"http://127.0.0.1:{later_port}", expiry_case)["exchange"][
                "body"
            ]
            assert expired["asks"] == [] and expired["bids"] == []
            assert expired["source_kind"] == "NO_DATA"
        finally:
            later.terminate()
            try:
                later.wait(timeout=10)
            except subprocess.TimeoutExpired:
                later.kill()
                later.wait(timeout=10)
    finally:
        child.terminate()
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=10)
    print(
        "snapshot edge probes passed: 16 levels per side, ties, provenance, typed benchmark, privacy, query errors, concurrent writer"
    )


if __name__ == "__main__":
    asyncio.run(main())
