from datetime import UTC, datetime, timedelta

from app.services.onboarding_attention import (
    AttentionStage,
    OnboardingCandidate,
    classify_candidate,
    format_attention_message,
)


NOW = datetime(2026, 8, 4, 8, 0, tzinfo=UTC)


def candidate(**overrides) -> OnboardingCandidate:
    values = {
        "key": "user:1",
        "email": "person@example.com",
        "first_name": "Pat",
        "last_name": "Lee",
        "role": "BUYER",
        "created_at": NOW - timedelta(days=1),
        "email_verified": True,
        "verification_expires_at": None,
        "user_status": "APPROVED",
        "organization_name": "Example Energy",
        "organization_status": "APPROVED",
        "organization_provenance": "REAL",
        "membership_status": "APPROVED",
        "membership_reviewed_at": NOW - timedelta(hours=3),
        "user_approved_at": NOW - timedelta(hours=3),
        "organization_approved_at": NOW - timedelta(hours=3),
        "last_login": None,
        "pending_registration_expires_at": None,
    }
    values.update(overrides)
    return OnboardingCandidate(**values)


def test_classification_prioritizes_rejection():
    result = classify_candidate(candidate(user_status="REJECTED", email_verified=False), NOW)
    assert result is not None
    assert result.stage == AttentionStage.REJECTED


def test_pending_organization_setup_alerts_within_one_monitor_interval():
    result = classify_candidate(candidate(
        key="pending:1",
        user_status=None,
        organization_status=None,
        organization_provenance=None,
        membership_status=None,
        email_verified=False,
        pending_registration_expires_at=NOW + timedelta(minutes=5),
    ), NOW)
    assert result is not None
    assert result.stage == AttentionStage.ORGANIZATION_SETUP_EXPIRING


def test_unverified_user_alerts_after_verification_window():
    result = classify_candidate(candidate(
        email_verified=False,
        user_status="PENDING",
        verification_expires_at=NOW - timedelta(seconds=1),
    ), NOW)
    assert result is not None
    assert result.stage == AttentionStage.VERIFICATION_STALLED


def test_verified_user_with_incomplete_gates_requires_approval():
    result = classify_candidate(candidate(membership_status="PENDING"), NOW)
    assert result is not None
    assert result.stage == AttentionStage.APPROVAL_REQUIRED


def test_first_login_alert_starts_two_hours_after_final_gate():
    before = classify_candidate(candidate(
        membership_reviewed_at=NOW - timedelta(hours=1, minutes=59),
    ), NOW)
    at_boundary = classify_candidate(candidate(
        membership_reviewed_at=NOW - timedelta(hours=2),
    ), NOW)
    assert before is None
    assert at_boundary is not None
    assert at_boundary.stage == AttentionStage.FIRST_LOGIN_OVERDUE


def test_successful_login_is_complete():
    assert classify_candidate(candidate(last_login=NOW - timedelta(minutes=1)), NOW) is None


def test_admin_canary_and_non_real_organizations_are_excluded():
    assert classify_candidate(candidate(role="ADMIN"), NOW) is None
    assert classify_candidate(candidate(
        email="canary+prod@prod.canary.verdaxis.exchange",
    ), NOW) is None
    for provenance in ("DEMO", "TEST", "CANARY"):
        assert classify_candidate(candidate(organization_provenance=provenance), NOW) is None


def test_message_contains_operator_context_without_sensitive_fields():
    attention = classify_candidate(candidate(), NOW)
    assert attention is not None
    message = format_attention_message(attention, NOW)
    assert "Pat Lee · person@example.com" in message
    assert "Organization: Example Energy" in message
    assert "First login overdue" in message
    assert "token" not in message.lower()
    assert "hash" not in message.lower()
    assert "ip address" not in message.lower()
