from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
UNIT_DIR = ROOT / "deploy/monitor"


def unit(name: str) -> str:
    path = UNIT_DIR / name
    assert path.exists(), f"missing {name}"
    return path.read_text()


@pytest.mark.parametrize(
    "name",
    [
        "verdaxis-health.service",
        "verdaxis-backup-verify.service",
    ],
)
def test_reader_services_have_strict_common_sandbox(name):
    text = unit(name)

    for directive in (
        "Type=oneshot",
        "NoNewPrivileges=yes",
        "PrivateTmp=yes",
        "PrivateDevices=yes",
        "ProtectSystem=strict",
        "ProtectKernelTunables=yes",
        "ProtectKernelModules=yes",
        "ProtectKernelLogs=yes",
        "ProtectControlGroups=yes",
        "ProtectProc=invisible",
        "ProcSubset=pid",
        "RestrictNamespaces=yes",
        "RestrictSUIDSGID=yes",
        "LockPersonality=yes",
        "MemoryDenyWriteExecute=yes",
        "CapabilityBoundingSet=",
        "AmbientCapabilities=",
        "SystemCallArchitectures=native",
        "SystemCallFilter=@system-service",
        "DevicePolicy=closed",
        "UMask=0077",
        "TasksMax=",
        "MemoryMax=",
        "TimeoutStartSec=",
    ):
        assert directive in text
    assert "/verdaxis/prod/be/.env" not in text
    assert "/verdaxis/staging/be/.env" not in text
    if name == "verdaxis-backup-verify.service":
        assert "EnvironmentFile=" not in text


def test_health_service_is_dedicated_loopback_only_and_state_scoped():
    text = unit("verdaxis-health.service")

    assert "User=verdaxis-health" in text
    assert "Group=verdaxis-health" in text
    assert "StateDirectory=verdaxis-health" in text
    assert "IPAddressDeny=any" in text
    assert "IPAddressAllow=127.0.0.0/8" in text
    assert "RestrictAddressFamilies=AF_UNIX AF_INET" in text
    assert "local_health_check.py" in text
    assert "ProtectHome=yes" in text
    assert "EnvironmentFile=" not in text
    assert "--production-identity-file /etc/verdaxis-monitor/runtime-identities/production.identity" in text
    assert "--staging-identity-file /etc/verdaxis-monitor/runtime-identities/staging.identity" in text
    assert "OnFailure=verdaxis-monitor-alert@health-failure.service" in text
    assert "OnSuccess=verdaxis-monitor-alert@health-healthy.service" in text


def test_backup_service_has_no_network_and_exposes_only_backup_directory():
    text = unit("verdaxis-backup-verify.service")

    assert "User=verdaxis-backup-reader" in text
    assert "Group=verdaxis-backup-readers" in text
    assert "StateDirectory=verdaxis-backup-verify" in text
    assert "PrivateNetwork=yes" in text
    assert "ProtectHome=tmpfs" in text
    assert "BindReadOnlyPaths=/home/verdaxis-prod/backups" in text
    assert "InaccessiblePaths=/home/verdaxis-prod/.backup-env" in text
    assert "backup_verify.py" in text
    assert "--attempt-directory /home/verdaxis-prod/backups/attempts" in text
    assert "OnFailure=verdaxis-monitor-alert@backup-failure.service" in text
    assert "OnSuccess=verdaxis-monitor-alert@backup-healthy.service" in text


def test_monitor_does_not_supply_a_backup_producer_service_or_timer():
    assert not (UNIT_DIR / "verdaxis-backup.service").exists()
    assert not (UNIT_DIR / "verdaxis-backup.timer").exists()


@pytest.mark.parametrize(
    "name",
    [
        "verdaxis-health.timer",
        "verdaxis-backup-verify.timer",
        "verdaxis-demo-activity.timer",
        "verdaxis-demo-activity-staging.timer",
    ],
)
def test_timers_are_persistent_and_explicitly_bound(name):
    text = unit(name)

    assert "Persistent=true" in text
    assert "Unit=" in text
    assert "WantedBy=timers.target" in text


@pytest.mark.parametrize(
    (
        "name",
        "timer_name",
        "environment",
        "backend_dir",
        "user",
        "other_environment",
        "other_backend_dir",
        "other_service",
    ),
    [
        (
            "verdaxis-demo-activity.service",
            "verdaxis-demo-activity.timer",
            "production",
            "/home/verdaxis-prod/verdaxis/prod/be",
            "verdaxis-demo-production",
            "staging",
            "/home/verdaxis-prod/verdaxis/staging/be",
            "verdaxis-demo-activity-staging.service",
        ),
        (
            "verdaxis-demo-activity-staging.service",
            "verdaxis-demo-activity-staging.timer",
            "staging",
            "/home/verdaxis-prod/verdaxis/staging/be",
            "verdaxis-demo-staging",
            "production",
            "/home/verdaxis-prod/verdaxis/prod/be",
            "verdaxis-demo-activity.service",
        ),
    ],
)
def test_demo_activity_loads_exact_per_environment_identity_and_credentials(
    name,
    timer_name,
    environment,
    backend_dir,
    user,
    other_environment,
    other_backend_dir,
    other_service,
):
    text = unit(name)
    timer = unit(timer_name)

    assert f"User={user}" in text
    assert f"Group={user}" in text
    assert f"EnvironmentFile=/etc/verdaxis-monitor/demo/{environment}.env" in text
    assert f"EnvironmentFile={backend_dir}/.runtime-release.env" in text
    assert f"EnvironmentFile={backend_dir}/.env" not in text
    assert f"Environment=ENVIRONMENT={environment}" in text
    assert "ExecStartPre=" not in text
    assert "RuntimeDirectory=verdaxis-demo-" in text
    assert f"ConditionPathExists=!{backend_dir}/.runtime-deploy/{environment}.state" in text
    assert f"BindReadOnlyPaths={backend_dir}" in text
    assert (
        f"ExecStart=/usr/bin/env ENVIRONMENT={environment} /usr/bin/python3 "
        f"/usr/local/libexec/verdaxis-monitor/verify_runtime_identity.py "
        f"--source-directory {backend_dir} --expected-environment {environment} "
        f"--expected-release-sha ${{RELEASE_SHA}} --python-executable "
        f"{backend_dir}/venv/bin/python --script scripts/run_demo_activity.py "
        "--runtime-directory %t/verdaxis-demo-"
    ) in text
    assert "development" not in text.lower()
    assert "sudo" not in text.lower()
    assert "CapabilityBoundingSet=" in text
    assert "NoNewPrivileges=yes" in text
    assert "ProtectSystem=strict" in text
    assert "ProtectHome=tmpfs" in text
    assert "IPAddressDeny=any" in text
    assert "IPAddressAllow=127.0.0.0/8" in text
    assert "TimeoutStartSec=" in text
    assert "ReadWritePaths=" not in text
    assert "/home/verdaxis-prod/.backup-env" not in text
    assert f"/etc/verdaxis-monitor/demo/{other_environment}.env" not in text
    assert other_backend_dir not in text
    assert other_service not in text
    assert f"Unit={name}" in timer
    assert other_service not in timer
    assert "Persistent=true" in timer


def test_demo_activity_services_are_independent_without_cross_environment_ordering():
    production = unit("verdaxis-demo-activity.service")
    staging = unit("verdaxis-demo-activity-staging.service")

    assert "verdaxis-demo-activity-staging" not in production
    assert "verdaxis-demo-activity.service" not in staging
    assert "Requires=verdaxis-demo" not in production + staging
    assert "PartOf=verdaxis-demo" not in production + staging
    assert "BindsTo=verdaxis-demo" not in production + staging


def test_alert_consumer_is_separate_credential_bearing_identity_with_hourly_reminders():
    service = unit("verdaxis-monitor-alert@.service")
    reminder_service = unit("verdaxis-monitor-alert-reminder.service")
    reminder = unit("verdaxis-monitor-alert-reminder.timer")

    for alert_unit in (service, reminder_service):
        assert "User=verdaxis-monitor-alert" in alert_unit
        assert "Group=verdaxis-monitor-alert" in alert_unit
        assert "EnvironmentFile=/etc/verdaxis-monitor/alert.env" in alert_unit
        for directive in (
            "NoNewPrivileges=yes",
            "CapabilityBoundingSet=",
            "AmbientCapabilities=",
            "PrivateTmp=yes",
            "PrivateDevices=yes",
            "PrivateMounts=yes",
            "ProtectSystem=strict",
            "ProtectHome=yes",
            "ProtectClock=yes",
            "ProtectHostname=yes",
            "ProtectKernelTunables=yes",
            "ProtectKernelModules=yes",
            "ProtectKernelLogs=yes",
            "ProtectControlGroups=yes",
            "ProtectProc=invisible",
            "ProcSubset=pid",
            "RestrictNamespaces=yes",
            "RestrictSUIDSGID=yes",
            "RestrictRealtime=yes",
            "LockPersonality=yes",
            "MemoryDenyWriteExecute=yes",
            "RemoveIPC=yes",
            "KeyringMode=private",
            "DevicePolicy=closed",
        ):
            assert directive in alert_unit
    assert "alert_dispatch.py --event %i" in service
    assert "alert_dispatch.py --remind all" in reminder_service
    assert reminder_service.count("ExecStart=") == 1
    assert "OnCalendar=hourly" in reminder
    assert "Persistent=true" in reminder


def test_identities_and_directories_are_declarative_and_rerunnable():
    sysusers = unit("verdaxis-monitor.sysusers")
    tmpfiles = unit("verdaxis-monitor.tmpfiles")

    for identity in (
        "verdaxis-health",
        "verdaxis-backup-reader",
        "verdaxis-monitor-alert",
        "verdaxis-demo-production",
        "verdaxis-demo-staging",
    ):
        assert identity in sysusers
    assert "m verdaxis-prod verdaxis-backup-readers" in sysusers
    assert "d /var/lib/verdaxis-monitor-alert 0700 verdaxis-monitor-alert verdaxis-monitor-alert" in tmpfiles
    assert "d /var/lib/verdaxis-monitor 0700 root root" in tmpfiles
    assert "d /etc/verdaxis-monitor/demo 0711 root root" in tmpfiles
    assert "z /etc/verdaxis-monitor/alert.env 0600 root root" in tmpfiles
    assert (
        "z /etc/verdaxis-monitor/demo/production.env 0600 verdaxis-demo-production verdaxis-demo-production"
        in tmpfiles
    )
    assert (
        "z /etc/verdaxis-monitor/demo/staging.env 0600 verdaxis-demo-staging verdaxis-demo-staging"
        in tmpfiles
    )
    for external_owner_path in (
        "/home/verdaxis-prod/backups",
        "/home/verdaxis-prod/.backup-env",
        "/home/verdaxis-prod/verdaxis/prod/be/.env",
        "/home/verdaxis-prod/verdaxis/staging/be/.env",
        ".runtime-release.env",
    ):
        assert external_owner_path not in tmpfiles
    assert "m verdaxis-demo-production verdaxis-backup-readers" not in sysusers
    assert "m verdaxis-demo-staging verdaxis-backup-readers" not in sysusers
