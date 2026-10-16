"""Task 7b exact-byte checks using only existing corpus rows."""
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from scripts.rust_contract.record import differences, record
from scripts.rust_contract.run_disposable import attest


def compare(path, query=""):
    case = {"id": "task7b-probe", "method": "GET", "path": path, "query": query}
    token = os.environ["DISPOSABLE_TEST_TOKEN"]
    left = record(os.environ["PYTHON_CONTRACT_URL"], token, [case])
    right = record(os.environ["RUST_CONTRACT_URL"], token, [case])
    assert not differences(left, right), (path, query, differences(left, right))
    return right[0]["exchange"]["body"]


async def check_ci_projection():
    from sqlalchemy import text

    from app.seeds.safety import seed_session

    # Reuse existing corpus rows; the runner owns this disposable database.
    async with seed_session() as session:
        await session.execute(text("""
            UPDATE orderbook_orders SET energy_density_mj_kg = 42.00
            WHERE status IN ('OPEN', 'PARTIALLY_FILLED')
        """))
        await session.commit()
    rows = compare("/api/orderbook/with-ci", "side=ASK&limit=100")
    overlays = [row["ci_adjusted_price"] for row in rows if row["ci_adjusted_price"]]
    assert overlays, "must exercise non-null CI projection"
    assert all(item["financial_benefit_status"] == "UNPRICED" for item in overlays)
    assert all(item["compliance_cost_per_mt"] == "0.00" for item in overlays)
    assert all(row["ci_adjusted_price"] is None for row in rows if row["lng_terms"])
    print(f"Task 7b: {len(overlays)} non-null CI overlays matched", flush=True)


def check_query_count():
    import json
    import subprocess
    from urllib.parse import urlsplit

    from scripts.rust_contract.run_disposable import IMAGE
    from scripts.rust_contract.image_identity import matches_pinned_image

    port = str(urlsplit(os.environ["RUST_DATABASE_URL"]).port)
    candidates = subprocess.check_output(
        ["docker", "ps", "--format", "{{.ID}}"], text=True,
    ).splitlines()
    matching = []
    for candidate in candidates:
        info = json.loads(subprocess.check_output(["docker", "inspect", candidate]))[0]
        bindings = info["NetworkSettings"]["Ports"].get("5432/tcp") or []
        if any(b["HostIp"] == "127.0.0.1" and b["HostPort"] == port for b in bindings):
            # Podman canonicalizes away the tag and adds docker.io; retain
            # the exact repository and immutable digest check on both runtimes.
            assert matches_pinned_image(info["Config"]["Image"], IMAGE)
            matching.append(candidate)
    assert len(matching) == 1
    container = matching[0]

    def admin(sql):
        return subprocess.check_output(
            ["docker", "exec", container, "psql", "-XAt", "-U", "postgres",
             "-d", "verdaxis_rust_test", "-v", "ON_ERROR_STOP=1", "-c", sql],
            text=True,
        ).strip()

    admin("SELECT pg_stat_statements_reset()")
    record(os.environ["RUST_CONTRACT_URL"], os.environ["DISPOSABLE_TEST_TOKEN"],
           [{"id": "batch-count", "method": "GET", "path": "/api/orderbook/asks",
             "query": "limit=100"}])
    count = int(admin("""
        SELECT coalesce(sum(calls), 0) FROM pg_stat_statements
        WHERE userid=(SELECT oid FROM pg_roles WHERE rolname='rust_owner')
          AND dbid=(SELECT oid FROM pg_database WHERE datname=current_database())
          AND query LIKE '%, requested_keys AS MATERIALIZED (%'
    """))
    assert count == 1, f"expected one benchmark statement, observed {count}"
    print("Task 7b: one benchmark statement for the full ASK page", flush=True)


async def seed_mixed_sort_pages():
    import importlib.util
    from datetime import UTC, datetime

    from sqlalchemy import text

    from app.seeds.safety import seed_session

    spec = importlib.util.spec_from_file_location(
        "market_oracle", Path(__file__).with_name("market_contract_oracle.py")
    )
    oracle = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(oracle)
    async with seed_session() as session:
        columns = list((await session.execute(text(
            "SELECT column_name FROM information_schema.columns "
            "WHERE table_schema='public' AND table_name='orderbook_orders' "
            "ORDER BY ordinal_position"
        ))).scalars())
        columns.remove("acceptance_ordinal")
        replacements = {
            "id": "CAST(:id AS uuid)",
            "availability_window": "'2033-09'",
            "price_per_mt_usd": ":price",
            "quantity_mt": ":quantity",
            "remaining_quantity_mt": ":quantity",
            "created_at": "CAST(:created AS timestamptz)",
            "status": "'OPEN'",
            "idempotency_key": "NULL",
            "idempotency_operation": "NULL",
            "idempotency_request_hash": "NULL",
        }
        projection = [replacements.get(column, f'"{column}"') for column in columns]
        statement = text(
            "INSERT INTO orderbook_orders (" + ",".join(f'"{c}"' for c in columns)
            + ") SELECT " + ",".join(projection)
            + " FROM orderbook_orders WHERE id=:source"
        )
        # Insertion/acceptance order opposes ascending price. The middle price
        # tie straddles a two-row page; quantities and creation times also vary.
        for source in ("real_scalar_bid", "ask_ok"):
            for index, (price, quantity, day) in enumerate(
                ((900, 60, 12), (700, 100, 13), (700, 100, 14), (500, 80, 11))
            ):
                result = await session.execute(statement, {
                    "id": oracle.fid(f"lists7b:mixed:{source}:{index}"),
                    "source": oracle.fid("order:" + source),
                    "price": price, "quantity": quantity,
                    "created": datetime(2026, 10, day, 12, tzinfo=UTC),
                })
                assert result.rowcount == 1
        await session.commit()


def check_mixed_sort_pages():
    from decimal import Decimal

    for side in ("bids", "asks"):
        path = "/api/orderbook/" + side
        orders = {}
        for sort in ("price_asc", "price_desc", "quantity_desc", "newest"):
            query = f"availability_window=2033-09&sort_by={sort}"
            full = compare(path, query + "&limit=100")
            assert full["total"] == 4, "mixed-sort lane fixtures must be present"
            rows = full["items"]
            prices = [Decimal(row["price_per_mt_usd"]) for row in rows]
            if sort.startswith("price_"):
                assert prices == sorted(prices, reverse=sort == "price_desc")
                assert len(set(prices)) == 3
            elif sort == "quantity_desc":
                keys = [(Decimal(row["remaining_quantity_mt"]), row["created_at"], row["id"]) for row in rows]
                assert keys == sorted(keys, reverse=True)
            else:
                keys = [(row["created_at"], row["id"]) for row in rows]
                assert keys == sorted(keys, reverse=True)
            pages = []
            for skip in (0, 2, 4):
                page = compare(path, query + f"&skip={skip}&limit=2")
                assert page["total"] == 4
                pages.extend(page["items"])
            assert pages == rows
            orders[sort] = [row["id"] for row in rows]
        assert orders["price_asc"] != orders["price_desc"]
    # Legacy routes ignore sort_by and retain newest order even with mixed prices.
    for path in ("/api/orderbook", "/api/orderbook/with-ci"):
        rows = compare(path, "limit=100&sort_by=price_asc")
        assert rows == sorted(rows, key=lambda row: (row["created_at"], row["id"]), reverse=True)
    print("Task 7b: mixed-value sort pages matched on all four routes", flush=True)


def check_integer_boundaries():
    oversized = "9223372036854775808"
    for path in ("/api/orderbook", "/api/orderbook/with-ci",
                 "/api/orderbook/bids", "/api/orderbook/asks"):
        for query in (
            f"skip={oversized}&page=1&page_size=2",
            f"skip={oversized}",
            f"page={oversized}&limit=2",
            "skip=-1&page=1",
            "skip=wrong&page=1",
            "limit=101&page_size=2",
        ):
            compare(path, query)
    print("Task 7b: 24 integer boundary comparisons passed", flush=True)


def main():
    for name in ("PYTHON_CONTRACT_URL", "RUST_CONTRACT_URL"):
        attest(os.environ[name], os.environ["DISPOSABLE_TEST_TOKEN"])
    check_integer_boundaries()
    count = 0
    for side in ("bids", "asks"):
        path = "/api/orderbook/" + side
        for sort in ("newest", "price_asc", "price_desc", "quantity_desc"):
            full = compare(path, f"sort_by={sort}&availability_window=SPOT&limit=100")
            assert full["total"] <= 100, "exhaustive SPOT fixture must fit the baseline page"
            count += 1
            seen = []
            for skip in range(0, full["total"] + 3, 2):
                page = compare(path, f"sort_by={sort}&availability_window=SPOT&skip={skip}&limit=2")
                assert page["total"] == full["total"]
                assert page["items"] == full["items"][skip:skip + 2]
                seen.extend(row["id"] for row in page["items"])
                count += 1
            assert len(seen) == len(set(seen)) == full["total"]
        for query in (
            "skip=98&limit=5", "skip=100&limit=2", "skip=10000&limit=2",
            "skip=100000&limit=1", "region=Singapore", "region=Asia",
            "region=Rotterdam", "region=Europe", "region=", "region=missing",
            "include_off_spec=true", "limit=100", "limit=0", "limit=101",
            "skip=-1", "sort_by=wrong", "market_product=wrong",
            "availability_window=wrong", "availability_window=", "side=ignored",
            "limit=1&limit=2", "include_off_spec=wrong", "fuel_type=",
            "product_id=bad&delivery_point_id=bad&include_off_spec=x&sort_by=x&skip=-1&limit=0",
        ):
            compare(path, query)
            count += 1
    from app.market_catalog import PRODUCT_IDS
    legacy_scope = "product_id=" + str(PRODUCT_IDS["BIO_LNG"])
    for path in ("/api/orderbook", "/api/orderbook/with-ci"):
        full = compare(path, legacy_scope + "&limit=100")
        expected = sorted(full, key=lambda row: (row["created_at"], row["id"]), reverse=True)
        assert full == expected
        for skip in range(0, len(full) + 3, 2):
            page = compare(path, legacy_scope + f"&skip={skip}&limit=2")
            assert page == full[skip:skip + 2]
            count += 1
        for query in ("side=BID", "side=ASK", "side=bad", "limit=0", "limit=101",
                      "include_off_spec=true", "skip=100000", "limit=1&limit=2"):
            compare(path, query)
            count += 1
    for query in ("page=2&page_size=2", "page=2&skip=9&limit=10&page_size=2",
                  "page_size=2&skip=2", "page=1", "page=100000", "page=0",
                  "page_size=101", "availability_window=SPOT"):
        compare("/api/orderbook", query)
        count += 1
    # Every list uses the same allowlisted DTO, including digest and benchmark.
    for row in full:
        assert not {"organization_id", "user_id", "private_digest_input"} & row.keys()
        if row["lng_terms"] is not None:
            assert row["ci_adjusted_price"] is None
        overlay = row["ci_adjusted_price"]
        if overlay is not None:
            assert overlay["financial_benefit_status"] == "UNPRICED"
            assert overlay["compliance_cost_per_mt"] == "0.00"
    import asyncio
    asyncio.run(check_ci_projection())
    check_query_count()
    asyncio.run(seed_mixed_sort_pages())
    check_mixed_sort_pages()
    print(f"Task 7b: {count} exact edge comparisons passed", flush=True)


if __name__ == "__main__":
    main()
