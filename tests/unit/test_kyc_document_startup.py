"""Key configuration failure must not stop the application."""
from types import SimpleNamespace

import pytest

from tests.unit.test_kyc_document_vault import key_file


@pytest.mark.parametrize("environment", ["staging", "production", "test"])
def test_missing_key_logs_once_and_returns_unavailable(monkeypatch, caplog, environment):
    from app.services.kyc_document_vault import load_runtime_keys
    monkeypatch.delenv("CREDENTIALS_DIRECTORY", raising=False)
    config = SimpleNamespace(ENVIRONMENT=environment, KYC_DOCUMENT_KEYS_PATH=None)
    assert load_runtime_keys(config) is None
    assert caplog.text.count("KYC document storage unavailable") == 1


def test_systemd_credential_precedes_explicit_path(tmp_path, monkeypatch):
    from app.services.kyc_document_vault import load_runtime_keys
    key_file(tmp_path).rename(tmp_path / "kyc-document-keys")
    monkeypatch.setenv("CREDENTIALS_DIRECTORY", str(tmp_path))
    config = SimpleNamespace(ENVIRONMENT="staging", KYC_DOCUMENT_KEYS_PATH="ignored")
    assert load_runtime_keys(config).active == "old"


def test_explicit_path_is_development_only(tmp_path, monkeypatch):
    from app.services.kyc_document_vault import load_runtime_keys
    monkeypatch.delenv("CREDENTIALS_DIRECTORY", raising=False)
    config = SimpleNamespace(ENVIRONMENT="test", KYC_DOCUMENT_KEYS_PATH=str(key_file(tmp_path)))
    assert load_runtime_keys(config).active == "old"
    config.ENVIRONMENT = "production"
    assert load_runtime_keys(config) is None
