"""Strict release-health and operator installation gates."""

import importlib.util
import json
from pathlib import Path
import subprocess

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


def _load_systemd_source_verifier():
    path = ROOT / "scripts" / "verify_systemd_source.py"
    assert path.exists(), "release-bound systemd source verifier is required"
    spec = importlib.util.spec_from_file_location("verify_systemd_source", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _committed_unit_source(tmp_path: Path) -> tuple[Path, str, tuple[str, ...]]:
    root = tmp_path / "approved-release"
    unit_dir = root / "deploy" / "systemd"
    unit_dir.mkdir(parents=True)
    units = (
        "deploy/systemd/verdaxis-backend.service",
        "deploy/systemd/verdaxis-news-refresh.service",
        "deploy/systemd/verdaxis-news-refresh.timer",
    )
    for relative in units:
        (root / relative).write_text(f"[Unit]\nDescription={Path(relative).name}\n")
    preflight = root / "scripts" / "preflight_runtime.py"
    preflight.parent.mkdir()
    preflight.write_text("# approved preflight\n")
    subprocess.run(["git", "init", "-q"], cwd=root, check=True)
    subprocess.run(
        ["git", "config", "user.email", "runtime-test@example.invalid"],
        cwd=root,
        check=True,
    )
    subprocess.run(
        ["git", "config", "user.name", "Runtime Test"], cwd=root, check=True
    )
    subprocess.run(["git", "add", "."], cwd=root, check=True)
    subprocess.run(["git", "commit", "-qm", "approved units"], cwd=root, check=True)
    source_ref = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=root, text=True
    ).strip()
    return root, source_ref, units


@pytest.mark.parametrize("mode", ["dry-run", "apply"])
def test_systemd_source_provenance_refuses_wrong_sha_in_every_mode(tmp_path, mode):
    verifier = _load_systemd_source_verifier()
    root, _source_ref, units = _committed_unit_source(tmp_path)

    with pytest.raises(verifier.SourceProvenanceError, match="source ref"):
        verifier.verify_source_provenance(
            source_root=root,
            source_ref="b" * 40,
            unit_paths=units,
            mode=mode,
        )


@pytest.mark.parametrize("mode", ["dry-run", "apply"])
def test_systemd_source_provenance_refuses_dirty_unit_bytes(tmp_path, mode):
    verifier = _load_systemd_source_verifier()
    root, source_ref, units = _committed_unit_source(tmp_path)
    (root / units[0]).write_text("[Unit]\nDescription=unapproved bytes\n")

    with pytest.raises(verifier.SourceProvenanceError, match="dirty"):
        verifier.verify_source_provenance(
            source_root=root,
            source_ref=source_ref,
            unit_paths=units,
            mode=mode,
        )


@pytest.mark.parametrize("mode", ["dry-run", "apply"])
def test_systemd_source_provenance_refuses_digest_mismatch_hidden_from_status(
    tmp_path, monkeypatch, mode
):
    verifier = _load_systemd_source_verifier()
    root, source_ref, units = _committed_unit_source(tmp_path)
    real_git = verifier._git

    def status_blind_git(source_root, *args, **kwargs):
        if args[:2] == ("status", "--porcelain"):
            return ""
        return real_git(source_root, *args, **kwargs)

    monkeypatch.setattr(verifier, "_git", status_blind_git)
    (root / units[0]).write_text("[Unit]\nDescription=hidden unapproved bytes\n")

    with pytest.raises(verifier.SourceProvenanceError, match="digest"):
        verifier.verify_source_provenance(
            source_root=root,
            source_ref=source_ref,
            unit_paths=units,
            mode=mode,
        )


@pytest.mark.parametrize("mode", ["dry-run", "apply"])
def test_systemd_source_provenance_refuses_hidden_non_unit_source_changes(
    tmp_path, mode
):
    verifier = _load_systemd_source_verifier()
    root, source_ref, units = _committed_unit_source(tmp_path)
    preflight = "scripts/preflight_runtime.py"
    subprocess.run(
        ["git", "update-index", "--assume-unchanged", preflight],
        cwd=root,
        check=True,
    )
    (root / preflight).write_text("# bypassed preflight\n")

    with pytest.raises(verifier.SourceProvenanceError, match="index flags"):
        verifier.verify_source_provenance(
            source_root=root,
            source_ref=source_ref,
            unit_paths=units,
            mode=mode,
        )


@pytest.mark.parametrize("mode", ["dry-run", "apply"])
def test_systemd_source_provenance_refuses_git_replacement_refs(tmp_path, mode):
    verifier = _load_systemd_source_verifier()
    root, source_ref, units = _committed_unit_source(tmp_path)
    subprocess.run(
        ["git", "commit", "--allow-empty", "-qm", "replacement commit"],
        cwd=root,
        check=True,
    )
    replacement_ref = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=root, text=True
    ).strip()
    subprocess.run(
        ["git", "checkout", "-q", "--detach", source_ref], cwd=root, check=True
    )
    subprocess.run(
        ["git", "replace", source_ref, replacement_ref], cwd=root, check=True
    )

    with pytest.raises(verifier.SourceProvenanceError, match="replacement refs"):
        verifier.verify_source_provenance(
            source_root=root,
            source_ref=source_ref,
            unit_paths=units,
            mode=mode,
        )


@pytest.mark.parametrize("mode", ["dry-run", "apply"])
def test_systemd_source_provenance_accepts_exact_clean_release(tmp_path, mode):
    verifier = _load_systemd_source_verifier()
    root, source_ref, units = _committed_unit_source(tmp_path)

    digests = verifier.verify_source_provenance(
        source_root=root,
        source_ref=source_ref,
        unit_paths=units,
        mode=mode,
    )

    assert set(digests) == set(units)
    assert all(len(digest) == 64 for digest in digests.values())


def test_systemd_installer_requires_independent_environment_and_source_ref():
    source = (ROOT / "scripts" / "install_systemd_units.sh").read_text()

    assert "--environment" in source
    assert "--source-ref" in source
    assert "verify_systemd_source.py" in source
    assert "/home/verdaxis-prod/verdaxis/prod/be" in source
    assert "/home/verdaxis-prod/verdaxis/staging/be" in source
    for unit_name in (
        "verdaxis-backend.service",
        "verdaxis-news-refresh.service",
        "verdaxis-news-refresh.timer",
        "verdaxis-backend-staging.service",
        "verdaxis-news-refresh-staging.service",
        "verdaxis-news-refresh-staging.timer",
    ):
        assert unit_name in source
    assert "preflight_backend production" not in source
    assert "preflight_backend staging" not in source
    assert source.index("verify_systemd_source.py") < source.index(
        'if [[ "$MODE" == "dry-run" ]]'
    )
