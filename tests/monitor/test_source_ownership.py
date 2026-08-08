from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MONITOR = ROOT / "deploy/monitor"
MANIFEST = MONITOR / "artifact-manifest.json"
# Integration note (2026-07-20): on hardening/local-monitor-v2 this compared
# runtime-owned files against commit f31736d and proved the monitor branch
# never touched runtime-owned source. On the integration tree those files
# legitimately change in reviewed stages, and a single-commit stage cannot
# pin its own commit hash, so the tripwire pins per-file git blob SHAs
# instead. The invariant is unchanged: no monitor/test/tooling work may
# modify a runtime-owned file silently — a stage that deliberately changes
# one must update its pinned blob in the same reviewed commit.
# Market Support adds feature-flag settings and one feature-flagged router.
RUNTIME_OWNED_BLOBS = {
    "app/config.py": "1d32e6c523943752f1404998938a8bcd8ca372f7",
    "app/database.py": "43456b2c2079c07368359d4d54f5ce9629c9b57b",
    "app/main.py": "ee803a97bfb8739d3c660fb714a8fe4ca96f8dc8",
    "scripts/deploy.sh": "ec47368a175ecbc05db905a314cdd34fdc2ef44a",
    "scripts/run_demo_activity.py": "aac20936cd1b01adcebf8097762cae136b6eb0bf",
}

EXPECTED_ARTIFACTS = {
    "alert.env.example": (
        "/usr/share/doc/verdaxis-monitor/alert.env.example",
        "0644",
    ),
    "alert_dispatch.py": (
        "/usr/local/libexec/verdaxis-monitor/alert_dispatch.py",
        "0755",
    ),
    "backup_verify.py": (
        "/usr/local/libexec/verdaxis-monitor/backup_verify.py",
        "0755",
    ),
    "legacy_retirement.py": (
        "/usr/local/libexec/verdaxis-monitor/legacy_retirement.py",
        "0755",
    ),
    "local_health_check.py": (
        "/usr/local/libexec/verdaxis-monitor/local_health_check.py",
        "0755",
    ),
    "outbox_backlog_probe.py": (
        "/usr/local/libexec/verdaxis-monitor/outbox_backlog_probe.py",
        "0755",
    ),
    "retirement-evidence.example.json": (
        "/usr/share/doc/verdaxis-monitor/retirement-evidence.example.json",
        "0644",
    ),
    "runtime-identity.example": (
        "/usr/share/doc/verdaxis-monitor/runtime-identity.example",
        "0644",
    ),
    "runtime-v2-readiness-corpus.json": (
        "/usr/share/doc/verdaxis-monitor/runtime-v2-readiness-corpus.json",
        "0644",
    ),
    "status_state.py": (
        "/usr/local/libexec/verdaxis-monitor/status_state.py",
        "0644",
    ),
    "verdaxis-backup-verify.service": (
        "/etc/systemd/system/verdaxis-backup-verify.service",
        "0644",
    ),
    "verdaxis-backup-verify.timer": (
        "/etc/systemd/system/verdaxis-backup-verify.timer",
        "0644",
    ),
    "verdaxis-demo-activity-staging.service": (
        "/etc/systemd/system/verdaxis-demo-activity-staging.service",
        "0644",
    ),
    "verdaxis-demo-activity-staging.timer": (
        "/etc/systemd/system/verdaxis-demo-activity-staging.timer",
        "0644",
    ),
    "verdaxis-demo-activity.service": (
        "/etc/systemd/system/verdaxis-demo-activity.service",
        "0644",
    ),
    "verdaxis-demo-activity.timer": (
        "/etc/systemd/system/verdaxis-demo-activity.timer",
        "0644",
    ),
    "verdaxis-health.service": (
        "/etc/systemd/system/verdaxis-health.service",
        "0644",
    ),
    "verdaxis-health.timer": (
        "/etc/systemd/system/verdaxis-health.timer",
        "0644",
    ),
    "verdaxis-monitor-alert-reminder.service": (
        "/etc/systemd/system/verdaxis-monitor-alert-reminder.service",
        "0644",
    ),
    "verdaxis-monitor-alert-reminder.timer": (
        "/etc/systemd/system/verdaxis-monitor-alert-reminder.timer",
        "0644",
    ),
    "verdaxis-monitor-alert@.service": (
        "/etc/systemd/system/verdaxis-monitor-alert@.service",
        "0644",
    ),
    "verdaxis-monitor.sysusers": (
        "/usr/lib/sysusers.d/verdaxis-monitor.conf",
        "0644",
    ),
    "verdaxis-monitor.tmpfiles": (
        "/usr/lib/tmpfiles.d/verdaxis-monitor.conf",
        "0644",
    ),
    "verify_runtime_identity.py": (
        "/usr/local/libexec/verdaxis-monitor/verify_runtime_identity.py",
        "0755",
    ),
}


def load_manifest() -> dict:
    payload = json.loads(MANIFEST.read_text())
    assert set(payload) == {"schema_version", "artifacts"}
    assert type(payload["schema_version"]) is int
    assert payload["schema_version"] == 1
    assert isinstance(payload["artifacts"], list)
    return payload


def test_monitor_has_no_competing_installer_runtime_transaction_or_producer_unit():
    forbidden = {
        "PILOT-RUNBOOK.patch",
        "artifact-manifest.sha256",
        "backup_producer.py",
        "backup_producer_permissions.py",
        "install.sh",
        "install_transaction.py",
        "runtime_identity.py",
        "verdaxis-backup.service",
    }

    assert not {path.name for path in MONITOR.iterdir()} & forbidden


def test_runtime_owned_source_matches_pinned_blobs():
    for relative_path, expected in RUNTIME_OWNED_BLOBS.items():
        content = (ROOT / relative_path).read_bytes()
        blob = hashlib.sha1(b"blob %d\x00" % len(content) + content).hexdigest()
        assert blob == expected, relative_path


def test_static_manifest_is_exact_byte_attested_and_non_promoting():
    payload = load_manifest()
    artifacts = payload["artifacts"]
    assert [artifact["source"] for artifact in artifacts] == sorted(EXPECTED_ARTIFACTS)
    assert all(
        isinstance(artifact, dict)
        and set(artifact) == {"source", "destination", "mode", "sha256"}
        for artifact in artifacts
    )

    observed = {}
    destinations = set()
    for artifact in artifacts:
        source = artifact["source"]
        destination = artifact["destination"]
        mode = artifact["mode"]
        digest = artifact["sha256"]
        assert isinstance(source, str) and Path(source).name == source
        assert isinstance(destination, str) and destination.startswith("/")
        assert ".." not in Path(destination).parts
        assert destination not in destinations
        assert isinstance(mode, str) and re.fullmatch(r"0[67][045][045]", mode)
        assert isinstance(digest, str) and re.fullmatch(r"[0-9a-f]{64}", digest)
        source_path = MONITOR / source
        assert source_path.is_file() and not source_path.is_symlink()
        assert hashlib.sha256(source_path.read_bytes()).hexdigest() == digest
        destinations.add(destination)
        observed[source] = (destination, mode)

    assert observed == EXPECTED_ARTIFACTS
def test_every_remaining_unit_is_in_the_static_inventory():
    payload = load_manifest()
    sources = {artifact["source"] for artifact in payload["artifacts"]}
    units = {
        path.name
        for path in MONITOR.iterdir()
        if path.suffix in {".service", ".timer"}
    }

    assert units == {source for source in sources if Path(source).suffix in {".service", ".timer"}}


def test_monitor_tree_contains_no_activation_or_root_promotion_implementation():
    for path in MONITOR.glob("*.py"):
        source = path.read_text()
        if path.name != "legacy_retirement.py":
            assert "/usr/bin/systemctl" not in source
            assert "daemon-reload" not in source
            assert "systemctl enable" not in source
        assert "/etc/systemd/system" not in source
        assert "/usr/local/libexec" not in source

    assert not list(MONITOR.glob("install*"))


def test_monitor_python_does_not_import_application_runtime_modules():
    for path in MONITOR.glob("*.py"):
        source = path.read_text()
        assert "from app" not in source
        assert "import app" not in source
