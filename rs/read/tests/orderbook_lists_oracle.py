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
    rows = compare("/api/orderbook/with-ci", "limit=100")
    overlays = [row["ci_adjusted_price"] for row in rows if row["ci_adjusted_price"]]
    assert overlays, "must exercise non-null CI projection"
    assert all(item["financial_benefit_status"] == "UNPRICED" for item in overlays)
    assert all(item["compliance_cost_per_mt"] == "0.00" for item in overlays)
    assert all(row["ci_adjusted_price"] is None for row in rows if row["lng_terms"])
    print(f"Task 7b: {len(overlays)} non-null CI overlays matched", flush=True)


def check_query_count():
    import json
    import subprocess
    import time
    from urllib.parse import urlsplit

    from scripts.rust_contract.run_disposable import IMAGE

    port = str(urlsplit(os.environ["RUST_DATABASE_URL"]).port)
    candidates = subprocess.check_output(
        ["docker", "ps", "--format", "{{.ID}}"], text=True,
    ).splitlines()
    matching = []
    for candidate in candidates:
        info = json.loads(subprocess.check_output(["docker", "inspect", candidate]))[0]
        bindings = info["NetworkSettings"]["Ports"].get("5432/tcp") or []
        if any(b["HostIp"] == "127.0.0.1" and b["HostPort"] == port for b in bindings):
            assert info["Config"]["Image"] == IMAGE
            matching.append(candidate)
    assert len(matching) == 1
    container = matching[0]

    def admin(sql):
        subprocess.run(["docker", "exec", container, "psql", "-U", "postgres",
                        "-v", "ON_ERROR_STOP=1", "-c", sql], check=True, capture_output=True)

    def logs():
        result = subprocess.run(["docker", "logs", container], check=True,
                                capture_output=True, text=True)
        return result.stdout + result.stderr

    admin("ALTER SYSTEM SET log_statement = 'all'")
    admin("SELECT pg_reload_conf()")
    try:
        time.sleep(0.2)
        before = logs()
        record(os.environ["RUST_CONTRACT_URL"], os.environ["DISPOSABLE_TEST_TOKEN"],
               [{"id": "batch-count", "method": "GET", "path": "/api/orderbook/asks",
                 "query": "limit=100"}])
        time.sleep(0.2)
        after = logs()
        marker = ", benchmark_candidates AS MATERIALIZED ("
        assert after.count(marker) - before.count(marker) == 1
        print("Task 7b: one benchmark statement for the full ASK page", flush=True)
    finally:
        admin("ALTER SYSTEM RESET log_statement")
        admin("SELECT pg_reload_conf()")


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
            full = compare(path, f"sort_by={sort}&limit=100")
            count += 1
            seen = []
            for skip in range(0, full["total"] + 3, 2):
                page = compare(path, f"sort_by={sort}&skip={skip}&limit=2")
                assert page["total"] == full["total"]
                assert page["items"] == full["items"][skip:skip + 2]
                seen.extend(row["id"] for row in page["items"])
                count += 1
            assert len(seen) == len(set(seen)) == full["total"]
        for query in (
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
    for path in ("/api/orderbook", "/api/orderbook/with-ci"):
        full = compare(path, "limit=100")
        expected = sorted(full, key=lambda row: (row["created_at"], row["id"]), reverse=True)
        assert full == expected
        for skip in range(0, len(full) + 3, 2):
            page = compare(path, f"skip={skip}&limit=2")
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
    print(f"Task 7b: {count} exact edge comparisons passed", flush=True)


if __name__ == "__main__":
    main()
