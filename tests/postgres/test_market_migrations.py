"""Fresh/legacy-shaped migration and explicit quarantine proofs on PostgreSQL 17."""
from __future__ import annotations

import asyncio
import json
import os
import subprocess
import sys
from pathlib import Path
from uuid import UUID, uuid4

import pytest
from sqlalchemy import text
from sqlalchemy.engine import make_url
from sqlalchemy.ext.asyncio import create_async_engine


_BACKEND_ROOT = Path(__file__).resolve().parents[2]
_PARENT = "miq_20260720_market_quarantine"
_HEAD = "mi_20260720_market_integrity"
_SENTINEL = UUID("00000000-dead-beef-0000-aaa0e15eed01")
_DEMO_ORG = UUID("4da7b285-34ee-5443-9406-f96b4ed1a251")
_DEMO_SELLER_ORG = UUID("0dbce576-2026-5925-ab66-674d505e98ad")
_SINGAPORE_POINT = UUID("73835e92-820e-584b-8280-bb61c63aa28e")


def _command_env(database_url: str) -> dict[str, str]:
    return {
        "PATH": os.environ.get("PATH", ""),
        "DATABASE_URL": database_url,
    }


def _alembic(database_url: str, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "alembic", *arguments],
        cwd=_BACKEND_ROOT,
        env=_command_env(database_url),
        text=True,
        capture_output=True,
        check=False,
    )


def _remediation_cli(
    database_url: str,
    *arguments: str,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            "scripts/remediate_market_data.py",
            "--database-url",
            database_url,
            "--environment",
            "test",
            "--attestation",
            f"test:{make_url(database_url).database}",
            *arguments,
        ],
        cwd=_BACKEND_ROOT,
        env={"PATH": os.environ.get("PATH", "")},
        text=True,
        capture_output=True,
        check=False,
    )


async def _database_execute(database_url: str, statement: str, parameters=None):
    engine = create_async_engine(database_url)
    try:
        async with engine.begin() as connection:
            return await connection.execute(text(statement), parameters or {})
    finally:
        await engine.dispose()


async def _seed_parent_sentinel(
    database_url: str,
    *,
    organization_id,
    product_id,
) -> None:
    engine = create_async_engine(database_url)
    try:
        async with engine.begin() as connection:
            await connection.execute(
                text(
                    "INSERT INTO organizations (id, name, type, verification_status) "
                    "VALUES (:organization_id, 'Legacy staging supplier', "
                    "'FUEL_SUPPLIER', 'PENDING')"
                ),
                {"organization_id": organization_id},
            )
            await connection.execute(
                text(
                    "INSERT INTO organizations (id, name, type, verification_status) "
                    "VALUES (:demo_org_id, 'Peninsula Petroleum', "
                    "'FUEL_BUYER', 'APPROVED')"
                ),
                {"demo_org_id": _DEMO_ORG},
            )
            await connection.execute(
                text(
                    "INSERT INTO orderbook_orders ("
                    "id, organization_id, side, quantity_mt, "
                    "remaining_quantity_mt, price_per_mt_usd, product_id, "
                    "availability_window, status) VALUES ("
                    ":sentinel_id, :organization_id, 'ASK', 0, 0, 0, "
                    ":product_id, 'SPOT', 'CANCELLED')"
                ),
                {
                    "sentinel_id": _SENTINEL,
                    "organization_id": organization_id,
                    "product_id": product_id,
                },
            )
    finally:
        await engine.dispose()


async def _seed_parent_accepted_rfq_graph(database_url: str):
    rfq_id, quote_id, trade_id = uuid4(), uuid4(), uuid4()
    engine = create_async_engine(database_url)
    try:
        async with engine.begin() as connection:
            product_id = (
                await connection.execute(
                    text("SELECT id FROM products WHERE name = 'Bio Methanol'")
                )
            ).scalar_one()
            await connection.execute(
                text(
                    "INSERT INTO delivery_points "
                    "(id, name, region, timezone, is_active) VALUES "
                    "(:id, 'Singapore', 'Asia', 'Asia/Singapore', true)"
                ),
                {"id": _SINGAPORE_POINT},
            )
            point_id = _SINGAPORE_POINT
            await connection.execute(
                text(
                    "INSERT INTO organizations (id, name, type, verification_status) VALUES "
                    "(:buyer, 'Legacy deterministic demo buyer', 'FUEL_BUYER', 'APPROVED'), "
                    "(:seller, 'Legacy deterministic demo seller', 'FUEL_SUPPLIER', 'APPROVED')"
                ),
                {"buyer": _DEMO_ORG, "seller": _DEMO_SELLER_ORG},
            )
            await connection.execute(
                text(
                    "INSERT INTO rfqs (id, buyer_org_id, product_id, delivery_point_id, "
                    "quantity_mt, availability_window, status, expires_at) VALUES "
                    "(:rfq, :buyer, :product, :point, 25, 'SPOT', 'ACCEPTED', "
                    "now() + interval '1 day')"
                ),
                {
                    "rfq": rfq_id,
                    "buyer": _DEMO_ORG,
                    "product": product_id,
                    "point": point_id,
                },
            )
            await connection.execute(
                text(
                    "INSERT INTO rfq_quotes (id, rfq_id, seller_org_id, "
                    "price_per_mt_usd, status) VALUES "
                    "(:quote, :rfq, :seller, 700, 'ACCEPTED')"
                ),
                {"quote": quote_id, "rfq": rfq_id, "seller": _DEMO_SELLER_ORG},
            )
            await connection.execute(
                text(
                    "INSERT INTO trades (id, buyer_id, seller_id, initiated_by, "
                    "quantity_mt, price_per_mt_usd, status, confirmed_at) VALUES "
                    "(:trade, :buyer, :seller, 'BUYER', 25, 700, 'CONFIRMED', now())"
                ),
                {
                    "trade": trade_id,
                    "buyer": _DEMO_ORG,
                    "seller": _DEMO_SELLER_ORG,
                },
            )
    finally:
        await engine.dispose()
    return rfq_id, quote_id, trade_id


@pytest.fixture
async def migration_database(market_pg_url: str):
    source_url = make_url(market_pg_url)
    database_name = f"market_{uuid4().hex[:10]}_market_integrity_test"
    database_url = source_url.set(database=database_name).render_as_string(
        hide_password=False
    )
    # Integration note: the enforced role policy keeps every verdaxis role
    # NOCREATEDB, so the throwaway database is created through the admin URL
    # and owned by the migrator that runs the migration proofs.
    admin_url = (
        make_url(os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"])
        .set(database="postgres")
        .render_as_string(hide_password=False)
    )
    admin_engine = create_async_engine(admin_url, isolation_level="AUTOCOMMIT")
    try:
        async with admin_engine.connect() as connection:
            await connection.execute(
                text(
                    f'CREATE DATABASE "{database_name}" '
                    f'OWNER "{source_url.username}"'
                )
            )
        yield database_url, database_name
    finally:
        async with admin_engine.connect() as connection:
            await connection.execute(
                text(
                    "SELECT pg_terminate_backend(pid) FROM pg_stat_activity "
                    "WHERE datname = :database_name AND pid <> pg_backend_pid()"
                ),
                {"database_name": database_name},
            )
            await connection.execute(text(f'DROP DATABASE IF EXISTS "{database_name}"'))
        await admin_engine.dispose()


@pytest.mark.asyncio
async def test_fresh_database_upgrades_and_checks_without_application_secrets(
    migration_database,
):
    database_url, _database_name = migration_database
    heads = await asyncio.to_thread(_alembic, database_url, "heads")
    assert heads.returncode == 0, heads.stderr
    assert _HEAD in heads.stdout

    upgraded = await asyncio.to_thread(_alembic, database_url, "upgrade", "head")
    assert upgraded.returncode == 0, upgraded.stderr
    current = await asyncio.to_thread(_alembic, database_url, "current")
    assert current.returncode == 0, current.stderr
    assert _HEAD in current.stdout
    checked = await asyncio.to_thread(_alembic, database_url, "check")
    assert checked.returncode == 0, f"{checked.stdout}\n{checked.stderr}"


@pytest.mark.asyncio
async def test_parent_shape_sentinel_requires_explicit_cli_quarantine_then_upgrades(
    migration_database,
):
    database_url, database_name = migration_database
    parent = await asyncio.to_thread(_alembic, database_url, "upgrade", _PARENT)
    assert parent.returncode == 0, parent.stderr

    organization_id = uuid4()
    parent_product = await _database_execute(
        database_url,
        "SELECT id FROM products WHERE name = 'Bio Methanol'",
    )
    product_id = parent_product.scalar_one()
    await _seed_parent_sentinel(
        database_url,
        organization_id=organization_id,
        product_id=product_id,
    )

    missing_legacy_unique = await _database_execute(
        database_url,
        "SELECT count(*) FROM pg_constraint "
        "WHERE conname = 'uq_trades_party_idempotency_key'",
    )
    assert missing_legacy_unique.scalar_one() == 0

    refused = await asyncio.to_thread(_alembic, database_url, "upgrade", "head")
    refusal_output = f"{refused.stdout}\n{refused.stderr}"
    assert refused.returncode != 0
    assert str(_SENTINEL) in refusal_output
    assert "explicit-ID quarantine" in refusal_output

    dry_run = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "quarantine",
        "--order-id",
        str(_SENTINEL),
    )
    assert dry_run.returncode == 0, dry_run.stderr
    assert json.loads(dry_run.stdout)["dry_run"] is True
    still_present = await _database_execute(
        database_url,
        "SELECT count(*) FROM orderbook_orders WHERE id = :sentinel_id",
        {"sentinel_id": _SENTINEL},
    )
    assert still_present.scalar_one() == 1

    applied = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "--operator",
        "market-integrity-test",
        "--reason",
        "approved exact sentinel quarantine proof",
        "--reference",
        "TEST-MI-001",
        "--apply",
        "quarantine",
        "--order-id",
        str(_SENTINEL),
    )
    assert applied.returncode == 0, applied.stderr
    assert json.loads(applied.stdout)["dry_run"] is False
    archive = await _database_execute(
        database_url,
        "SELECT reason, operator, reference, original_row->>'id' AS original_id "
        "FROM market_row_quarantines WHERE source_id = :sentinel_id",
        {"sentinel_id": _SENTINEL},
    )
    archived = archive.mappings().one()
    assert archived["original_id"] == str(_SENTINEL)
    assert archived["operator"] == "market-integrity-test"
    assert archived["reference"] == "TEST-MI-001"

    upgraded = await asyncio.to_thread(_alembic, database_url, "upgrade", "head")
    assert upgraded.returncode == 0, upgraded.stderr

    rename_dry_run = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "rename-demo-organizations",
    )
    assert rename_dry_run.returncode == 0, rename_dry_run.stderr
    change = json.loads(rename_dry_run.stdout)["changes"][0]
    assert change["id"] == str(_DEMO_ORG)
    assert change["before"] == "Peninsula Petroleum"
    assert change["after"] == "Verdaxis Demo Buyer 01"

    renamed = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "--operator",
        "market-integrity-test",
        "--reason",
        "replace deterministic demo legal identity",
        "--reference",
        "TEST-MI-002",
        "--apply",
        "rename-demo-organizations",
    )
    assert renamed.returncode == 0, renamed.stderr
    demo_name = await _database_execute(
        database_url,
        "SELECT name FROM organizations WHERE id = :demo_org_id",
        {"demo_org_id": _DEMO_ORG},
    )
    assert demo_name.scalar_one() == "Verdaxis Demo Buyer 01"

    downgrade = await asyncio.to_thread(_alembic, database_url, "downgrade", _PARENT)
    assert downgrade.returncode != 0
    assert "downgrade is unsupported" in f"{downgrade.stdout}\n{downgrade.stderr}"
    current = await asyncio.to_thread(_alembic, database_url, "current")
    assert current.returncode == 0, current.stderr
    assert _HEAD in current.stdout

    checked = await asyncio.to_thread(_alembic, database_url, "check")
    assert checked.returncode == 0, f"{checked.stdout}\n{checked.stderr}"


@pytest.mark.asyncio
async def test_accepted_rfq_requires_approved_exact_graph_quarantine_before_upgrade(
    migration_database,
):
    database_url, _database_name = migration_database
    parent = await asyncio.to_thread(_alembic, database_url, "upgrade", _PARENT)
    assert parent.returncode == 0, parent.stderr
    rfq_id, quote_id, trade_id = await _seed_parent_accepted_rfq_graph(database_url)

    refused_upgrade = await asyncio.to_thread(_alembic, database_url, "upgrade", "head")
    assert refused_upgrade.returncode != 0
    assert str(rfq_id) in f"{refused_upgrade.stdout}\n{refused_upgrade.stderr}"
    assert "explicit remediation" in f"{refused_upgrade.stdout}\n{refused_upgrade.stderr}"

    dry_run = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "quarantine-accepted-rfqs",
        "--rfq-id",
        str(rfq_id),
        "--rfq-trade",
        f"{rfq_id}={trade_id}",
    )
    assert dry_run.returncode == 0, dry_run.stderr
    dry_payload = json.loads(dry_run.stdout)
    assert dry_payload["dry_run"] is True
    assert dry_payload["accepted_rfqs"][0]["accepted_quote_id"] == str(quote_id)
    assert dry_payload["accepted_rfqs"][0]["selected_trade_id"] == str(trade_id)

    invalid_binding = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "--operator",
        "market-integrity-test",
        "--reason",
        "approval-gated accepted RFQ quarantine proof",
        "--reference",
        "TEST-RFQ-APPROVAL",
        "--apply",
        "quarantine-accepted-rfqs",
        "--rfq-id",
        str(rfq_id),
        "--rfq-trade",
        f"{rfq_id}={uuid4()}",
        "--accepted-rfq-approval-reference",
        "TEST-RFQ-APPROVAL",
    )
    assert invalid_binding.returncode != 0
    assert "ambiguous, unbound, or has unsupported dependencies" in invalid_binding.stderr
    still_present = await _database_execute(
        database_url,
        "SELECT count(*) FROM rfqs WHERE id = :rfq_id",
        {"rfq_id": rfq_id},
    )
    assert still_present.scalar_one() == 1

    missing_approval = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "--operator",
        "market-integrity-test",
        "--reason",
        "approval-gated accepted RFQ quarantine proof",
        "--reference",
        "TEST-RFQ-APPROVAL",
        "--apply",
        "quarantine-accepted-rfqs",
        "--rfq-id",
        str(rfq_id),
        "--rfq-trade",
        f"{rfq_id}={trade_id}",
    )
    assert missing_approval.returncode != 0
    assert "product approval" in missing_approval.stderr

    applied = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "--operator",
        "market-integrity-test",
        "--reason",
        "approval-gated accepted RFQ quarantine proof",
        "--reference",
        "TEST-RFQ-APPROVAL",
        "--apply",
        "quarantine-accepted-rfqs",
        "--rfq-id",
        str(rfq_id),
        "--rfq-trade",
        f"{rfq_id}={trade_id}",
        "--accepted-rfq-approval-reference",
        "TEST-RFQ-APPROVAL",
    )
    assert applied.returncode == 0, applied.stderr
    archived = await _database_execute(
        database_url,
        "SELECT source_table, source_id, reference FROM market_row_quarantines "
        "WHERE source_id = ANY(CAST(:ids AS uuid[])) ORDER BY source_table",
        {"ids": [str(rfq_id), str(quote_id), str(trade_id)]},
    )
    rows = archived.mappings().all()
    assert {(row["source_table"], row["source_id"]) for row in rows} == {
        ("rfqs", rfq_id),
        ("rfq_quotes", quote_id),
        ("trades", trade_id),
    }
    assert {row["reference"] for row in rows} == {"TEST-RFQ-APPROVAL"}
    active_count = await _database_execute(
        database_url,
        "SELECT (SELECT count(*) FROM rfqs WHERE id = :rfq) + "
        "(SELECT count(*) FROM rfq_quotes WHERE id = :quote) + "
        "(SELECT count(*) FROM trades WHERE id = :trade)",
        {"rfq": rfq_id, "quote": quote_id, "trade": trade_id},
    )
    assert active_count.scalar_one() == 0

    upgraded = await asyncio.to_thread(_alembic, database_url, "upgrade", "head")
    assert upgraded.returncode == 0, upgraded.stderr
