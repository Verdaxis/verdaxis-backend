from datetime import UTC, datetime, timedelta
from pathlib import Path
from uuid import uuid4

import pytest
from pydantic import ValidationError

from app.config import settings
from app.models.market_support import (
    MarketSupportContext,
    MarketSupportContextScope,
    MarketSupportContextStatus,
)
from app.schemas.market_support import (
    MarketSupportContextCreate,
    MarketSupportFinalConfirmation,
)


def test_context_contract_is_opaque_short_lived_and_organization_scoped():
    context = MarketSupportContext(
        actor_user_id=uuid4(),
        organization_id=uuid4(),
        accountable_user_id=uuid4(),
        support_reference="case-123",
        scope=MarketSupportContextScope.ASSISTED_ORDER_ENTRY,
        expires_at=datetime.now(UTC) + timedelta(minutes=30),
        status=MarketSupportContextStatus.ACTIVE,
        version=1,
    )

    assert context.__table__.columns["id"].default is not None
    assert context.status == MarketSupportContextStatus.ACTIVE
    assert context.version == 1
    assert context.scope == MarketSupportContextScope.ASSISTED_ORDER_ENTRY
    assert settings.MARKET_SUPPORT_CONTEXT_TTL_MINUTES > 0
    assert "organization_id" in MarketSupportContext.__table__.columns
    assert "accountable_user_id" in MarketSupportContext.__table__.columns
    assert "market_support_context_id" in __import__(
        "app.models.market_support", fromlist=["MarketSupportAuthorization"]
    ).MarketSupportAuthorization.__table__.columns


def test_context_start_requires_explicit_replacement_confirmation_and_reference():
    base = {
        "organization_id": uuid4(),
        "support_reference": "case-123",
    }
    assert MarketSupportContextCreate(**base).confirm_replacement is False
    assert MarketSupportContextCreate(**base, confirm_replacement=True).confirm_replacement

    with pytest.raises(ValidationError):
        MarketSupportContextCreate(**{**base, "support_reference": " "})


def test_final_support_confirmation_does_not_require_evidence_text():
    confirmation = MarketSupportFinalConfirmation(
        external_instruction_reference="ticket-123",
        instruction_at=datetime.now(UTC),
        acknowledge_exact_terms=True,
        acknowledge_executable_standing_order=True,
    )

    assert confirmation.evidence_excerpt is None
    assert confirmation.instruction_at.tzinfo is not None


def test_context_migration_is_after_assisted_listings_and_links_authorizations():
    source = (
        Path(__file__).resolve().parents[2]
        / "alembic/versions/ms_20260723_organization_context.py"
    ).read_text()
    assert 'down_revision = "ms_20260723_assisted_listings"' in source
    assert "market_support_context_id" in source
    assert "market_support_contexts" in source


def test_assisted_order_policy_migration_supports_both_sides_and_gtc():
    source = (
        Path(__file__).resolve().parents[2]
        / "alembic/versions/ms_20260723_assisted_order_v2.py"
    ).read_text()
    assert 'down_revision = "ms_20260723_organization_context"' in source
    assert "ASSISTED_ORDER_ENTRY" in source
    assert 'server_default="ASSISTED_ORDER_ENTRY"' in source
    assert "order_side IN ('BID', 'ASK')" in source
    assert '"order_expires_at"' in source and "nullable=True" in source


def test_forensic_confirmation_facts_are_persisted_without_plaintext_evidence():
    from app.models.market_support import MarketSupportAuthorization

    columns = MarketSupportAuthorization.__table__.columns
    assert "instruction_at" in columns
    assert "acknowledge_exact_terms" in columns
    assert "acknowledge_executable_standing_order" in columns
    assert "evidence_excerpt" not in columns
    source = (
        Path(__file__).resolve().parents[2]
        / "alembic/versions/ms_20260723_organization_context.py"
    ).read_text()
    assert "instruction_at" in source
    assert "acknowledge_exact_terms" in source
    assert "acknowledge_executable_standing_order" in source
