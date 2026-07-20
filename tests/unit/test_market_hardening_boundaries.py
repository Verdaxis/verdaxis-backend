"""Focused regression tests for the Sprint 1 market boundaries."""

import asyncio
from io import BytesIO
import threading
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch
from uuid import uuid4

import pytest
from fastapi import HTTPException, Request, UploadFile
from starlette.datastructures import Headers

from app.models.rfq import RFQStatus
from app.models.user import UserRole
from app.routers import kyc as kyc_router
from app.routers import ai as ai_router
from app.routers import rfq as rfq_router
from app.schemas.ai import AIChatRequest
from app.services import ai_service, kyc as kyc_service
from app.services import news_feed
from app.services.activity import publish_trade_event


def _upload(data: bytes, content_type: str) -> UploadFile:
    return UploadFile(
        file=BytesIO(data),
        filename="document",
        headers=Headers({"content-type": content_type}),
    )


@pytest.mark.asyncio
async def test_kyc_upload_requires_matching_signature():
    with pytest.raises(HTTPException) as exc_info:
        await kyc_router._read_validated_document(_upload(b"%PDF-1.7", "image/png"))

    assert exc_info.value.status_code == 415


@pytest.mark.asyncio
async def test_kyc_provider_missing_configuration_fails_closed(monkeypatch):
    monkeypatch.setattr(kyc_service.settings, "GEMINI_API_KEY", None)

    with pytest.raises(kyc_service.KYCProviderUnavailable):
        await kyc_service.verify_document_with_gemini(b"%PDF-1.7", "application/pdf", "identity document")


@pytest.mark.asyncio
async def test_kyc_provider_capacity_rejects_without_waiting(monkeypatch):
    monkeypatch.setattr(kyc_service.settings, "GEMINI_API_KEY", "test-key")
    capacity = SimpleNamespace(acquire=lambda: False)
    monkeypatch.setattr(kyc_service, "_provider_capacity", capacity)
    provider_call = AsyncMock()
    monkeypatch.setattr(asyncio, "to_thread", provider_call)

    with pytest.raises(kyc_service.KYCProviderUnavailable):
        await kyc_service.verify_document_with_gemini(b"%PDF-1.7", "application/pdf", "identity document")

    provider_call.assert_not_awaited()


@pytest.mark.asyncio
async def test_kyc_submission_rejects_membership_change_during_provider_call(monkeypatch):
    original_org_id = uuid4()
    user_id = uuid4()
    current_user = SimpleNamespace(
        id=user_id,
        organization_id=original_org_id,
        status=kyc_router.UserStatus.APPROVED,
        must_change_password=False,
    )
    original_org = SimpleNamespace(
        id=original_org_id,
        name="Original Org",
        tax_id="ORG-123",
        verification_status="APPROVED",
    )
    moved_user = SimpleNamespace(
        id=user_id,
        organization_id=uuid4(),
        status=kyc_router.UserStatus.APPROVED,
        must_change_password=False,
    )
    original_org_result = MagicMock()
    original_org_result.scalar_one_or_none.return_value = original_org
    moved_user_result = MagicMock()
    moved_user_result.scalar_one_or_none.return_value = moved_user
    db = AsyncMock()
    db.execute.side_effect = [original_org_result, moved_user_result]
    monkeypatch.setattr(
        kyc_router,
        "verify_document_with_gemini",
        AsyncMock(return_value={"passed": True}),
    )

    request = Request(
        {
            "type": "http",
            "method": "POST",
            "path": "/api/kyc/submit",
            "query_string": b"",
            "headers": [],
            "client": ("127.0.0.1", 1234),
            "scheme": "https",
            "server": ("test", 443),
        }
    )
    with pytest.raises(HTTPException) as exc_info:
        await kyc_router.submit_kyc(
            request=request,
            current_user=current_user,
            db=db,
            passport=_upload(b"%PDF-passport", "application/pdf"),
            company_doc=_upload(b"%PDF-company", "application/pdf"),
            declared_company_name=None,
            registration_number=None,
        )

    assert exc_info.value.status_code == 409
    assert exc_info.value.detail["code"] == "KYC_MEMBERSHIP_CHANGED"


def test_ai_request_limits_message_and_history():
    with pytest.raises(ValueError):
        AIChatRequest(message="x" * 4001)
    with pytest.raises(ValueError):
        AIChatRequest(message="hello", history=[{"content": "x"}] * 21)
    with pytest.raises(ValueError):
        AIChatRequest(message="hello", history=[{"content": "x" * 2001}])


@pytest.mark.asyncio
async def test_ai_provider_missing_configuration_fails_closed(monkeypatch):
    monkeypatch.setattr(ai_service.settings, "GEMINI_API_KEY", None)

    with pytest.raises(ai_service.AIProviderUnavailable):
        await ai_service.chat_with_copilot("hello")


@pytest.mark.asyncio
async def test_ai_chat_maps_provider_unavailable_to_503(monkeypatch):
    monkeypatch.setattr(
        ai_router,
        "chat_with_copilot",
        AsyncMock(side_effect=ai_service.AIProviderUnavailable("unavailable")),
    )

    with pytest.raises(HTTPException) as exc_info:
        await ai_router.chat(
            request=Request(
                {
                    "type": "http",
                    "method": "POST",
                    "path": "/api/ai/chat",
                    "query_string": b"",
                    "headers": [],
                    "client": ("127.0.0.1", 1234),
                    "scheme": "https",
                }
            ),
            payload=AIChatRequest(message="hello"),
            current_user=SimpleNamespace(),
        )

    assert exc_info.value.status_code == 503


@pytest.mark.asyncio
async def test_ai_provider_call_runs_off_event_loop(monkeypatch):
    monkeypatch.setattr(ai_service.settings, "GEMINI_API_KEY", "test-key")
    loop_thread = threading.get_ident()
    provider_thread = None

    def provider(_message):
        nonlocal provider_thread
        provider_thread = threading.get_ident()
        return "ok"

    monkeypatch.setattr(ai_service, "_chat_sync", provider)
    assert await ai_service.chat_with_copilot("hello") == "ok"
    assert provider_thread is not None and provider_thread != loop_thread


@pytest.mark.asyncio
async def test_news_missing_provider_uses_local_categorizer(monkeypatch):
    monkeypatch.setattr(news_feed.settings, "GEMINI_API_KEY", None)
    provider = AsyncMock(side_effect=AssertionError("provider must not be called"))
    monkeypatch.setattr(news_feed, "_categorize_headline_sync", provider)

    first = await news_feed.categorize_headline("IMO announces new emissions regulation")
    second = await news_feed.categorize_headline("IMO announces new emissions regulation")

    assert first == second == {"category": "regulation", "relevance": 3, "summary": None}
    provider.assert_not_awaited()


@pytest.mark.asyncio
async def test_news_auth_failure_disables_provider_for_refresh_run(monkeypatch):
    monkeypatch.setattr(news_feed.settings, "GEMINI_API_KEY", "revoked-key")
    calls = 0

    def revoked_provider(_title):
        nonlocal calls
        calls += 1
        raise PermissionError("API key was reported as leaked")

    monkeypatch.setattr(news_feed, "_categorize_headline_sync", revoked_provider)
    state = news_feed._RefreshProviderState(provider_enabled=True)
    token = news_feed._refresh_provider_state.set(state)
    try:
        await news_feed.categorize_headline("First headline")
        await news_feed.categorize_headline("Second headline")
    finally:
        news_feed._refresh_provider_state.reset(token)

    assert calls == 1
    assert state.provider_enabled is False


def _user(role: UserRole, organization_id=None):
    return SimpleNamespace(
        role=role,
        organization_id=organization_id or uuid4(),
    )


def _rfq(*, buyer_org_id=None, status=RFQStatus.OPEN, expires_at=None):
    from datetime import UTC, datetime, timedelta

    return SimpleNamespace(
        buyer_org_id=buyer_org_id or uuid4(),
        status=status,
        expires_at=expires_at or datetime.now(UTC) + timedelta(hours=1),
    )


def test_rfq_visibility_hides_supplier_ineligible_details():
    supplier = _user(UserRole.SUPPLIER)
    assert rfq_router._rfq_visibility_filters(supplier)

    visible = _rfq()
    rfq_router._ensure_rfq_detail_visible(visible, supplier)

    with pytest.raises(HTTPException) as exc_info:
        rfq_router._ensure_rfq_detail_visible(
            _rfq(status=RFQStatus.ACCEPTED), supplier
        )
    assert exc_info.value.status_code == 404


@pytest.mark.asyncio
async def test_trade_lifecycle_events_are_published_only_to_participant_orgs():
    trade = SimpleNamespace(buyer_id=uuid4(), seller_id=uuid4())
    publish = AsyncMock()
    with patch("app.services.activity.event_bus.publish", publish):
        await publish_trade_event(trade, "trade_confirmed", {"trade_id": "t"})

    channels = {call.args[0] for call in publish.await_args_list}
    assert channels == {f"trades:{trade.buyer_id}", f"trades:{trade.seller_id}"}
