from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MONITOR = ROOT / "deploy/monitor"
MANIFEST = MONITOR / "artifact-manifest.json"
# The runtime deploy transaction remains byte-pinned across monitor changes.
# The removed Python application and demo launcher no longer own runtime source.
RUNTIME_OWNED_BLOBS = {
    "scripts/deploy.sh": "1f104f3353dc75b8e127a079df639f5dc313e3c2",
}
REMOVED_API_SOURCES = (
    "app/config.py",
    "app/database.py",
    "app/main.py",
    "scripts/run_demo_activity.py",
)

# Task 10b: only native active units and their reviewed policy/docs are installed.
EXPECTED_ARTIFACTS = {
    "../external_monitor/systemd/verdaxis-monitor-verify.service": (
        "/etc/systemd/system/verdaxis-monitor-verify.service",
        "0644",
    ),
    "../external_monitor/systemd/verdaxis-monitor.service": (
        "/etc/systemd/system/verdaxis-monitor.service",
        "0644",
    ),
    "../external_monitor/systemd/verdaxis-monitor.timer": (
        "/etc/systemd/system/verdaxis-monitor.timer",
        "0644",
    ),
    "../external_monitor/systemd/verdaxis-onboarding-attention.service": (
        "/etc/systemd/system/verdaxis-onboarding-attention.service",
        "0644",
    ),
    "../external_monitor/systemd/verdaxis-onboarding-attention.timer": (
        "/etc/systemd/system/verdaxis-onboarding-attention.timer",
        "0644",
    ),
    "../external_monitor/verdaxis-monitor.env.example": (
        "/usr/share/doc/verdaxis-monitor/verdaxis-monitor.env.example",
        "0644",
    ),
    "../external_monitor/verdaxis-render.sysusers": (
        "/usr/lib/sysusers.d/verdaxis-render.conf",
        "0644",
    ),
    "../external_monitor/verdaxis-render.tmpfiles": (
        "/usr/lib/tmpfiles.d/verdaxis-render.conf",
        "0644",
    ),
    "alert.env.example": (
        "/usr/share/doc/verdaxis-monitor/alert.env.example",
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
    "verdaxis-monitor-handoff.acl.tmpfiles": (
        "/usr/lib/tmpfiles.d/verdaxis-monitor-handoff-acl.conf",
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
}


def load_manifest() -> dict:
    payload = json.loads(MANIFEST.read_text())
    assert set(payload) == {"schema_version", "artifacts", "binary"}
    assert type(payload["schema_version"]) is int
    assert payload["schema_version"] == 1
    assert isinstance(payload["artifacts"], list)
    # Binary identity is supplied from the verified release record at install
    # time, never guessed from source or an installed-host metadata file.
    assert payload["binary"] == {
        "source": ".runtime-rust/verdaxis-read",
        "destination": "/usr/local/libexec/verdaxis/verdaxis-read",
        "mode": "0755",
    }
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
    for relative_path in REMOVED_API_SOURCES:
        assert not (ROOT / relative_path).exists(), relative_path
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
        assert isinstance(source, str) and source in EXPECTED_ARTIFACTS
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

    assert units == {source for source in sources if "/" not in source and Path(source).suffix in {".service", ".timer"}}


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
