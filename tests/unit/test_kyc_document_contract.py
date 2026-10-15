"""Storage and release contracts for encrypted evidence."""
from pathlib import Path

from alembic.config import Config
from alembic.script import ScriptDirectory


def test_checkpoint_and_migration_chain():
    script = ScriptDirectory.from_config(Config("alembic.ini"))
    assert script.get_current_head() == "kycd_20261015_staging"
    assert script.get_revision("kycd_20261015_staging").down_revision == "kycg_20261014_staging"
    rows = Path("deploy/migration-checkpoints.tsv").read_text().splitlines()
    assert "kycg_20261014_staging\tkycd_20261015_staging" in rows
    assert "kycd_20261015_staging\tkycd_20261015_staging" in rows


def test_model_export_and_append_only_acl():
    from app import models
    assert hasattr(models, "KYCDocument")
    table = models.KYCDocument.__table__
    assert {fk.ondelete for fk in table.foreign_keys} == {"RESTRICT"}
    assert {c.name for c in table.columns} == {
        "id", "submission_id", "user_id", "organization_id", "kind", "content_type",
        "size_bytes", "key_id", "wrapped_key", "key_nonce", "data_nonce", "ciphertext", "created_at",
    }
    assert "('kyc_documents', ARRAY['SELECT', 'INSERT'])" in Path("deploy/postgres/app_acl_policy.sql").read_text()


def test_credentials_are_optional_at_systemd_boundary():
    for suffix, key_suffix in (("", ""), ("-staging", "-staging")):
        unit = Path(f"deploy/systemd/verdaxis-backend{suffix}.service").read_text()
        assert f"LoadCredential=kyc-document-keys:/etc/verdaxis/kyc-document-keys{key_suffix}.json" in unit
        assert "SetCredential=kyc-document-keys:\n" in unit
