import pytest
from fastapi import HTTPException, Request

from app import config, routing


def _origins(environment: str) -> tuple[str, ...]:
    function = getattr(config, "credentialed_origins_for_environment", None)
    assert callable(function), "environment-bound credentialed origins are not implemented"
    return function(environment)


def _guard(*args, **kwargs) -> None:
    function = getattr(routing, "require_trusted_browser_origin", None)
    assert callable(function), "sensitive cookie Origin guard is not implemented"
    function(*args, **kwargs)


def _request(*, origin: str | None, cookie: str | None = None) -> Request:
    headers: list[tuple[bytes, bytes]] = []
    if origin is not None:
        headers.append((b"origin", origin.encode()))
    if cookie is not None:
        headers.append((b"cookie", f"refresh_token={cookie}".encode()))
    return Request(
        {
            "type": "http",
            "method": "POST",
            "path": "/api/auth/refresh",
            "query_string": b"",
            "headers": headers,
            "client": ("127.0.0.1", 1234),
            "scheme": "https",
            "server": ("api.verdaxis.exchange", 443),
        }
    )


def test_production_and_staging_credentialed_origins_are_exact_and_disjoint():
    production = _origins("production")
    staging = _origins("staging")

    assert production == ("https://app.verdaxis.exchange", "https://verdaxis.exchange")
    assert staging == ("https://staging.verdaxis.exchange",)
    assert set(production).isdisjoint(staging)


def test_unknown_deployed_environment_has_no_credentialed_origins():
    assert _origins("preview") == ()


def test_same_origin_cookie_request_is_allowed():
    _guard(
        _request(origin="https://app.verdaxis.exchange", cookie="prod-token"),
        environment="production",
        cookie_authenticated=True,
    )


def test_staging_origin_cannot_use_or_read_production_cookie_token():
    with pytest.raises(HTTPException) as exc_info:
        _guard(
            _request(origin="https://staging.verdaxis.exchange", cookie="prod-token"),
            environment="production",
            cookie_authenticated=True,
        )

    assert exc_info.value.status_code == 403
    assert exc_info.value.detail["code"] == "BROWSER_ORIGIN_REJECTED"


def test_cookie_authenticated_request_without_origin_is_rejected():
    with pytest.raises(HTTPException) as exc_info:
        _guard(
            _request(origin=None, cookie="prod-token"),
            environment="production",
            cookie_authenticated=True,
        )

    assert exc_info.value.status_code == 403


def test_non_browser_token_client_without_cookie_or_origin_remains_supported():
    _guard(
        _request(origin=None),
        environment="production",
        cookie_authenticated=False,
    )


def test_production_unit_is_loopback_only_and_does_not_trust_every_proxy():
    unit = (
        __import__("pathlib").Path(__file__).resolve().parents[2]
        / "deploy/systemd/verdaxis-backend.service"
    ).read_text()
    assert "--host 127.0.0.1" in unit
    assert "--forwarded-allow-ips 127.0.0.1" in unit
    assert "--forwarded-allow-ips *" not in unit
