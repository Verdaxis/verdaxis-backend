"""Task 7c comparisons on the existing, attested disposable corpus."""
import json
import os
import subprocess
import sys
from pathlib import Path
from urllib.parse import urlencode

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from scripts.rust_contract.record import differences, record
from scripts.rust_contract.run_disposable import (
    MIGRATION_REVISION,
    ORACLE_TIMEZONE,
    attest,
    ephemeral_port,
    wait_for_port,
)


def compare(url, path, query=""):
    case = dict(id=f"7c-{path}-{query}", method="GET", path=path, query=query)
    token = os.environ["DISPOSABLE_TEST_TOKEN"]
    left = record(os.environ["PYTHON_CONTRACT_URL"], token, [case])
    right = record(url, token, [case])
    problems = differences(left, right)
    assert not problems, (case, problems)
    return right[0]["exchange"]


def compact_year_zero_failure(url):
    """Compare an isolated malformed-history response, restoring the corpus."""
    import asyncio

    from sqlalchemy import text

    from app.seeds.safety import seed_session
    from scripts.rust_contract.seed_corpus import attest_session

    from uuid import uuid4
    order_id = uuid4()

    async def set_fixture(present):
        async with seed_session() as session:
            await attest_session(session)
            if present:
                # Copy a valid DEMO scalar row through the normal insert guards.
                # Slice identity is immutable, so never UPDATE its window.
                from app.models.orderbook import OrderBookOrder
                columns = [column.name for column in OrderBookOrder.__table__.columns
                           if column.name not in {"id", "availability_window", "acceptance_ordinal"}]
                names = ", ".join('"' + name + '"' for name in columns)
                await session.execute(text(
                    f"INSERT INTO orderbook_orders (id, availability_window, {names}) "
                    f"SELECT :id, '0000-Q1', {names} FROM orderbook_orders "
                    "WHERE provenance='DEMO' AND status='OPEN' "
                    "AND fame_terms IS NULL AND lng_terms IS NULL "
                    "AND idempotency_key IS NULL ORDER BY id LIMIT 1"
                ), {"id": order_id})
            else:
                await session.execute(text(
                    "DELETE FROM orderbook_orders WHERE id=:id"
                ), {"id": order_id})
            await session.commit()

    asyncio.run(set_fixture(True))
    try:
        case = dict(id="7c-year-zero", method="GET",
                    path="/api/orderbook/map-summary/compact", query="",
                    headers={"Origin": "https://staging.verdaxis.exchange",
                             "X-Request-ID": "task7c-year-zero"})
        token = os.environ["DISPOSABLE_TEST_TOKEN"]
        left = record(os.environ["PYTHON_CONTRACT_URL"], token, [case])
        right = record(url, token, [case])
        assert left[0]["exchange"]["status"] == 500
        assert not differences(left, right), differences(left, right)
    finally:
        asyncio.run(set_fixture(False))


def main():
    token = os.environ["DISPOSABLE_TEST_TOKEN"]
    url = os.environ["RUST_CONTRACT_URL"]
    attest(url, token)
    attest(os.environ["PYTHON_CONTRACT_URL"], token)
    policy = json.loads((ROOT / "deploy/rust/market-policy.json").read_text())
    compact_year_zero_failure(url)
    count = 1
    for path in ["aggregated", "product-counts"]:
        base = "side=ASK&" if path == "product-counts" else ""
        queries = ["", "region=", "region=unmatched", "availability_window=SPOT",
                   "availability_window=invalid", "include_off_spec=true",
                   "include_off_spec=bad", "delivery_point_id=bad",
                   "delivery_point_id=bad&include_off_spec=bad&limit=0"]
        for point in policy["points"]:
            queries.extend([urlencode({"region": point["name"]}),
                            urlencode({"region": point["region"]}),
                            urlencode({"delivery_point_id": point["id"]})])
        if path == "aggregated":
            queries.extend(["limit=1", "limit=2048", "limit=0", "market_product=bad",
                            "product_id=bad&market_product=bad&include_off_spec=bad&limit=0",
                            "limit=0&limit=2&unknown=ignored"])
            for product in policy["products"]:
                queries.append(urlencode({"market_product": product["code"]}))
        else:
            queries.extend(["side=BID", "side=bad"])
        for query in queries:
            compare(url, f"/api/orderbook/{path}", base + query)
            count += 1
    compare(url, "/api/orderbook/product-counts")
    empty = compare(url, "/api/orderbook/product-counts", "side=BID&region=unmatched")["body"]
    assert empty["total"] == 0 and empty["counts"] and not any(empty["counts"].values())
    summary = compare(url, "/api/orderbook/map-summary")["body"]
    compact = compare(url, "/api/orderbook/map-summary/compact")["body"]
    assert summary["groups"] and summary["recent_asks"] and compact["markets"]
    for market in compact["markets"]:
        groups = [group for group in summary["groups"] if all(
            group[key] == market[key] for key in ("product_id", "delivery_point_id", "evidence_class"))]
        assert market["bid_order_count"] + market["ask_order_count"] == sum(group["order_count"] for group in groups)
    for field in ["products", "regions", "fuel-types"]:
        values = compare(url, f"/api/orderbook/{field}")["body"]
        assert values == sorted(set(values))
    # The shared test server assigns compatibility routes to the list group.
    # Start that existing route set without changing integration-owned dispatch.
    port = ephemeral_port()
    config = dict(environment="test", release_sha=os.environ["RELEASE_SHA"],
                  migration_revision=MIGRATION_REVISION, database_url=os.environ["RUST_DATABASE_URL"],
                  db_session_time_zone=ORACLE_TIMEZONE, bind=f"127.0.0.1:{port}", pool_max=9,
                  python_budget=52, configured_max_connections=100,
                  kyc_execution_gate_enabled=os.environ["KYC_EXECUTION_GATE_ENABLED"].lower() == "true")
    child = subprocess.Popen([str(ROOT / "rs/target/debug/examples/contract_server")],
                            env={**os.environ, "RUST_CONFIG": json.dumps(config), "CONTRACT_GROUP": "orderbook_lists"})
    try:
        wait_for_port(port)
        listings_url = f"http://127.0.0.1:{port}"
        attest(listings_url, token)
        for query in ["", "limit=100", "skip=1&limit=2", "skip=9999", "skip=-1&limit=0", "limit=bad", "limit=0&limit=2"]:
            response = compare(listings_url, "/api/listings", query)
            if query == "limit=100":
                from scripts.rust_contract.seed_corpus import matrix_module
                missing_declaration = str(matrix_module().fid("order:ask_missing_declaration"))
                assert missing_declaration in {item["id"] for item in response["body"]}
                forbidden = {"organization_id", "user_id", "actor_user_id", "support_authorization_id", "private_digest_input"}
                assert all(not forbidden.intersection(item) for item in response["body"])
            count += 1
        cases = json.loads((ROOT / "tests/rust_contract/cases.json").read_text())["groups"]["orderbook_map"]
        compatibility = [case for case in cases if case["path"] == "/api/listings"]
        left = record(os.environ["PYTHON_CONTRACT_URL"], token, compatibility)
        right = record(listings_url, token, compatibility)
        assert not differences(left, right), differences(left, right)
        count += len(compatibility)
    finally:
        child.terminate()
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
    print(f"Task 7c: {count + 7} edge comparisons passed", flush=True)


if __name__ == "__main__":
    main()
