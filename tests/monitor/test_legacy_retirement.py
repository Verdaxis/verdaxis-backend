from __future__ import annotations

import importlib.util
import itertools
import json
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "deploy/monitor/legacy_retirement.py"
START = datetime(2026, 7, 20, tzinfo=timezone.utc)
END = START + timedelta(hours=30)
NOW = END + timedelta(minutes=2)
CONFIRMATION = "RETIRE_LEGACY_AFTER_PROVEN_DUAL_RUN"
TIMER_UNITS = (
    "verdaxis-health.timer",
    "verdaxis-backup-verify.timer",
    "verdaxis-monitor-alert-reminder.timer",
    "verdaxis-demo-activity.timer",
    "verdaxis-demo-activity-staging.timer",
    "verdaxis-monitor.timer",
)


def load_module():
    assert MODULE_PATH.exists(), "guarded legacy retirement command is missing"
    spec = importlib.util.spec_from_file_location("legacy_retirement", MODULE_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def utc(value: datetime) -> str:
    return value.isoformat(timespec="seconds").replace("+00:00", "Z")


def expected_matrix(
    telegram: bool = True,
    healthchecks_health: bool = True,
    healthchecks_backup: bool = True,
) -> frozenset[tuple[str, str]]:
    pairs = set()
    if telegram:
        pairs.update(
            {
                ("telegram", "health-failure"),
                ("telegram", "health-healthy"),
                ("telegram", "backup-failure"),
                ("telegram", "backup-healthy"),
            }
        )
    if healthchecks_health:
        pairs.update(
            {
                ("healthchecks_health", "health-failure"),
                ("healthchecks_health", "health-healthy"),
            }
        )
    if healthchecks_backup:
        pairs.update(
            {
                ("healthchecks_backup", "backup-failure"),
                ("healthchecks_backup", "backup-healthy"),
            }
        )
    return frozenset(pairs)


def config_text(
    *,
    telegram: bool = True,
    healthchecks_health: bool = True,
    healthchecks_backup: bool = True,
) -> str:
    lines = ["# values are intentionally irrelevant to retirement proof"]
    if telegram:
        lines.extend(
            [
                "TELEGRAM_BOT_TOKEN=configured",
                "TELEGRAM_CHAT_ID=configured",
            ]
        )
    if healthchecks_health:
        lines.append("HEALTHCHECKS_HEALTH_URL=configured")
    if healthchecks_backup:
        lines.append("HEALTHCHECKS_BACKUP_URL=configured")
    return "\n".join(lines) + "\n"


def valid_evidence(
    matrix: frozenset[tuple[str, str]] | None = None,
    start: datetime = START,
) -> dict:
    matrix = matrix if matrix is not None else expected_matrix()
    history = []
    current = start
    while current <= start + timedelta(hours=30):
        history.append(
            {
                "observed_at": utc(current),
                "local_health": "healthy",
                "backup": "healthy",
                "legacy_monitor": "healthy",
            }
        )
        current += timedelta(minutes=5)
    history_end = start + timedelta(hours=30)
    signed = history_end + timedelta(minutes=1)
    receipts = [
        {
            "destination": destination,
            "event": event,
            "receipt_id": f"{index + 1:032x}",
            "delivered_at": utc(
                history_end
                if event.endswith("-healthy")
                else history_end - timedelta(minutes=1)
            ),
        }
        for index, (destination, event) in enumerate(sorted(matrix))
    ]
    return {
        "schema_version": 2,
        "first_green_at": utc(start),
        "history": history,
        "local_alert_delivery": {
            "signed_at": utc(signed),
            "signer": "operator@example.invalid",
            "hourly_dedupe": True,
            "receipts": receipts,
        },
        "external_monitor": {
            "signed_at": utc(signed),
            "signer": "operator@example.invalid",
            "coverage": {
                "vercel_production_frontend": True,
                "caddy_production_api": True,
                "caddy_staging_api": True,
                "caddy_staging_frontend": True,
                "dns": True,
                "tls": True,
                "rendered_page": True,
                "alert_delivery": True,
                "signup_canary": True,
                "analytics_ingestion_canary": True,
            },
        },
        "scheduler": {
            "signed_at": utc(signed),
            "signer": "operator@example.invalid",
            "one_demo_scheduler_owner": True,
            "legacy_monitor_active_during_history": True,
        },
    }


def write_evidence(path: Path, payload: dict) -> Path:
    path.write_text(json.dumps(payload))
    return path


def healthy_status(name: str) -> dict:
    return {
        "schema_version": 1,
        "generated_at": utc(END + timedelta(minutes=1)),
        "category": "healthy",
        "checks": [
            {
                "name": name,
                "category": "healthy",
                "code": "ok",
                "duration_ms": 1,
                "last_success_at": utc(END + timedelta(minutes=1)),
            }
        ],
    }


def write_alert_states(directory: Path, payload: dict) -> None:
    directory.mkdir()
    states = {
        "health": {
            "schema_version": 3,
            "state": "healthy",
            "observed_at": payload["local_alert_delivery"]["signed_at"],
            "receipts": {},
        },
        "backup": {
            "schema_version": 3,
            "state": "healthy",
            "observed_at": payload["local_alert_delivery"]["signed_at"],
            "receipts": {},
        },
    }
    for proof in payload["local_alert_delivery"]["receipts"]:
        check, state = proof["event"].rsplit("-", 1)
        persisted_destination = (
            "telegram" if proof["destination"] == "telegram" else "healthchecks"
        )
        destination_receipts = states[check]["receipts"].setdefault(
            persisted_destination, {}
        )
        destination_receipts[state] = {
            "receipt_id": proof["receipt_id"],
            "delivered_at": proof["delivered_at"],
        }
    for check, state in states.items():
        (directory / f"{check}.json").write_text(json.dumps(state))


def write_live_proof(
    tmp_path: Path,
    payload: dict,
    *,
    telegram: bool = True,
    healthchecks_health: bool = True,
    healthchecks_backup: bool = True,
) -> dict:
    alert_config = tmp_path / "alert.env"
    alert_config.write_text(
        config_text(
            telegram=telegram,
            healthchecks_health=healthchecks_health,
            healthchecks_backup=healthchecks_backup,
        )
    )
    alert_state = tmp_path / "alert-state"
    write_alert_states(alert_state, payload)
    health_status = tmp_path / "health-status.json"
    backup_status = tmp_path / "backup-status.json"
    health_status.write_text(json.dumps(healthy_status("prod_ready")))
    backup_status.write_text(json.dumps(healthy_status("backup_attempt")))
    return {
        "alert_config_path": alert_config,
        "alert_state_directory": alert_state,
        "health_status_path": health_status,
        "backup_status_path": backup_status,
    }


@pytest.mark.parametrize(
    "telegram,healthchecks_health,healthchecks_backup",
    list(itertools.product((False, True), repeat=3)),
)
def test_config_key_presence_derives_the_complete_destination_event_matrix(
    tmp_path, telegram, healthchecks_health, healthchecks_backup
):
    retirement = load_module()
    config = tmp_path / "alert.env"
    config.write_text(
        config_text(
            telegram=telegram,
            healthchecks_health=healthchecks_health,
            healthchecks_backup=healthchecks_backup,
        )
    )
    health_covered = telegram or healthchecks_health
    backup_covered = telegram or healthchecks_backup

    if not health_covered or not backup_covered:
        with pytest.raises(retirement.RetirementRefused, match="coverage"):
            retirement.configured_receipt_matrix(config)
    else:
        matrix = retirement.configured_receipt_matrix(config)
        assert matrix == expected_matrix(
            telegram, healthchecks_health, healthchecks_backup
        )
        assert "configured" not in repr(matrix)


def test_config_presence_treats_values_as_opaque_and_never_retains_them(tmp_path):
    retirement = load_module()
    config = tmp_path / "alert.env"
    secret_fragment = "must-not-appear"
    config.write_text(
        "TELEGRAM_BOT_TOKEN=token=with=padding\n"
        "TELEGRAM_CHAT_ID=chat=identifier\n"
        f"HEALTHCHECKS_HEALTH_URL=https://example.invalid/ping?token={secret_fragment}=x\n"
        "HEALTHCHECKS_BACKUP_URL=https://example.invalid/ping?token=backup=y\n"
    )

    matrix = retirement.configured_receipt_matrix(config)

    assert matrix == expected_matrix()
    assert secret_fragment not in repr(matrix)


@pytest.mark.parametrize(
    "raw",
    [
        "TELEGRAM_BOT_TOKEN=configured\n",
        "TELEGRAM_CHAT_ID=configured\n",
        "TELEGRAM_BOT_TOKEN=configured\nTELEGRAM_BOT_TOKEN=again\nTELEGRAM_CHAT_ID=configured\n",
        "UNKNOWN_ALERT_KEY=configured\nHEALTHCHECKS_HEALTH_URL=configured\nHEALTHCHECKS_BACKUP_URL=configured\n",
    ],
)
def test_config_presence_rejects_partial_telegram_duplicate_or_unknown_keys(
    tmp_path, raw
):
    retirement = load_module()
    config = tmp_path / "alert.env"
    config.write_text(raw)

    with pytest.raises(retirement.RetirementRefused):
        retirement.configured_receipt_matrix(config)


def test_valid_evidence_and_durable_receipts_validate_without_retiring(tmp_path):
    retirement = load_module()
    payload = valid_evidence()
    evidence = write_evidence(tmp_path / "evidence.json", payload)
    live = write_live_proof(tmp_path, payload)
    calls = []

    result = retirement.guarded_retirement(
        evidence,
        execute=False,
        confirmation=None,
        runner=lambda command: calls.append(command),
        now=NOW,
        **live,
    )

    assert result == "validated"
    assert calls == []


@pytest.mark.parametrize("check", ["health", "backup"])
def test_retirement_requires_healthy_durable_alert_state(tmp_path, check):
    retirement = load_module()
    payload = valid_evidence()
    evidence = write_evidence(tmp_path / "evidence.json", payload)
    live = write_live_proof(tmp_path, payload)
    state_path = live["alert_state_directory"] / f"{check}.json"
    state = json.loads(state_path.read_text())
    state["state"] = "failure"
    state_path.write_text(json.dumps(state))

    with pytest.raises(retirement.RetirementRefused, match="healthy|receipt state"):
        retirement.guarded_retirement(
            evidence, execute=False, confirmation=None, now=NOW, **live
        )


@pytest.mark.parametrize(
    ("check", "destination"),
    [
        ("health", "telegram"),
        ("health", "healthchecks"),
        ("backup", "telegram"),
        ("backup", "healthchecks"),
    ],
)
def test_every_destination_recovery_must_be_newer_than_its_failure(
    tmp_path, check, destination
):
    retirement = load_module()
    payload = valid_evidence()
    evidence = write_evidence(tmp_path / "evidence.json", payload)
    live = write_live_proof(tmp_path, payload)
    state_path = live["alert_state_directory"] / f"{check}.json"
    state = json.loads(state_path.read_text())
    state["receipts"][destination]["healthy"]["delivered_at"] = state["receipts"][
        destination
    ]["failure"]["delivered_at"]
    state_path.write_text(json.dumps(state))
    for receipt in payload["local_alert_delivery"]["receipts"]:
        if receipt["destination"] == (
            "telegram" if destination == "telegram" else f"healthchecks_{check}"
        ) and receipt["event"] == f"{check}-healthy":
            receipt["delivered_at"] = state["receipts"][destination]["healthy"][
                "delivered_at"
            ]
    evidence.write_text(json.dumps(payload))

    with pytest.raises(retirement.RetirementRefused, match="recovery|newer"):
        retirement.guarded_retirement(
            evidence, execute=False, confirmation=None, now=NOW, **live
        )


@pytest.mark.parametrize(
    "change",
    [
        lambda payload: payload.update(first_green_at=utc(START + timedelta(minutes=5))),
        lambda payload: payload.update(schema_version=True),
        lambda payload: payload.update(history=payload["history"][:-13]),
        lambda payload: payload["history"].pop(100),
        lambda payload: payload["history"][100].update(local_health="failure"),
        lambda payload: payload["local_alert_delivery"].update(hourly_dedupe=False),
        lambda payload: payload["local_alert_delivery"]["receipts"].pop(),
        lambda payload: payload["local_alert_delivery"]["receipts"][0].update(
            receipt_id=[]
        ),
        lambda payload: payload["external_monitor"]["coverage"].update(tls=False),
        lambda payload: payload["external_monitor"]["coverage"].update(
            signup_canary=False
        ),
        lambda payload: payload["external_monitor"]["coverage"].update(
            analytics_ingestion_canary=False
        ),
        lambda payload: payload["scheduler"].update(one_demo_scheduler_owner=False),
        lambda payload: payload["scheduler"].update(
            legacy_monitor_active_during_history=False
        ),
    ],
)
def test_retirement_refuses_incomplete_history_receipts_or_signoff(tmp_path, change):
    retirement = load_module()
    payload = valid_evidence()
    change(payload)
    evidence = write_evidence(tmp_path / "evidence.json", payload)
    live = write_live_proof(tmp_path, valid_evidence())

    with pytest.raises(retirement.RetirementRefused):
        retirement.guarded_retirement(
            evidence,
            execute=True,
            confirmation=CONFIRMATION,
            runner=lambda _command: pytest.fail("retirement command must not run"),
            timer_checker=lambda _name: True,
            now=NOW,
            **live,
        )


@pytest.mark.parametrize("payload", [[], None, {"schema_version": 2}, {"state": []}])
def test_retirement_refuses_hostile_or_incomplete_json(tmp_path, payload):
    retirement = load_module()
    evidence = write_evidence(tmp_path / "evidence.json", payload)
    valid = valid_evidence()
    live = write_live_proof(tmp_path, valid)

    with pytest.raises(retirement.RetirementRefused):
        retirement.guarded_retirement(
            evidence,
            execute=True,
            confirmation=CONFIRMATION,
            runner=lambda _command: pytest.fail("retirement command must not run"),
            timer_checker=lambda _name: True,
            now=NOW,
            **live,
        )


def test_retirement_evidence_rejects_duplicate_json_keys(tmp_path):
    retirement = load_module()
    evidence = tmp_path / "evidence.json"
    raw = json.dumps(valid_evidence())
    evidence.write_text(
        raw.replace('"schema_version": 2', '"schema_version": 2, "schema_version": 2', 1)
    )

    with pytest.raises(retirement.RetirementRefused, match="missing or unsafe"):
        retirement.validate_evidence_file(
            evidence, required_receipts=expected_matrix(), now=NOW
        )


@pytest.mark.parametrize("target", ["alert", "status"])
def test_retirement_live_json_rejects_duplicate_keys(tmp_path, target):
    retirement = load_module()
    payload = valid_evidence()
    evidence = write_evidence(tmp_path / "evidence.json", payload)
    live = write_live_proof(tmp_path, payload)
    if target == "alert":
        path = live["alert_state_directory"] / "health.json"
        raw = path.read_text()
        path.write_text(
            raw.replace('"schema_version": 3', '"schema_version": 3, "schema_version": 3', 1)
        )
    else:
        path = live["health_status_path"]
        raw = path.read_text()
        path.write_text(
            raw.replace('"schema_version": 1', '"schema_version": 1, "schema_version": 1', 1)
        )

    with pytest.raises(retirement.RetirementRefused):
        retirement.guarded_retirement(
            evidence, execute=False, confirmation=None, now=NOW, **live
        )


def test_execute_requires_exact_confirmation_after_valid_evidence(tmp_path):
    retirement = load_module()
    payload = valid_evidence()
    evidence = write_evidence(tmp_path / "evidence.json", payload)
    live = write_live_proof(tmp_path, payload)

    with pytest.raises(retirement.RetirementRefused, match="confirmation"):
        retirement.guarded_retirement(
            evidence,
            execute=True,
            confirmation="yes",
            runner=lambda _command: pytest.fail("retirement command must not run"),
            timer_checker=lambda _name: True,
            now=NOW,
            **live,
        )


def test_valid_execute_rechecks_all_timers_immediately_before_fixed_command(tmp_path):
    retirement = load_module()
    payload = valid_evidence()
    evidence = write_evidence(tmp_path / "evidence.json", payload)
    live = write_live_proof(tmp_path, payload)
    calls = []

    result = retirement.guarded_retirement(
        evidence,
        execute=True,
        confirmation=CONFIRMATION,
        runner=lambda command: calls.append(("command", command)),
        timer_checker=lambda name: calls.append(("timer", name)) or True,
        now=NOW,
        **live,
    )

    assert result == "retired"
    assert calls == [
        *(("timer", name) for name in TIMER_UNITS),
        *(("timer", name) for name in TIMER_UNITS),
        (
            "command",
            [
                "/usr/bin/systemctl",
                "disable",
                "--now",
                "verdaxis-monitor.timer",
            ],
        ),
    ]


@pytest.mark.parametrize("inactive_timer", TIMER_UNITS)
def test_timer_becoming_inactive_at_final_revalidation_refuses_retirement(
    tmp_path, inactive_timer
):
    retirement = load_module()
    payload = valid_evidence()
    evidence = write_evidence(tmp_path / "evidence.json", payload)
    live = write_live_proof(tmp_path, payload)
    probes = {name: 0 for name in TIMER_UNITS}

    def check_timer(name):
        probes[name] += 1
        return not (name == inactive_timer and probes[name] == 2)

    with pytest.raises(retirement.RetirementRefused, match="timer"):
        retirement.guarded_retirement(
            evidence,
            execute=True,
            confirmation=CONFIRMATION,
            runner=lambda _command: pytest.fail("retirement command must not run"),
            timer_checker=check_timer,
            now=NOW,
            **live,
        )

    assert probes[inactive_timer] == 2


@pytest.mark.parametrize("inactive_timer", TIMER_UNITS)
def test_any_inactive_v2_or_legacy_timer_refuses_retirement(
    tmp_path, inactive_timer
):
    retirement = load_module()
    payload = valid_evidence()
    evidence = write_evidence(tmp_path / "evidence.json", payload)
    live = write_live_proof(tmp_path, payload)

    with pytest.raises(retirement.RetirementRefused, match="timer"):
        retirement.guarded_retirement(
            evidence,
            execute=True,
            confirmation=CONFIRMATION,
            runner=lambda _command: pytest.fail("retirement command must not run"),
            timer_checker=lambda name: name != inactive_timer,
            now=NOW,
            **live,
        )


def test_config_is_rechecked_after_timer_probes_and_before_disable(tmp_path):
    retirement = load_module()
    payload = valid_evidence()
    evidence = write_evidence(tmp_path / "evidence.json", payload)
    live = write_live_proof(tmp_path, payload)
    checked = []

    def check_timer(name):
        checked.append(name)
        if name == TIMER_UNITS[-1]:
            live["alert_config_path"].write_text(
                "HEALTHCHECKS_HEALTH_URL=configured\n"
                "HEALTHCHECKS_BACKUP_URL=configured\n"
            )
        return True

    with pytest.raises(retirement.RetirementRefused, match="changed|receipt"):
        retirement.guarded_retirement(
            evidence,
            execute=True,
            confirmation=CONFIRMATION,
            runner=lambda _command: pytest.fail("retirement command must not run"),
            timer_checker=check_timer,
            now=NOW,
            **live,
        )

    assert tuple(checked) == TIMER_UNITS


def test_final_destructive_gate_uses_current_utc_after_timer_probes(tmp_path):
    retirement = load_module()
    payload = valid_evidence()
    evidence = write_evidence(tmp_path / "evidence.json", payload)
    live = write_live_proof(tmp_path, payload)
    moments = iter((NOW, NOW + timedelta(hours=2)))

    with pytest.raises(retirement.RetirementRefused, match="recent|stale"):
        retirement.guarded_retirement(
            evidence,
            execute=True,
            confirmation=CONFIRMATION,
            runner=lambda _command: pytest.fail("retirement command must not run"),
            timer_checker=lambda _name: True,
            clock=lambda: next(moments),
            **live,
        )


@pytest.mark.parametrize("target", ["health", "backup", "receipt"])
def test_unhealthy_status_or_receipt_drift_refuses_at_the_last_gate(tmp_path, target):
    retirement = load_module()
    payload = valid_evidence()
    evidence = write_evidence(tmp_path / "evidence.json", payload)
    live = write_live_proof(tmp_path, payload)
    if target in {"health", "backup"}:
        status_path = live[f"{target}_status_path"]
        status = json.loads(status_path.read_text())
        status["category"] = "failure"
        status["checks"][0]["category"] = "failure"
        status_path.write_text(json.dumps(status))
    else:
        state_path = live["alert_state_directory"] / "health.json"
        state = json.loads(state_path.read_text())
        state["receipts"]["telegram"]["failure"]["receipt_id"] = "f" * 32
        state_path.write_text(json.dumps(state))

    with pytest.raises(retirement.RetirementRefused):
        retirement.guarded_retirement(
            evidence,
            execute=True,
            confirmation=CONFIRMATION,
            runner=lambda _command: pytest.fail("retirement command must not run"),
            timer_checker=lambda _name: True,
            now=NOW,
            **live,
        )


def test_symlinked_evidence_is_rejected(tmp_path):
    retirement = load_module()
    payload = valid_evidence()
    actual = write_evidence(tmp_path / "actual.json", payload)
    link = tmp_path / "evidence.json"
    link.symlink_to(actual)

    with pytest.raises(retirement.RetirementRefused):
        retirement.validate_evidence_file(
            link,
            required_receipts=expected_matrix(),
            now=NOW,
        )


def test_retirement_rejects_2099_future_history_and_signoffs():
    retirement = load_module()
    payload = valid_evidence(start=datetime(2099, 1, 1, tzinfo=timezone.utc))

    with pytest.raises(retirement.RetirementRefused, match="future|elapsed"):
        retirement.validate_evidence(
            payload,
            required_receipts=expected_matrix(),
            now=NOW,
        )


def test_retirement_rejects_future_signoff_or_receipt_beyond_clock_skew():
    retirement = load_module()
    for field in ("signed_at", "receipt"):
        payload = valid_evidence()
        if field == "signed_at":
            payload["local_alert_delivery"]["signed_at"] = "2099-01-01T00:00:00Z"
        else:
            payload["local_alert_delivery"]["receipts"][0][
                "delivered_at"
            ] = "2099-01-01T00:00:00Z"
        with pytest.raises(retirement.RetirementRefused, match="future|timestamp"):
            retirement.validate_evidence(
                payload,
                required_receipts=expected_matrix(),
                now=NOW,
            )


def test_retirement_requires_elapsed_recent_continuous_history():
    retirement = load_module()
    with pytest.raises(retirement.RetirementRefused, match="elapsed|30 hours"):
        retirement.validate_evidence(
            valid_evidence(),
            required_receipts=expected_matrix(),
            now=START + timedelta(hours=29, minutes=59),
        )
    with pytest.raises(retirement.RetirementRefused, match="recent"):
        retirement.validate_evidence(
            valid_evidence(),
            required_receipts=expected_matrix(),
            now=NOW + timedelta(hours=2),
        )
