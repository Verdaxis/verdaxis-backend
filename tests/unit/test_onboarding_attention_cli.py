from datetime import UTC, datetime, timedelta

import pytest

from app.cli.onboarding_attention import bootstrap_state, load_state, reconcile, write_state
from app.services.onboarding_attention import OnboardingCandidate


NOW = datetime(2026, 8, 4, 8, 0, tzinfo=UTC)


def candidate(*, last_login=None, membership_status="APPROVED"):
    return OnboardingCandidate(
        key="user:1",
        email="person@example.com",
        first_name="Pat",
        last_name="Lee",
        role="BUYER",
        created_at=NOW - timedelta(days=1),
        email_verified=True,
        verification_expires_at=None,
        user_status="APPROVED",
        organization_name="Example Energy",
        organization_status="APPROVED",
        organization_provenance="REAL",
        membership_status=membership_status,
        membership_reviewed_at=NOW - timedelta(hours=3),
        user_approved_at=NOW - timedelta(hours=3),
        organization_approved_at=NOW - timedelta(hours=3),
        last_login=last_login,
        pending_registration_expires_at=None,
    )


def test_reconcile_sends_once_for_unchanged_stage():
    messages = []
    state = reconcile([candidate()], {}, NOW, messages.append)
    assert len(messages) == 1
    state = reconcile([candidate()], state, NOW + timedelta(minutes=5), messages.append)
    assert len(messages) == 1
    assert state["user:1"]["stage"] == "first_login_overdue"


def test_reconcile_retries_when_delivery_fails():
    def fail(_message):
        raise OSError("telegram unavailable")

    with pytest.raises(OSError):
        reconcile([candidate()], {}, NOW, fail)
    delivered = []
    state = reconcile([candidate()], {}, NOW + timedelta(minutes=5), delivered.append)
    assert len(delivered) == 1
    assert state["user:1"]["stage"] == "first_login_overdue"


def test_reconcile_sends_recovery_after_successful_login():
    initial = reconcile([candidate()], {}, NOW, lambda _message: None)
    messages = []
    recovered = reconcile(
        [candidate(last_login=NOW + timedelta(minutes=1))],
        initial,
        NOW + timedelta(minutes=1),
        messages.append,
    )
    assert len(messages) == 1
    assert "onboarding recovered" in messages[0].lower()
    assert "user:1" not in recovered


def test_stage_change_sends_new_action_without_recovery_noise():
    initial = reconcile(
        [candidate(membership_status="PENDING")], {}, NOW, lambda _message: None
    )
    messages = []
    changed = reconcile(
        [candidate()], initial, NOW + timedelta(hours=3), messages.append
    )
    assert len(messages) == 1
    assert "First login overdue" in messages[0]
    assert changed["user:1"]["stage"] == "first_login_overdue"


def test_state_round_trip_contains_identifiers_and_stages_only(tmp_path):
    path = tmp_path / "state.json"
    state = {"user:1": {"stage": "approval_required", "updated_at": NOW.isoformat()}}
    write_state(path, state)
    assert load_state(path) == state
    assert "person@example.com" not in path.read_text()
    assert path.stat().st_mode & 0o777 == 0o600


def test_bootstrap_silences_existing_stage_and_recovery():
    state = bootstrap_state([candidate()], NOW)
    assert state["user:1"]["silent"] == "true"
    messages = []
    unchanged = reconcile([candidate()], state, NOW + timedelta(minutes=5), messages.append)
    assert unchanged == state
    recovered = reconcile(
        [candidate(last_login=NOW + timedelta(minutes=6))],
        state,
        NOW + timedelta(minutes=6),
        messages.append,
    )
    assert messages == []
    assert recovered == {}
