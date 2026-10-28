"""Supplier-offer rollback preserves history and restores the prior RFQ schema."""
from __future__ import annotations

import os
from pathlib import Path
import re
import subprocess
import sys
from uuid import uuid4

import pytest
from sqlalchemy.engine import make_url


_BACKEND_ROOT = Path(__file__).resolve().parents[2]
_PREVIOUS_REVISION = "fame_20260922_rfq_contract"
_SUPPLIER_REVISION = "fame_20260922_supplier_offers"


@pytest.fixture
def supplier_scratch_database():
    admin_url = os.environ.get("POSTGRES_ADMIN_TEST_DATABASE_URL")
    migration_url = os.environ.get("MARKET_INTEGRITY_TEST_DATABASE_URL")
    migrator_role = os.environ.get("RUNTIME_TEST_MIGRATOR_ROLE")
    if not all((admin_url, migration_url, migrator_role)):
        pytest.skip("supplier migration roundtrip requires disposable PostgreSQL roles")
    assert re.fullmatch(r"[a-z_][a-z0-9_]*", migrator_role)
    admin = make_url(admin_url)
    database = f"verdaxis_supplier_{uuid4().hex[:10]}_market_integrity_test"
    url = make_url(migration_url).set(database=database).render_as_string(hide_password=False)

    def sql(statement: str, *, db: str = database) -> str:
        result = subprocess.run(
            [
                "psql", "-X", "-h", admin.host or "", "-p", str(admin.port or 5432),
                "-U", admin.username or "", "-d", db, "-v", "ON_ERROR_STOP=1",
                "-At", "-c", statement,
            ],
            env={**os.environ, "PGPASSWORD": admin.password or ""},
            capture_output=True, text=True, check=False,
        )
        assert result.returncode == 0, result.stderr
        return result.stdout.strip()

    def migrate(direction: str, revision: str, *, success: bool = True) -> str:
        result = subprocess.run(
            [sys.executable, "-m", "alembic", direction, revision],
            cwd=_BACKEND_ROOT,
            env={**os.environ, "DATABASE_URL": url, "MIGRATOR_DATABASE_URL": url},
            capture_output=True, text=True, check=False,
        )
        output = result.stdout + result.stderr
        assert (result.returncode == 0) is success, output
        return output

    sql(f"CREATE DATABASE {database} OWNER {migrator_role}", db="postgres")
    try:
        sql("CREATE EXTENSION IF NOT EXISTS postgis")
        yield sql, migrate
    finally:
        sql(f"DROP DATABASE {database} WITH (FORCE)", db="postgres")


def test_supplier_history_blocks_downgrade_and_empty_rollback_restores_schema(
    supplier_scratch_database,
):
    sql, migrate = supplier_scratch_database
    migrate("upgrade", _PREVIOUS_REVISION)
    rfq_schema_sql = """
        SELECT json_agg(row_to_json(c) ORDER BY ordinal_position)
        FROM (
            SELECT column_name, data_type, is_nullable, column_default, ordinal_position
            FROM information_schema.columns
            WHERE table_schema = 'public' AND table_name = 'rfqs'
        ) c
    """
    previous_rfq_schema = sql(rfq_schema_sql)
    sql("""
        INSERT INTO organizations (id, name, type, provenance)
        VALUES ('88888888-0000-0000-0000-000000000001', 'Rollback supplier', 'FUEL_SUPPLIER', 'REAL');
        INSERT INTO users (id, email, password_hash, role, organization_id)
        VALUES ('88888888-0000-0000-0000-000000000002', 'rollback-supplier@example.test',
                'unused', 'SUPPLIER', '88888888-0000-0000-0000-000000000001');
        INSERT INTO delivery_points (id, name, region, timezone, is_active)
        VALUES ('73835e92-820e-584b-8280-bb61c63aa28e', 'Singapore', 'Asia', 'Asia/Singapore', true)
        ON CONFLICT DO NOTHING;
        INSERT INTO rfqs (id, buyer_org_id, product_id, delivery_point_id, quantity_mt,
                          availability_window, is_anonymous, status, expires_at, created_at, contract_terms)
        VALUES ('88888888-0000-0000-0000-000000000003', '88888888-0000-0000-0000-000000000001',
                'e561e43f-d9b2-598e-981c-f1d28d515ddc', '73835e92-820e-584b-8280-bb61c63aa28e',
                100, 'SPOT', false, 'OPEN', now() + interval '1 day', now(), '{"schema_version":1}');
    """)
    migrate("upgrade", _SUPPLIER_REVISION)
    sql("""
        INSERT INTO supplier_offers (id, supplier_org_id, supplier_user_id, product_id,
            delivery_point_id, quantity_mt, min_fill_mt, price_per_mt_usd, availability_window,
            listing_terms, expires_at, created_at, updated_at)
        VALUES ('88888888-0000-0000-0000-000000000004', '88888888-0000-0000-0000-000000000001',
            '88888888-0000-0000-0000-000000000002', 'e561e43f-d9b2-598e-981c-f1d28d515ddc',
            '73835e92-820e-584b-8280-bb61c63aa28e', 100, 20, 1050, 'SPOT',
            '{"batch_reference":"rollback-proof"}', now() + interval '1 day', now(), now());
    """)
    refusal = migrate("downgrade", _PREVIOUS_REVISION, success=False)
    assert "Cannot remove supplier offers while offer or targeted RFQ history exists" in refusal
    assert sql("SELECT version_num FROM alembic_version") == _SUPPLIER_REVISION
    assert sql("SELECT listing_terms->>'batch_reference' FROM supplier_offers") == "rollback-proof"

    sql("""
        INSERT INTO rfqs (id, buyer_org_id, product_id, delivery_point_id, quantity_mt,
            availability_window, is_anonymous, status, expires_at, created_at,
            source_offer_id, target_supplier_org_id, source_offer_snapshot)
        VALUES ('88888888-0000-0000-0000-000000000005', '88888888-0000-0000-0000-000000000001',
            'e561e43f-d9b2-598e-981c-f1d28d515ddc', '73835e92-820e-584b-8280-bb61c63aa28e',
            20, 'SPOT', false, 'OPEN', now() + interval '1 day', now(),
            '88888888-0000-0000-0000-000000000004', '88888888-0000-0000-0000-000000000001',
            '{"revision":1,"batch_reference":"rollback-proof"}');
        UPDATE supplier_offers SET status = 'WITHDRAWN', revision = 2;
    """)
    migrate("downgrade", _PREVIOUS_REVISION, success=False)
    assert sql("SELECT version_num FROM alembic_version") == _SUPPLIER_REVISION
    assert sql("SELECT source_offer_snapshot->>'revision' FROM rfqs WHERE source_offer_id IS NOT NULL") == "1"
    assert sql("SELECT status || ':' || revision FROM supplier_offers") == "WITHDRAWN:2"

    sql("DELETE FROM rfqs WHERE source_offer_id IS NOT NULL; DELETE FROM supplier_offers")
    migrate("downgrade", _PREVIOUS_REVISION)
    assert sql(rfq_schema_sql) == previous_rfq_schema
    assert sql("SELECT to_regclass('public.supplier_offers') IS NULL") == "t"
    assert sql("SELECT contract_terms->>'schema_version' FROM rfqs") == "1"
    migrate("upgrade", _SUPPLIER_REVISION)
    assert sql("SELECT version_num FROM alembic_version") == _SUPPLIER_REVISION
    assert sql("SELECT count(*) FROM supplier_offers") == "0"
    assert sql("SELECT contract_terms->>'schema_version' FROM rfqs") == "1"
