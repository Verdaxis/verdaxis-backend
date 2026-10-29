"""Run the watchlist guard component on disposable PostgreSQL.

The Rust target ``streams_watchlist_guard`` holds the scenarios. Its database
tests are ignored in ordinary builds. This suite seeds one market through the
schema owner, then runs every one of them on the migrated schema: watchlist
fixture rows are written by the schema owner, the feed, count and transport
statements run as the deployed application role, and the suite fails unless
each named test passed.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import urlsplit
from uuid import uuid4

import pytest
from sqlalchemy import text

ROOT = Path(__file__).resolve().parents[2]
TARGET = "streams_watchlist_guard"
OFFLINE_TESTS = {
    "no_producer_startup",
    "no_default_activation_cutoff",
    "source_rules_reject_each_forbidden_form",
}
DATABASE_TESTS = {
    "watchlist_feed_metrics_contract",
    "dispatch_does_not_create_watchlist_rows",
    "best_price_guard_before_at_after_cutoff",
    "guard_precedes_pagination_and_counts",
}
PASSED_LINE = re.compile(r"^test (\w+) \.\.\. ok$", re.MULTILINE)
SUMMARY_LINE = re.compile(
    r"^test result: ok\. (\d+) passed; 0 failed; 0 ignored;", re.MULTILINE
)


def _sqlx_url(url: str) -> str:
    assert url.startswith("postgresql+asyncpg://"), "the harness supplies asyncpg URLs"
    return url.replace("postgresql+asyncpg://", "postgresql://", 1)


async def _seed_market(session) -> dict[str, str]:
    """A REAL organization with an open public order and a cancelled order.

    Both orders are BID rows of one active product and delivery point in the
    SPOT window, so only their status decides whether they are public.
    """
    label = uuid4().hex
    market = {
        "organization": uuid4(),
        "owner": uuid4(),
        "public_order": uuid4(),
        "closed_order": uuid4(),
        "product": uuid4(),
        "point": uuid4(),
        "product_name": f"Guard methanol {label}",
        "fuel_type": "Methanol",
        "fuel_grade": f"Guard {label}",
        "point_name": f"Guard point {label}",
        "region": "Guard region",
    }
    await session.execute(
        text(
            "INSERT INTO products (id,name,fuel_type,fuel_grade,is_active) "
            "VALUES (:id,:name,:fuel_type,:fuel_grade,true)"
        ),
        {
            "id": market["product"],
            "name": market["product_name"],
            "fuel_type": market["fuel_type"],
            "fuel_grade": market["fuel_grade"],
        },
    )
    await session.execute(
        text(
            "INSERT INTO delivery_points (id,name,region,timezone,is_active) "
            "VALUES (:id,:name,:region,'UTC',true)"
        ),
        {"id": market["point"], "name": market["point_name"], "region": market["region"]},
    )
    await session.execute(
        text("INSERT INTO organizations (id,name,type) VALUES (:id,:name,'FUEL_BUYER')"),
        {"id": market["organization"], "name": f"Guard organization {label}"},
    )
    # Company approval is the trust decision that classifies it as REAL.
    provenance = (
        await session.execute(
            text(
                "UPDATE organizations SET verification_status='APPROVED' "
                "WHERE id=:id RETURNING provenance"
            ),
            {"id": market["organization"]},
        )
    ).scalar_one()
    assert provenance == "REAL"
    await session.execute(
        text(
            "INSERT INTO users (id,email,password_hash,role,status,organization_id,"
            "email_verified,must_change_password) "
            "VALUES (:id,:email,'unused','BUYER','APPROVED',:organization,true,false)"
        ),
        {
            "id": market["owner"],
            "email": f"guard-{label}@example.test",
            "organization": market["organization"],
        },
    )
    for order, status in (
        (market["public_order"], "OPEN"),
        (market["closed_order"], "CANCELLED"),
    ):
        await session.execute(
            text(
                "INSERT INTO orderbook_orders (id,organization_id,product_id,"
                "delivery_point_id,provenance,side,quantity_mt,remaining_quantity_mt,"
                "price_per_mt_usd,availability_window,status) VALUES (:id,:organization,"
                ":product,:point,'REAL','BID',10,10,500,'SPOT',:status)"
            ),
            {
                "id": order,
                "organization": market["organization"],
                "product": market["product"],
                "point": market["point"],
                "status": status,
            },
        )
    await session.commit()
    return {key: str(value) for key, value in market.items()}


@pytest.mark.asyncio
async def test_rust_watchlist_guard_component(pg_session, analytics_pg_url):
    _, session = pg_session
    app_url = _sqlx_url(os.environ["DATABASE_URL"])
    fixture_url = _sqlx_url(analytics_pg_url)
    app, owner = urlsplit(app_url), urlsplit(fixture_url)
    assert app.username != owner.username, "reads use the application role"
    assert (app.hostname, app.port, app.path) == (owner.hostname, owner.port, owner.path)
    market = await _seed_market(session)

    env = {
        **os.environ,
        "RELEASE_SHA": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
        "RUST_TEST_PYTHON": sys.executable,
        "SQLX_OFFLINE": "true",
        "WATCHLIST_GUARD_APP_URL": app_url,
        "WATCHLIST_GUARD_FIXTURE_URL": fixture_url,
        "WATCHLIST_GUARD_MARKET": json.dumps(market),
    }
    result = subprocess.run(
        [
            "cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked",
            "-p", "verdaxis-read", "--features", "contract-tests", "--test", TARGET,
            "--", "--include-ignored", "--test-threads=1",
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=1800,
        check=False,
    )
    print(result.stdout)
    print(result.stderr[-4000:])
    assert result.returncode == 0, f"cargo test exited {result.returncode}"
    passed = set(PASSED_LINE.findall(result.stdout))
    summary = SUMMARY_LINE.search(result.stdout)
    assert summary is not None, "no passing test summary"
    assert int(summary.group(1)) == len(passed)
    assert passed == OFFLINE_TESTS | DATABASE_TESTS
