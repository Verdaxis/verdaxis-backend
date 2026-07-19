"""Strict release-health and operator installation gates."""

import json
from pathlib import Path

import pytest

from scripts.validate_health_response import HealthResponseError, validate_health_response


ROOT = Path(__file__).parents[2]
SHA = "a" * 40


def test_health_response_requires_exact_status_environment_and_release_sha():
    validate_health_response(
        json.dumps(
            {
                "status": "ok",
                "db": "connected",
                "environment": "production",
                "release_sha": SHA,
            }
        ),
        expected_environment="production",
        expected_release_sha=SHA,
    )

    for mutation in (
        {"status": "okay"},
        {"environment": "staging"},
        {"release_sha": "b" * 40},
    ):
        payload = {
            "status": "ok",
            "db": "connected",
            "environment": "production",
            "release_sha": SHA,
            **mutation,
        }
        with pytest.raises(HealthResponseError):
            validate_health_response(
                json.dumps(payload),
                expected_environment="production",
                expected_release_sha=SHA,
            )


@pytest.mark.parametrize("payload", ["", "not-json", "[]", "null"])
def test_health_response_rejects_malformed_or_non_object_json(payload):
    with pytest.raises(HealthResponseError):
        validate_health_response(
            payload,
            expected_environment="production",
            expected_release_sha=SHA,
        )


def test_deploy_rejects_dirty_release_and_uses_json_health_gate():
    source = (ROOT / "scripts/deploy.sh").read_text()

    assert "ALLOW_DIRTY" not in source
    assert "status --porcelain" in source
    assert "validate_health_response.py" in source
    assert "grep -q" not in source
    assert source.rindex("status --porcelain") < source.index(
        'write_release_artifact "$CURRENT_SHA"'
    )
    assert "preflight_runtime.py" in source


def test_systemd_installer_is_idempotent_preflights_and_never_starts_services():
    source = (ROOT / "scripts/install_systemd_units.sh").read_text()

    assert "systemd-analyze verify" in source
    assert "preflight_runtime.py" in source
    assert "alembic current --check-heads" in source
    assert "cmp --silent" in source or "cmp -s" in source
    assert "systemctl daemon-reload" in source
    assert "systemctl start" not in source
    assert "systemctl restart" not in source
    assert "systemctl enable" not in source
