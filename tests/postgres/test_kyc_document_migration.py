"""Disposable PostgreSQL storage, retention and downgrade contracts."""

import pytest

from tests.postgres.test_supplier_offer_migration_roundtrip import (
    supplier_scratch_database as supplier_scratch_database,
)


def test_document_migration_roundtrip_and_restrict(supplier_scratch_database):
    sql, migrate = supplier_scratch_database
    parent = "kycg_20261014_staging"
    migrate("upgrade", "kycd_20261015_staging")
    assert sql("SELECT to_regclass('public.kyc_documents') IS NOT NULL") == "t"
    migrate("downgrade", parent)
    assert sql("SELECT to_regclass('public.kyc_documents') IS NULL") == "t"
    migrate("upgrade", "kycd_20261015_staging")
    sql("""
        INSERT INTO organizations (id,name,type) VALUES
        ('99999999-0000-0000-0000-000000000001','Vault test','FUEL_BUYER');
        INSERT INTO users (id,email,password_hash,role,organization_id) VALUES
        ('99999999-0000-0000-0000-000000000002','vault@example.test','unused','BUYER',
        '99999999-0000-0000-0000-000000000001');
        INSERT INTO kyc_documents (id,submission_id,user_id,organization_id,kind,content_type,
        size_bytes,key_id,wrapped_key,key_nonce,data_nonce,ciphertext) VALUES
        ('99999999-0000-0000-0000-000000000003','99999999-0000-0000-0000-000000000004',
        '99999999-0000-0000-0000-000000000002','99999999-0000-0000-0000-000000000001',
        'IDENTITY','application/pdf',1,'synthetic','x','x','x','x');
    """)
    for table in ("users", "organizations"):
        with pytest.raises(AssertionError):
            sql(f"DELETE FROM {table}")
    assert "Cannot downgrade" in migrate("downgrade", parent, success=False)
    assert sql("SELECT count(*) FROM kyc_documents") == "1"
