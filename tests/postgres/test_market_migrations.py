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
# Later product migrations extend the linearized market chain. The mi-specific
# refusal/quarantine semantics exercised below are unchanged.
_HEAD = "oa_20260910_auto_real_orgs"
_SENTINEL = UUID("00000000-dead-beef-0000-aaa0e15eed01")
_DEMO_ORG = UUID("4da7b285-34ee-5443-9406-f96b4ed1a251")
_DEMO_SELLER_ORG = UUID("0dbce576-2026-5925-ab66-674d505e98ad")
_SINGAPORE_POINT = UUID("73835e92-820e-584b-8280-bb61c63aa28e")


def _command_env(database_url: str) -> dict[str, str]:
    return {
        "PATH": os.environ.get("PATH", ""),
        "DATABASE_URL": database_url,
        "MIGRATOR_DATABASE_URL": database_url,
        "ENVIRONMENT": "test",
        "RELEASE_SHA": "test",
        "JWT_SECRET": "test-secret-key-that-is-at-least-32-characters-long",
        "BACKEND_CORS_ORIGINS": "[]",
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


async def _seed_real_organization_candidate(
    database_url: str,
    *,
    organization_id,
    user_id,
    suffix: str,
) -> None:
    engine = create_async_engine(database_url)
    try:
        async with engine.begin() as connection:
            await connection.execute(
                text(
                    "INSERT INTO organizations "
                    "(id, name, type, verification_status) VALUES "
                    "(:organization_id, :name, 'FUEL_BUYER', 'PENDING')"
                ),
                {
                    "organization_id": organization_id,
                    "name": f"Real market candidate {suffix}",
                },
            )
            await connection.execute(
                text(
                    "INSERT INTO users "
                    "(id, email, password_hash, role, status, email_verified, "
                    "must_change_password, organization_id) VALUES "
                    "(:user_id, :email, 'not-a-real-hash', 'BUYER', "
                    "'APPROVED', true, false, :organization_id)"
                ),
                {
                    "user_id": user_id,
                    "email": f"real-market-{suffix}@example.invalid",
                    "organization_id": organization_id,
                },
            )
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
async def test_invalid_legacy_synthetic_order_requires_reviewed_expiry_snapshot(
    migration_database,
):
    database_url, _database_name = migration_database
    parent = await asyncio.to_thread(_alembic, database_url, "upgrade", _PARENT)
    assert parent.returncode == 0, parent.stderr
    order_id = uuid4()
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
                    "INSERT INTO organizations "
                    "(id, name, type, verification_status) VALUES "
                    "(:id, 'Legacy deterministic demo buyer', "
                    "'FUEL_BUYER', 'APPROVED')"
                ),
                {"id": _DEMO_ORG},
            )
            await connection.execute(
                text(
                    "INSERT INTO orderbook_orders "
                    "(id, organization_id, side, quantity_mt, "
                    "remaining_quantity_mt, price_per_mt_usd, product_id, "
                    "availability_window, status) VALUES "
                    "(:id, :organization_id, 'BID', 500, 500, 650, "
                    ":product_id, 'SPOT', 'OPEN')"
                ),
                {
                    "id": order_id,
                    "organization_id": _DEMO_ORG,
                    "product_id": product_id,
                },
            )
    finally:
        await engine.dispose()

    dry_run = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "expire-invalid-legacy-synthetic-orders",
    )
    assert dry_run.returncode == 0, dry_run.stderr
    report = json.loads(dry_run.stdout)["synthetic_order_expiry"]
    assert report["matching_count"] == 1
    assert report["applied"] is False

    applied = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "--operator",
        "market-integrity-test",
        "--reason",
        "expire reviewed malformed synthetic order",
        "--reference",
        "TEST-SYNTHETIC-EXPIRY",
        "--apply",
        "expire-invalid-legacy-synthetic-orders",
        "--expected-snapshot",
        report["snapshot_sha256"],
    )
    assert applied.returncode == 0, applied.stderr
    state = await _database_execute(
        database_url,
        "SELECT status, expires_at IS NOT NULL FROM orderbook_orders WHERE id = :id",
        {"id": order_id},
    )
    assert state.one() == ("EXPIRED", True)
    archive = await _database_execute(
        database_url,
        "SELECT source_table, reference FROM market_row_quarantines "
        "WHERE source_id = :id",
        {"id": order_id},
    )
    assert archive.one() == ("orderbook_orders_expired", "TEST-SYNTHETIC-EXPIRY")

    upgraded = await asyncio.to_thread(_alembic, database_url, "upgrade", "head")
    assert upgraded.returncode == 0, upgraded.stderr


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


@pytest.mark.asyncio
async def test_accepted_rfq_without_trade_requires_explicit_no_trade_attestation(
    migration_database,
):
    database_url, _database_name = migration_database
    parent = await asyncio.to_thread(_alembic, database_url, "upgrade", _PARENT)
    assert parent.returncode == 0, parent.stderr
    rfq_id, quote_id, trade_id = await _seed_parent_accepted_rfq_graph(database_url)
    await _database_execute(
        database_url,
        "DELETE FROM trades WHERE id = :trade_id",
        {"trade_id": trade_id},
    )

    refused = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "--operator",
        "market-integrity-test",
        "--reason",
        "explicit no-trade accepted RFQ quarantine proof",
        "--reference",
        "TEST-RFQ-NO-TRADE",
        "--apply",
        "quarantine-accepted-rfqs",
        "--rfq-id",
        str(rfq_id),
        "--accepted-rfq-approval-reference",
        "TEST-RFQ-NO-TRADE",
    )
    assert refused.returncode != 0
    assert "explicit no-trade" in refused.stderr

    applied = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "--operator",
        "market-integrity-test",
        "--reason",
        "explicit no-trade accepted RFQ quarantine proof",
        "--reference",
        "TEST-RFQ-NO-TRADE",
        "--apply",
        "quarantine-accepted-rfqs",
        "--rfq-id",
        str(rfq_id),
        "--rfq-no-trade",
        str(rfq_id),
        "--accepted-rfq-approval-reference",
        "TEST-RFQ-NO-TRADE",
    )
    assert applied.returncode == 0, applied.stderr
    archived = await _database_execute(
        database_url,
        "SELECT source_table FROM market_row_quarantines "
        "WHERE source_id = ANY(CAST(:ids AS uuid[])) ORDER BY source_table",
        {"ids": [str(rfq_id), str(quote_id)]},
    )
    assert archived.scalars().all() == ["rfq_quotes", "rfqs"]


@pytest.mark.asyncio
async def test_exact_operator_approval_promotes_only_eligible_real_organizations(
    migration_database,
):
    database_url, _database_name = migration_database
    parent = await asyncio.to_thread(_alembic, database_url, "upgrade", _PARENT)
    assert parent.returncode == 0, parent.stderr

    organization_id, user_id = uuid4(), uuid4()
    await _seed_real_organization_candidate(
        database_url,
        organization_id=organization_id,
        user_id=user_id,
        suffix="before-integrity",
    )

    dry_run = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "approve-real-organizations",
        "--organization-id",
        str(organization_id),
    )
    assert dry_run.returncode == 0, dry_run.stderr
    dry_payload = json.loads(dry_run.stdout)
    assert dry_payload["dry_run"] is True
    dry_organization = dry_payload["organizations"][0]
    assert dry_organization["already_approved"] is False
    assert dry_organization["eligible_trader_count"] == 1
    assert dry_organization["eligible_user_ids"] == [str(user_id)]
    assert dry_organization["organization_id"] == str(organization_id)
    assert dry_organization["previous_verification_status"] == "PENDING"
    assert dry_organization["provenance"] is None
    assert len(dry_organization["snapshot_sha256"]) == 64
    assert dry_organization["reviewed_snapshot"]["eligible_user_ids"] == [str(user_id)]

    stale_hash = "0" * 64
    stale_apply = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "--operator",
        "market-integrity-test",
        "--reason",
        "stale review must be rejected",
        "--reference",
        "TEST-ORG-REAL-STALE",
        "--apply",
        "approve-real-organizations",
        "--organization-id",
        str(organization_id),
        "--expected-snapshot",
        f"{organization_id}={stale_hash}",
    )
    assert stale_apply.returncode != 0
    assert "snapshot changed after dry-run review" in stale_apply.stderr

    applied = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "--operator",
        "market-integrity-test",
        "--reason",
        "approved verified trader grandfathering proof",
        "--reference",
        "TEST-ORG-REAL-001",
        "--apply",
        "approve-real-organizations",
        "--organization-id",
        str(organization_id),
        "--expected-snapshot",
        f"{organization_id}={dry_organization['snapshot_sha256']}",
    )
    assert applied.returncode == 0, applied.stderr
    applied_payload = json.loads(applied.stdout)
    assert applied_payload["dry_run"] is False

    approval = await _database_execute(
        database_url,
        "SELECT previous_verification_status, operator, reason, reference, "
        "reviewed_snapshot FROM organization_market_approvals "
        "WHERE organization_id = :organization_id",
        {"organization_id": organization_id},
    )
    approval_row = approval.mappings().one()
    assert approval_row["previous_verification_status"] == "PENDING"
    assert approval_row["operator"] == "market-integrity-test"
    assert approval_row["reference"] == "TEST-ORG-REAL-001"
    assert approval_row["reviewed_snapshot"] == dry_organization["reviewed_snapshot"]

    audit = await _database_execute(
        database_url,
        "SELECT action, resource_id, changes->'verification_status'->>'to' AS status "
        "FROM audit_logs WHERE action = 'MARKET_ORGANIZATION_APPROVED' "
        "AND resource_id = :resource_id",
        {"resource_id": str(organization_id)},
    )
    assert audit.one() == (
        "MARKET_ORGANIZATION_APPROVED",
        str(organization_id),
        "APPROVED",
    )

    await _database_execute(
        database_url,
        "UPDATE users SET email_verified = false WHERE id = :user_id",
        {"user_id": user_id},
    )
    drifted = await asyncio.to_thread(_alembic, database_url, "upgrade", "head")
    assert drifted.returncode != 0
    assert "organization approval snapshot drift" in (
        f"{drifted.stdout}\n{drifted.stderr}"
    )
    await _database_execute(
        database_url,
        "UPDATE users SET email_verified = true WHERE id = :user_id",
        {"user_id": user_id},
    )

    upgraded = await asyncio.to_thread(_alembic, database_url, "upgrade", "head")
    assert upgraded.returncode == 0, upgraded.stderr
    promoted = await _database_execute(
        database_url,
        "SELECT verification_status, provenance FROM organizations "
        "WHERE id = :organization_id",
        {"organization_id": organization_id},
    )
    assert promoted.one() == ("APPROVED", "REAL")

    future_organization_id, future_user_id = uuid4(), uuid4()
    await _seed_real_organization_candidate(
        database_url,
        organization_id=future_organization_id,
        user_id=future_user_id,
        suffix="after-integrity",
    )
    active_unknown_order_id = uuid4()
    await _database_execute(
        database_url,
        "INSERT INTO orderbook_orders "
        "(id, organization_id, owner_user_id, provenance, side, product_id, "
        "quantity_mt, remaining_quantity_mt, price_per_mt_usd, "
        "availability_window, status) SELECT :order_id, :organization_id, "
        ":user_id, 'UNKNOWN', 'BID', id, 10, 10, 500, 'SPOT', 'OPEN' "
        "FROM products WHERE name = 'Bio Methanol'",
        {
            "order_id": active_unknown_order_id,
            "organization_id": future_organization_id,
            "user_id": future_user_id,
        },
    )
    active_unknown_refusal = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "approve-real-organizations",
        "--organization-id",
        str(future_organization_id),
    )
    assert active_unknown_refusal.returncode != 0
    assert "nonterminal UNKNOWN orders" in active_unknown_refusal.stderr
    await _database_execute(
        database_url,
        "UPDATE orderbook_orders SET status = 'FILLED', remaining_quantity_mt = 0 "
        "WHERE id = :order_id",
        {"order_id": active_unknown_order_id},
    )
    filled_unknown_refusal = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "approve-real-organizations",
        "--organization-id",
        str(future_organization_id),
    )
    assert filled_unknown_refusal.returncode != 0
    assert "nonterminal UNKNOWN orders" in filled_unknown_refusal.stderr
    await _database_execute(
        database_url,
        "UPDATE orderbook_orders SET status = 'CANCELLED' WHERE id = :order_id",
        {"order_id": active_unknown_order_id},
    )
    future_dry_run = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "approve-real-organizations",
        "--organization-id",
        str(future_organization_id),
    )
    assert future_dry_run.returncode == 0, future_dry_run.stderr
    future_hash = json.loads(future_dry_run.stdout)["organizations"][0][
        "snapshot_sha256"
    ]
    future = await asyncio.to_thread(
        _remediation_cli,
        database_url,
        "--operator",
        "market-integrity-test",
        "--reason",
        "future real participant approval proof",
        "--reference",
        "TEST-ORG-REAL-002",
        "--apply",
        "approve-real-organizations",
        "--organization-id",
        str(future_organization_id),
        "--expected-snapshot",
        f"{future_organization_id}={future_hash}",
    )
    assert future.returncode == 0, future.stderr
    future_promoted = await _database_execute(
        database_url,
        "SELECT verification_status, provenance FROM organizations "
        "WHERE id = :organization_id",
        {"organization_id": future_organization_id},
    )
    assert future_promoted.one() == ("APPROVED", "REAL")


@pytest.mark.asyncio
async def test_automatic_org_classification_downgrade_restores_prior_guard(migration_database):
    database_url, _ = migration_database
    previous = "ai_20260831_invite_real_orgs"
    upgraded = await asyncio.to_thread(_alembic, database_url, "upgrade", previous)
    assert upgraded.returncode == 0, upgraded.stderr
    definition_sql = "SELECT pg_get_functiondef('verdaxis_immutable_org_provenance()'::regprocedure)"
    original = (await _database_execute(database_url, definition_sql)).scalar_one()

    upgraded = await asyncio.to_thread(_alembic, database_url, "upgrade", _HEAD)
    assert upgraded.returncode == 0, upgraded.stderr
    organization_id = uuid4()
    await _database_execute(
        database_url,
        "INSERT INTO organizations (id, name, type) VALUES (:id, 'Approval roundtrip', 'FUEL_BUYER')",
        {"id": organization_id},
    )
    result = await _database_execute(
        database_url,
        "UPDATE organizations SET verification_status='APPROVED' WHERE id=:id RETURNING provenance",
        {"id": organization_id},
    )
    assert result.scalar_one() == "REAL"
    downgraded = await asyncio.to_thread(_alembic, database_url, "downgrade", previous)
    assert downgraded.returncode == 0, downgraded.stderr
    restored = (await _database_execute(database_url, definition_sql)).scalar_one()
    assert " ".join(restored.split()) == " ".join(original.split())
    result = await _database_execute(
        database_url, "SELECT provenance FROM organizations WHERE id=:id", {"id": organization_id},
    )
    assert result.scalar_one() == "REAL"
