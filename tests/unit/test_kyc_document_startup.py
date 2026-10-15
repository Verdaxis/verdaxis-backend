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


@pytest.mark.parametrize("raw", [b"[" * 100000 + b"0" + b"]" * 100000,
                                b"\xffprivate-key", b'{"active":[],"keys":[]}',
                                b"x" * (1024 * 1024 + 1)])
def test_parse_failures_are_sanitized(tmp_path, monkeypatch, caplog, raw):
    from app.services.kyc_document_vault import load_runtime_keys
    (tmp_path / "kyc-document-keys").write_bytes(raw)
    monkeypatch.setenv("CREDENTIALS_DIRECTORY", str(tmp_path))
    assert load_runtime_keys(SimpleNamespace(ENVIRONMENT="staging")) is None
    assert caplog.messages == ["KYC document storage unavailable: key file missing or invalid"]


async def test_nested_key_file_does_not_stop_lifespan_or_submit_503(tmp_path, monkeypatch, caplog):
    from unittest.mock import AsyncMock
    from app import main
    from app.services import market_event_dispatch
    from app import database
    from app.routers.kyc import _document_keys
    from fastapi import HTTPException, Request

    (tmp_path / "kyc-document-keys").write_text("[" * 100000 + "0" + "]" * 100000)
    monkeypatch.setenv("CREDENTIALS_DIRECTORY", str(tmp_path))
    monkeypatch.setattr(database, "verify_database_runtime", AsyncMock())
    monkeypatch.setattr(market_event_dispatch.MarketEventDispatcher, "start", AsyncMock())
    monkeypatch.setattr(market_event_dispatch.MarketEventDispatcher, "stop", AsyncMock())
    monkeypatch.setattr(main, "mcp", None)
    async with main.lifespan(main.app):
        assert main.app.state.kyc_document_keys is None
        with pytest.raises(HTTPException) as error:
            _document_keys(Request({"type": "http", "app": main.app}))
        assert error.value.status_code == 503
        assert error.value.detail == {"code": "KYC_DOCUMENT_STORAGE_UNAVAILABLE"}
    assert caplog.messages.count("KYC document storage unavailable: key file missing or invalid") == 1


def test_oversized_file_never_reaches_json_parser(tmp_path, monkeypatch):
    from app.services import kyc_document_vault as vault
    path = tmp_path / "keys"
    path.write_bytes(b"x" * (vault._MAX_KEY_FILE_BYTES + 1))
    def forbidden(*args, **kwargs):
        pytest.fail("Oversized key file reached JSON parser")
    monkeypatch.setattr(vault.json, "loads", forbidden)
    with pytest.raises(vault.KeyFileError):
        vault.KeyRing.load(path)
