from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from pydantic import ValidationError

from app.routers.auth_simple import SurveySubmission, submit_survey


def test_survey_rejects_body_email_cross_user_targeting():
    with pytest.raises(ValidationError):
        SurveySubmission(
            email="victim@example.com",
            use_case="buyer",
            referral_source="search",
        )


@pytest.mark.asyncio
async def test_survey_updates_only_authenticated_principal_and_is_idempotent():
    authenticated_user = SimpleNamespace(
        onboarding_use_case=None,
        onboarding_referral_source=None,
    )
    db = AsyncMock()
    request = SimpleNamespace()
    body = SurveySubmission(use_case="supplier", referral_source="colleague")

    await submit_survey.__wrapped__(request, body, authenticated_user, db)

    assert authenticated_user.onboarding_use_case == "supplier"
    assert authenticated_user.onboarding_referral_source == "colleague"
    db.commit.assert_awaited_once()

    await submit_survey.__wrapped__(
        request,
        SurveySubmission(use_case="buyer", referral_source="tampered retry"),
        authenticated_user,
        db,
    )

    assert authenticated_user.onboarding_use_case == "supplier"
    assert authenticated_user.onboarding_referral_source == "colleague"
    db.commit.assert_awaited_once()
