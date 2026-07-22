from __future__ import annotations

import gzip
import importlib.util
import json
import os
import stat
from types import SimpleNamespace
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "deploy/monitor/backup_verify.py"
NOW = datetime(2026, 7, 20, 4, 0, tzinfo=timezone.utc)
BACKUP_ID = "20260720-030000"
DUMP_BYTES = b"--\n-- PostgreSQL database dump\n--\n" + (b"SELECT 1;\n" * 128)


def load_module():
    assert MODULE_PATH.exists(), "backup verifier has not been implemented"
    spec = importlib.util.spec_from_file_location("backup_verify", MODULE_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write_attempt(
    directory: Path,
    *,
    attempt_id: str,
    state: str,
    started_at: datetime,
    finished_at: datetime | None = None,
    backup_id: str | None = None,
) -> Path:
    attempts = directory / "attempts"
    attempts.mkdir(exist_ok=True)
    path = attempts / f"{attempt_id}.json"
    path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "attempt_id": attempt_id,
                "state": state,
                "started_at": started_at.isoformat(),
                "finished_at": finished_at.isoformat() if finished_at else None,
                "backup_id": backup_id,
            }
        )
    )
    return path


def write_fixture(
    directory: Path,
    *,
    payload: dict | None = None,
    backup_id: str = BACKUP_ID,
    write_success_attempt: bool = True,
):
    payload = payload or {
        "ok": True,
        "completed_at": (NOW - timedelta(hours=1)).isoformat(),
        "backup_id": backup_id,
        "databases": ["verdaxis", "verdaxis_staging", "umami"],
    }
    (directory / "status.json").write_text(json.dumps(payload))
    prefixes = ("verdaxis", "verdaxis-staging", "umami")
    completed_at = datetime.fromisoformat(payload["completed_at"].replace("Z", "+00:00"))
    for prefix in prefixes:
        artifact = directory / f"{prefix}-{backup_id}.sql.gz"
        with gzip.open(artifact, "wb") as handle:
            handle.write(DUMP_BYTES)
        os.utime(artifact, (completed_at.timestamp(), completed_at.timestamp()))
    if write_success_attempt:
        write_attempt(
            directory,
            attempt_id=f"attempt-{backup_id}",
            state="succeeded",
            started_at=completed_at - timedelta(minutes=5),
            finished_at=completed_at,
            backup_id=backup_id,
        )
    return directory / "status.json"


def verify(directory: Path, **overrides):
    module = load_module()
    options = {
        "now": NOW,
        "max_age_seconds": 30 * 60 * 60,
        "future_tolerance_seconds": 300,
        "max_timestamp_skew_seconds": 5,
        "max_metadata_bytes": 16_384,
        "max_attempt_records": 100,
        "max_attempt_bytes": 4_096,
        "max_artifacts": 100,
        "max_artifact_bytes": 10_000_000,
        "max_uncompressed_bytes": 10_000_000,
        "total_timeout_seconds": 5.0,
    }
    options.update(overrides)
    return module.verify_backups(
        directory / "status.json", directory, directory / "attempts", **options
    )


def codes(results):
    return {result["code"] for result in results}


def test_expected_inventory_and_complete_gzip_set_pass(tmp_path):
    write_fixture(tmp_path)

    results = verify(tmp_path)

    assert all(result["category"] == "healthy" for result in results)
    assert {result["name"] for result in results} == {
        "backup_attempt",
        "backup_metadata",
        "verdaxis_artifact",
        "verdaxis_staging_artifact",
        "umami_artifact",
    }


def test_missing_attempt_evidence_fails_closed(tmp_path):
    write_fixture(tmp_path)
    for marker in (tmp_path / "attempts").iterdir():
        marker.unlink()
    (tmp_path / "attempts").rmdir()

    assert codes(verify(tmp_path)) == {"attempt_evidence_missing"}


@pytest.mark.parametrize(
    ("state", "finished_at", "expected_code"),
    [
        ("started", None, "backup_attempt_incomplete"),
        ("failed", NOW - timedelta(minutes=20), "backup_attempt_failed"),
    ],
)
def test_newer_crashed_or_failed_attempt_keeps_older_success_red(
    tmp_path, state, finished_at, expected_code
):
    write_fixture(tmp_path)
    write_attempt(
        tmp_path,
        attempt_id=f"attempt-newer-{state}",
        state=state,
        started_at=NOW - timedelta(minutes=30),
        finished_at=finished_at,
    )

    assert codes(verify(tmp_path)) == {expected_code}


def test_only_a_newer_verified_success_recovers_from_a_failed_attempt(tmp_path):
    write_fixture(tmp_path)
    write_attempt(
        tmp_path,
        attempt_id="attempt-failed",
        state="failed",
        started_at=NOW - timedelta(minutes=30),
        finished_at=NOW - timedelta(minutes=20),
    )
    assert codes(verify(tmp_path)) == {"backup_attempt_failed"}

    newer_completed = NOW - timedelta(minutes=5)
    newer_id = newer_completed.strftime("%Y%m%d-%H%M%S")
    write_fixture(
        tmp_path,
        payload={
            "ok": True,
            "completed_at": newer_completed.isoformat(),
            "backup_id": newer_id,
            "databases": ["verdaxis", "verdaxis_staging", "umami"],
        },
        backup_id=newer_id,
    )

    assert all(result["category"] == "healthy" for result in verify(tmp_path))


def test_latest_success_must_match_the_published_generation(tmp_path):
    write_fixture(tmp_path)
    write_attempt(
        tmp_path,
        attempt_id="attempt-new-success",
        state="succeeded",
        started_at=NOW - timedelta(minutes=20),
        finished_at=NOW - timedelta(minutes=10),
        backup_id="20260720-035000",
    )

    assert codes(verify(tmp_path)) == {"attempt_status_mismatch"}


@pytest.mark.parametrize(
    "malformed",
    [
        [],
        {"schema_version": 1, "state": "failed"},
        {
            "schema_version": True,
            "attempt_id": "bad",
            "state": "failed",
            "started_at": "2026-07-20T03:30:00Z",
            "finished_at": "2026-07-20T03:31:00Z",
            "backup_id": None,
        },
    ],
)
def test_malformed_attempt_journal_fails_closed(tmp_path, malformed):
    write_fixture(tmp_path)
    (tmp_path / "attempts/bad.json").write_text(json.dumps(malformed))

    assert codes(verify(tmp_path)) == {"attempt_evidence_invalid"}


def test_attempt_journal_rejects_duplicate_json_keys(tmp_path):
    write_fixture(tmp_path)
    (tmp_path / "attempts/bad.json").write_text(
        '{"schema_version":1,"attempt_id":"bad","state":"started",'
        '"state":"started","started_at":"2026-07-20T03:30:00Z",'
        '"finished_at":null,"backup_id":null}'
    )

    assert codes(verify(tmp_path)) == {"attempt_evidence_invalid"}


def test_symlinked_attempt_record_fails_closed(tmp_path):
    write_fixture(tmp_path)
    external = tmp_path / "external-attempt.json"
    external.write_text("{}")
    (tmp_path / "attempts/symlink.json").symlink_to(external)

    assert codes(verify(tmp_path)) == {"attempt_evidence_invalid"}


@pytest.mark.parametrize(
    ("change", "expected_code"),
    [
        (lambda payload: payload.update(databases=["verdaxis"]), "unexpected_inventory"),
        (lambda payload: payload.update(completed_at="not-a-time"), "invalid_timestamp"),
        (lambda payload: payload.update(completed_at=(NOW + timedelta(minutes=6)).isoformat()), "future_timestamp"),
        (lambda payload: payload.update(completed_at=(NOW - timedelta(hours=31)).isoformat()), "stale_status"),
        (lambda payload: payload.update(backup_id="../../secret"), "invalid_backup_id"),
        (lambda payload: payload.update(ok=False), "backup_incomplete"),
    ],
)
def test_corrupt_future_stale_and_inventory_metadata_fail_safely(tmp_path, change, expected_code):
    status = write_fixture(tmp_path)
    payload = json.loads(status.read_text())
    change(payload)
    status.write_text(json.dumps(payload))

    results = verify(tmp_path)

    assert expected_code in codes(results)


@pytest.mark.parametrize(
    "inventory",
    [
        None,
        "verdaxis,verdaxis_staging,umami",
        [{"name": "verdaxis"}, "verdaxis_staging", "umami"],
        [["verdaxis"], "verdaxis_staging", "umami"],
    ],
)
def test_malformed_database_inventory_types_fail_without_crashing(tmp_path, inventory):
    status = write_fixture(tmp_path)
    payload = json.loads(status.read_text())
    payload["databases"] = inventory
    status.write_text(json.dumps(payload))

    results = verify(tmp_path)

    assert codes(results) == {"unexpected_inventory"}


def test_metadata_size_is_bounded_and_secret_is_not_reported(tmp_path):
    status = write_fixture(tmp_path)
    status.write_text(json.dumps({"secret": "do-not-copy" * 100}))

    results = verify(tmp_path, max_metadata_bytes=32)

    assert codes(results) == {"metadata_too_large"}
    assert "do-not-copy" not in json.dumps(results)


def test_backup_metadata_rejects_duplicate_json_keys(tmp_path):
    write_fixture(tmp_path)
    (tmp_path / "status.json").write_text(
        '{"ok":true,"ok":true,"completed_at":"2026-07-20T03:00:00Z",'
        '"backup_id":"20260720-030000",'
        '"databases":["verdaxis","verdaxis_staging","umami"]}'
    )

    assert codes(verify(tmp_path)) == {"metadata_invalid_json"}


def test_json_decoder_recursion_in_metadata_fails_without_crashing(
    tmp_path, monkeypatch
):
    module = load_module()
    write_fixture(tmp_path)
    (tmp_path / "status.json").write_text("[]")
    original_loads = module.json.loads

    def fail_decode(raw, **kwargs):
        if raw == b"[]":
            raise RecursionError
        return original_loads(raw, **kwargs)

    monkeypatch.setattr(module.json, "loads", fail_decode)

    results = module.verify_backups(
        tmp_path / "status.json",
        tmp_path,
        tmp_path / "attempts",
        now=NOW,
        max_age_seconds=30 * 60 * 60,
        future_tolerance_seconds=300,
        max_timestamp_skew_seconds=5,
        max_metadata_bytes=16_384,
        max_attempt_records=100,
        max_attempt_bytes=4_096,
        max_artifacts=100,
        max_artifact_bytes=10_000_000,
        max_uncompressed_bytes=10_000_000,
        total_timeout_seconds=5.0,
    )

    assert codes(results) == {"metadata_invalid_json"}


def test_metadata_symlink_is_rejected(tmp_path):
    status = write_fixture(tmp_path)
    external = tmp_path.parent / "external-status.json"
    external.write_bytes(status.read_bytes())
    status.unlink()
    status.symlink_to(external)

    results = verify(tmp_path)

    assert codes(results) == {"metadata_invalid_type"}


def test_newer_artifact_than_status_is_rejected(tmp_path):
    write_fixture(tmp_path)
    with gzip.open(tmp_path / "verdaxis-20260720-033000.sql.gz", "wb") as handle:
        handle.write(b"newer")

    results = verify(tmp_path)

    assert "status_not_newest" in codes(results)


def test_backup_id_timestamp_must_match_completed_at(tmp_path):
    write_fixture(tmp_path, backup_id="20260720-020000")

    results = verify(tmp_path)

    assert "backup_id_timestamp_mismatch" in codes(results)


def test_default_timestamp_skew_is_tight_for_single_instant_producer_contract():
    module = load_module()

    assert module.parse_args([]).max_timestamp_skew == 5


def test_finalized_artifact_mtime_must_match_completed_at(tmp_path):
    write_fixture(tmp_path)
    artifact = tmp_path / f"umami-{BACKUP_ID}.sql.gz"
    mismatched = (NOW - timedelta(hours=2)).timestamp()
    os.utime(artifact, (mismatched, mismatched))

    results = verify(tmp_path)

    assert "artifact_timestamp_mismatch" in codes(results)


def test_artifact_symlink_is_rejected(tmp_path):
    write_fixture(tmp_path)
    artifact = tmp_path / f"umami-{BACKUP_ID}.sql.gz"
    external = tmp_path.parent / "external.sql.gz"
    artifact.replace(external)
    artifact.symlink_to(external)

    results = verify(tmp_path)

    assert "artifact_invalid_type" in codes(results)


@pytest.mark.parametrize(("kind", "expected_code"), [("missing", "artifact_missing"), ("empty", "artifact_empty"), ("corrupt", "gzip_invalid")])
def test_missing_empty_and_corrupt_gzip_artifacts_fail(tmp_path, kind, expected_code):
    write_fixture(tmp_path)
    artifact = tmp_path / f"umami-{BACKUP_ID}.sql.gz"
    if kind == "missing":
        artifact.unlink()
    elif kind == "empty":
        artifact.write_bytes(b"")
    else:
        artifact.write_bytes(b"not gzip: do-not-copy")
        completed_at = (NOW - timedelta(hours=1)).timestamp()
        os.utime(artifact, (completed_at, completed_at))

    results = verify(tmp_path)

    assert expected_code in codes(results)
    assert "do-not-copy" not in json.dumps(results)


@pytest.mark.parametrize(
    ("content", "expected_code"),
    [
        (b"", "gzip_implausible"),
        (b"-- PostgreSQL database dump\n", "gzip_implausible"),
        (b"x" * 2048, "gzip_implausible"),
    ],
)
def test_valid_gzip_without_plausible_postgresql_dump_content_fails(
    tmp_path, content, expected_code
):
    write_fixture(tmp_path)
    artifact = tmp_path / f"umami-{BACKUP_ID}.sql.gz"
    with gzip.open(artifact, "wb") as handle:
        handle.write(content)
    completed_at = (NOW - timedelta(hours=1)).timestamp()
    os.utime(artifact, (completed_at, completed_at))

    assert expected_code in codes(verify(tmp_path))


def test_artifact_and_uncompressed_size_limits_are_enforced(tmp_path):
    write_fixture(tmp_path)

    compressed_limit = verify(tmp_path, max_artifact_bytes=4)
    expanded_limit = verify(tmp_path, max_uncompressed_bytes=4)

    assert "artifact_too_large" in codes(compressed_limit)
    assert "uncompressed_too_large" in codes(expanded_limit)


def test_status_write_is_atomic_0600(tmp_path):
    module = load_module()
    path = tmp_path / "state/status.json"
    payload = {"schema_version": 1, "generated_at": "2026-07-20T00:00:00Z", "category": "healthy", "checks": []}

    module.atomic_write_status(path, payload)

    assert json.loads(path.read_text()) == payload
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    assert not list(path.parent.glob("*.tmp"))


@pytest.mark.parametrize(("category", "exit_code"), [("healthy", 0), ("warning", 2), ("failure", 2)])
def test_main_exit_code(tmp_path, monkeypatch, category, exit_code):
    module = load_module()
    payload = {"schema_version": 1, "generated_at": "2026-07-20T00:00:00Z", "category": category, "checks": []}
    monkeypatch.setattr(module, "run_monitor", lambda _args: payload)
    path = tmp_path / "status.json"

    assert module.main(["--output", str(path)]) == exit_code
    assert json.loads(path.read_text())["category"] == category


def test_checks_null_state_is_quarantined_and_fails_closed_before_verification(tmp_path):
    module = load_module()
    path = tmp_path / "status.json"
    path.write_text('{"schema_version":1,"checks":null}')

    payload = module.run_monitor(SimpleNamespace(output=str(path)))

    assert payload["category"] == "failure"
    assert payload["checks"][0]["code"] == "previous_status_invalid"
    assert not path.exists()
    assert len(list(tmp_path.glob("status.json.invalid-*"))) == 1


def test_main_publishes_fail_closed_status_without_unsafe_builder(tmp_path, monkeypatch):
    module = load_module()
    path = tmp_path / "status.json"
    monkeypatch.setattr(
        module,
        "run_monitor",
        lambda _args: (_ for _ in ()).throw(RuntimeError("secret")),
    )
    monkeypatch.setattr(
        module,
        "build_status",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(AssertionError("unsafe builder reused")),
    )

    result = module.main(["--output", str(path)])

    saved = json.loads(path.read_text())
    assert result == 2
    assert saved["category"] == "failure"
    assert saved["checks"][0]["code"] == "fatal_error"
    assert "secret" not in json.dumps(saved)


def test_source_has_no_database_cloud_or_environment_credentials():
    source = MODULE_PATH.read_text().lower()

    for forbidden in (
        "subprocess",
        "pg_dump",
        "psql",
        "b2 ",
        "backup-env",
        "password",
        "database_url",
        "os.environ",
        "os.getenv",
    ):
        assert forbidden not in source
