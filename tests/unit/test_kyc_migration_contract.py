"""Offline checks for the integrated KYC migration and member lookup index."""
from pathlib import Path
from unittest.mock import MagicMock

from alembic.config import Config
from alembic.script import ScriptDirectory
from alembic.util import load_python_file

from app.models.user import User

ROOT = Path(__file__).resolve().parents[2]
INDEX_NAME = "ix_users_organization_id_id"


def test_kyc_is_the_only_head_after_bio_lng():
    config = Config(str(ROOT / "alembic.ini"))
    config.set_main_option("script_location", str(ROOT / "alembic"))
    scripts = ScriptDirectory.from_config(config)
    assert scripts.get_heads() == ["kycg_20261014_prod"]
    assert scripts.get_revision("kycg_20261014_prod").down_revision == "blng_20261014_prod"
    checkpoints = (ROOT / "deploy/migration-checkpoints.tsv").read_text().splitlines()
    assert "blng_20261014_prod\tkycg_20261014_prod" in checkpoints
    assert "kycg_20261014_prod\tkycg_20261014_prod" in checkpoints
    assert "uadl_20261003_delivery_reports\tkycg_20261014_prod" not in checkpoints


def test_member_lookup_index_has_migration_and_orm_parity(monkeypatch):
    migration = load_python_file(str(ROOT / "alembic/versions"), "kycg_20261014_prod.py")
    operations = MagicMock()
    monkeypatch.setattr(migration, "op", operations)
    migration.upgrade()
    assert operations.mock_calls[0].args == (
        "LOCK TABLE users IN ACCESS EXCLUSIVE MODE NOWAIT",
    )
    operations.create_index.assert_called_once_with(
        INDEX_NAME, "users", ["organization_id", "id"], unique=False,
    )
    migration.downgrade()
    operations.drop_index.assert_called_once_with(INDEX_NAME, table_name="users")
    index = next(index for index in User.__table__.indexes if index.name == INDEX_NAME)
    assert [column.name for column in index.columns] == ["organization_id", "id"]
    assert not index.unique
