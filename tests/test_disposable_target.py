from __future__ import annotations

import asyncio
import json
from pathlib import Path

import pytest


TOKEN = "disposable-test-token-0123456789abcdef"


def load_target_module():
    from tests import disposable_target

    return disposable_target


def test_target_has_no_implicit_url_or_opt_in(monkeypatch):
    target = load_target_module()
    for name in (
        "TEST_API_URL",
        "VERDAXIS_DISPOSABLE_TARGET_URL",
        "VERDAXIS_DISPOSABLE_TARGET_TOKEN",
        "VERDAXIS_RUN_DISPOSABLE_INTEGRATION",
    ):
        monkeypatch.delenv(name, raising=False)

    with pytest.raises(target.DisposableTargetError, match="explicit disposable target"):
        target.from_environment()


@pytest.mark.parametrize(
    "url",
    [
        "https://api.verdaxis.exchange",
        "https://api-staging.verdaxis.exchange",
        "http://144.126.151.136:8000",
        "http://localhost:59123",
        "http://127.0.0.1:8000",
        "http://127.0.0.1:8001",
        "http://127.0.0.1:80",
        "http://127.0.0.1:443",
        "http://127.0.0.2:59123",
        "http://[::1]:59123",
        "http://127.0.0.1:59123/api",
        "http://user:password@127.0.0.1:59123",
    ],
)
def test_target_hard_refuses_live_ambiguous_or_non_loopback_urls(url):
    target = load_target_module()

    with pytest.raises(target.DisposableTargetError):
        target.validate_target(url, TOKEN)


def test_target_accepts_only_numeric_loopback_ephemeral_port():
    target = load_target_module()

    configured = target.validate_target("http://127.0.0.1:59123", TOKEN)

    assert configured.base_url == "http://127.0.0.1:59123"
    assert configured.token == TOKEN
    assert configured.identity_url == (
        "http://127.0.0.1:59123/.well-known/verdaxis-disposable-test"
    )


@pytest.mark.parametrize("token", ["", "short", "a" * 257, "contains whitespace" * 2])
def test_target_requires_a_bounded_high_entropy_token(token):
    target = load_target_module()

    with pytest.raises(target.DisposableTargetError):
        target.validate_target("http://127.0.0.1:59123", token)


def test_disposable_identity_endpoint_requires_exact_typed_payload():
    target = load_target_module()
    configured = target.validate_target("http://127.0.0.1:59123", TOKEN)
    valid = {
        "disposable": True,
        "purpose": "verdaxis-integration-test",
        "token": TOKEN,
    }

    assert target.validate_identity_payload(json.dumps(valid).encode(), configured)

    invalid = [
        None,
        [],
        {**valid, "disposable": 1},
        {**valid, "token": "wrong"},
        {**valid, "purpose": ["verdaxis-integration-test"]},
        {**valid, "extra": "not allowed"},
    ]
    for payload in invalid:
        raw = b"not-json" if payload is None else json.dumps(payload).encode()
        with pytest.raises(target.DisposableTargetError):
            target.validate_identity_payload(raw, configured)


def test_disposable_identity_rejects_duplicate_json_keys():
    target = load_target_module()
    configured = target.validate_target("http://127.0.0.1:59123", TOKEN)
    raw = (
        '{"disposable":true,"disposable":true,'
        '"purpose":"verdaxis-integration-test",'
        f'"token":"{TOKEN}"}}'
    ).encode()

    with pytest.raises(target.DisposableTargetError):
        target.validate_identity_payload(raw, configured)


def test_source_tree_server_config_is_test_only_numeric_loopback_ephemeral():
    from tests.disposable_server import ServerConfigError, validate_config

    real_sha = "0123456789abcdef0123456789abcdef01234567"
    assert validate_config("test", TOKEN, "127.0.0.1", 59123, real_sha) == TOKEN
    for values in (
        ("production", TOKEN, "127.0.0.1", 59123, real_sha),
        ("test", TOKEN, "0.0.0.0", 59123, real_sha),
        ("test", TOKEN, "127.0.0.1", 8000, real_sha),
        ("test", "short", "127.0.0.1", 59123, real_sha),
        # Readiness asserts a full 40-hex release identity; the unit-test
        # default sentinel and short prefixes are refused up front.
        ("test", TOKEN, "127.0.0.1", 59123, "test"),
        ("test", TOKEN, "127.0.0.1", 59123, ""),
        ("test", TOKEN, "127.0.0.1", 59123, real_sha[:12]),
    ):
        with pytest.raises(ServerConfigError):
            validate_config(*values)


def test_source_tree_server_produces_identity_and_delegates_other_paths():
    from tests.disposable_server import DisposableIdentityApp

    delegated = []

    async def downstream(scope, receive, send):
        delegated.append(scope["path"])

    app = DisposableIdentityApp(downstream, TOKEN)
    sent = []

    async def receive():
        return {"type": "http.request", "body": b"", "more_body": False}

    async def send(message):
        sent.append(message)

    asyncio.run(
        app(
            {
                "type": "http",
                "method": "GET",
                "path": "/.well-known/verdaxis-disposable-test",
            },
            receive,
            send,
        )
    )
    assert sent[0]["status"] == 200
    assert json.loads(sent[1]["body"]) == {
        "disposable": True,
        "purpose": "verdaxis-integration-test",
        "token": TOKEN,
    }

    asyncio.run(
        app(
            {"type": "http", "method": "GET", "path": "/health"},
            receive,
            send,
        )
    )
    assert delegated == ["/health"]



def test_disposable_identity_rejects_json_decoder_recursion(monkeypatch):
    target = load_target_module()
    configured = target.validate_target("http://127.0.0.1:59123", TOKEN)

    def fail_decode(_raw, **_kwargs):
        raise RecursionError

    monkeypatch.setattr(target.json, "loads", fail_decode)

    with pytest.raises(target.DisposableTargetError):
        target.validate_identity_payload(b"{}", configured)


def test_attestation_uses_only_the_fixed_identity_path_and_bounded_fetch():
    target = load_target_module()
    configured = target.validate_target("http://127.0.0.1:59123", TOKEN)
    calls = []

    def fetch(url, *, timeout_seconds, max_bytes):
        calls.append((url, timeout_seconds, max_bytes))
        return json.dumps(
            {
                "disposable": True,
                "purpose": "verdaxis-integration-test",
                "token": TOKEN,
            }
        ).encode()

    target.attest(configured, fetch=fetch)

    assert calls == [(configured.identity_url, 2.0, 1024)]


@pytest.mark.parametrize(
    ("path", "expected"),
    [
        (Path("tests/integration/test_api.py"), True),
        (Path("tests/e2e_local_api.py"), True),
        (Path("tests/e2e/test_flow.py"), True),
        (Path("tests/unit/test_api.py"), False),
        (Path("tests/monitor/test_local.py"), False),
    ],
)
def test_integration_and_e2e_paths_are_classified_for_guarding(path, expected):
    target = load_target_module()

    assert target.requires_disposable_target(path) is expected


def test_conftest_contains_no_default_api_target():
    source = (Path(__file__).parent / "conftest.py").read_text()

    assert 'os.environ.get("TEST_API_URL",' not in source
    assert "http://localhost:8000" not in source
    assert "--run-disposable-integration" in source
    assert "--disposable-target-url" in source
    assert "--disposable-target-token" in source


# Integration note (2026-07-20): the monitor branch also guarded
# scripts/test_purchase_flow.py, but the runtime branch removed that harness
# as an unreferenced obsolete diagnostic (commit 4f70073) before integration,
# so the attestation guard now covers the sole remaining direct diagnostic.
@pytest.mark.parametrize(
    "relative_path",
    ["tests/verify_remote_auth.py"],
)
def test_direct_mutating_diagnostics_share_the_disposable_attestation_guard(
    relative_path
):
    source = (Path(__file__).parents[1] / relative_path).read_text()

    assert "from_environment" in source
    assert "attest" in source
    assert "http://localhost:8000" not in source
    assert "144.126.151.136" not in source


class FakeConfig:
    def __init__(self, *, run=False, url=None, token=None):
        self.options = {
            "--run-disposable-integration": run,
            "--disposable-target-url": url,
            "--disposable-target-token": token,
        }

    def getoption(self, name):
        return self.options[name]

    def addinivalue_line(self, _name, _value):
        return None


class FakeItem:
    def __init__(self, path):
        self.path = Path(path)
        self.markers = []

    def add_marker(self, marker):
        self.markers.append(marker)


def test_collection_skips_guarded_tests_without_explicit_opt_in():
    from tests import conftest

    integration = FakeItem("tests/integration/test_api.py")
    unit = FakeItem("tests/unit/test_api.py")

    conftest.pytest_collection_modifyitems(FakeConfig(), [integration, unit])

    assert [marker.name for marker in integration.markers] == [
        "disposable_integration",
        "skip",
    ]
    assert unit.markers == []


def test_collection_attests_before_allowing_guarded_tests(monkeypatch):
    from tests import conftest

    configured = validate = load_target_module().validate_target(
        "http://127.0.0.1:59123", TOKEN
    )
    config = FakeConfig(run=True, url=configured.base_url, token=TOKEN)
    config._verdaxis_disposable_target = configured
    item = FakeItem("tests/e2e/test_flow.py")
    calls = []
    monkeypatch.setattr(conftest, "attest", calls.append)

    conftest.pytest_collection_modifyitems(config, [item])

    assert calls == [validate]
    assert [marker.name for marker in item.markers] == ["disposable_integration"]


def test_pytest_opt_in_hard_fails_for_live_target():
    from tests import conftest

    config = FakeConfig(
        run=True,
        url="https://api.verdaxis.exchange",
        token=TOKEN,
    )

    with pytest.raises(pytest.UsageError):
        conftest.pytest_configure(config)
