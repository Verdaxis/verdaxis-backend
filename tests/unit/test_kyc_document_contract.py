"""Storage and release contracts for encrypted evidence."""
from pathlib import Path

from alembic.config import Config
from alembic.script import ScriptDirectory


def test_checkpoint_and_migration_chain():
    script = ScriptDirectory.from_config(Config("alembic.ini"))
    assert script.get_current_head() == "p0_20261016_union"
    assert script.get_revision("p0_20261016_union").down_revision == "kycd_20261015_staging"
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


def test_downgrade_locks_fk_targets_nowait_before_read_or_drop(monkeypatch):
    import importlib.util
    from unittest.mock import Mock
    spec = importlib.util.spec_from_file_location("vault_migration", "alembic/versions/kycd_20261015_staging.py")
    migration = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(migration)
    connection = Mock()
    connection.execute.return_value.scalar.return_value = False
    monkeypatch.setattr(migration.op, "get_bind", lambda: connection)
    drop = Mock()
    monkeypatch.setattr(migration.op, "drop_table", drop)
    migration.downgrade()
    statements = [str(call.args[0]) for call in connection.execute.call_args_list]
    assert statements[0] == "LOCK TABLE users, organizations, kyc_documents IN ACCESS EXCLUSIVE MODE NOWAIT"
    assert statements[1] == "SELECT EXISTS (SELECT 1 FROM kyc_documents)"
    drop.assert_called_once_with("kyc_documents")
