"""Disposable migration snapshot, immutable deadline and downgrade proof."""
from tests.postgres.test_supplier_offer_migration_roundtrip import supplier_scratch_database


def test_kyc_grace_snapshot_and_roundtrip(supplier_scratch_database):
    sql, migrate = supplier_scratch_database
    parent = "rcp_20261003_command_results"
    migrate("upgrade", parent)
    sql("""
        INSERT INTO organizations (id, name, type, provenance, verification_status)
        VALUES ('99999999-0000-0000-0000-000000000001', 'KYC grace supplier', 'FUEL_SUPPLIER', 'REAL', 'APPROVED');
        INSERT INTO users (id, email, password_hash, role, organization_id, status, kyc_status)
        VALUES ('99999999-0000-0000-0000-000000000002', 'kyc-grace@example.test', 'unused',
                'SUPPLIER', '99999999-0000-0000-0000-000000000001', 'APPROVED', 'PENDING');
    """)
    migrate("upgrade", "kycg_20261014_staging")
    assert sql("SELECT count(*) FROM kyc_execution_grace") == "1"
    assert sql("SELECT extract(epoch FROM expires_at - enrolled_at) = 1209600 FROM kyc_execution_grace") == "t"
    deadline = sql("SELECT expires_at FROM kyc_execution_grace")
    sql("UPDATE users SET kyc_status = 'SUBMITTED'")
    assert sql("SELECT expires_at FROM kyc_execution_grace") == deadline
    sql("""
        INSERT INTO users (id, email, password_hash, role, organization_id, status, kyc_status)
        VALUES ('99999999-0000-0000-0000-000000000003', 'kyc-later@example.test', 'unused',
                'SUPPLIER', '99999999-0000-0000-0000-000000000001', 'APPROVED', 'PENDING');
    """)
    assert sql("SELECT count(*) FROM kyc_execution_grace") == "1"
    migrate("downgrade", parent)
    assert sql("SELECT to_regclass('public.kyc_execution_grace') IS NULL") == "t"
