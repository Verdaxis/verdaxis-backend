"""Unit tests for subscription model, tier gating, and endpoints."""
import pytest
from unittest.mock import AsyncMock, MagicMock, patch
from uuid import uuid4

from app.models.subscription import Subscription, SubscriptionTier


# ---------------------------------------------------------------------------
# Model / Enum tests
# ---------------------------------------------------------------------------

class TestSubscriptionTier:
    def test_tier_values_exist(self):
        assert SubscriptionTier.FREE.value == "free"
        assert SubscriptionTier.STANDARD.value == "standard"
        assert SubscriptionTier.ENTERPRISE.value == "enterprise"

    def test_tier_order(self):
        from app.middleware.subscription import TIER_ORDER
        assert TIER_ORDER[SubscriptionTier.FREE] == 0
        assert TIER_ORDER[SubscriptionTier.STANDARD] == 1
        assert TIER_ORDER[SubscriptionTier.ENTERPRISE] == 2

    def test_enterprise_beats_standard(self):
        from app.middleware.subscription import TIER_ORDER
        assert TIER_ORDER[SubscriptionTier.ENTERPRISE] > TIER_ORDER[SubscriptionTier.STANDARD]

    def test_standard_beats_free(self):
        from app.middleware.subscription import TIER_ORDER
        assert TIER_ORDER[SubscriptionTier.STANDARD] > TIER_ORDER[SubscriptionTier.FREE]


class TestSubscriptionModel:
    def test_subscription_defaults_to_free_tier(self):
        org_id = uuid4()
        sub = Subscription(org_id=org_id)
        assert sub.tier == SubscriptionTier.FREE

    def test_subscription_is_active_by_default(self):
        sub = Subscription(org_id=uuid4())
        assert sub.is_active is True

    def test_subscription_expires_at_nullable(self):
        sub = Subscription(org_id=uuid4())
        assert sub.expires_at is None

    def test_subscription_can_set_tier(self):
        sub = Subscription(org_id=uuid4(), tier=SubscriptionTier.ENTERPRISE)
        assert sub.tier == SubscriptionTier.ENTERPRISE


# ---------------------------------------------------------------------------
# Schema tests
# ---------------------------------------------------------------------------

class TestSubscriptionSchemas:
    def test_subscription_response_from_model(self):
        from app.schemas.subscription import SubscriptionResponse
        org_id = uuid4()
        sub = Subscription(org_id=org_id, tier=SubscriptionTier.STANDARD)
        sub.id = uuid4()
        # Pydantic v2 from_attributes
        resp = SubscriptionResponse.model_validate(sub)
        assert resp.org_id == org_id
        assert resp.tier == SubscriptionTier.STANDARD
        assert resp.is_active is True

    def test_subscription_update_schema(self):
        from app.schemas.subscription import SubscriptionUpdate
        update = SubscriptionUpdate(tier=SubscriptionTier.ENTERPRISE)
        assert update.tier == SubscriptionTier.ENTERPRISE

    def test_subscription_update_requires_tier(self):
        from app.schemas.subscription import SubscriptionUpdate
        import pydantic
        with pytest.raises(pydantic.ValidationError):
            SubscriptionUpdate()


# ---------------------------------------------------------------------------
# Middleware: get_or_create_subscription
# ---------------------------------------------------------------------------

class TestGetOrCreateSubscription:
    @pytest.mark.asyncio
    async def test_returns_existing_subscription(self):
        from app.middleware.subscription import get_or_create_subscription
        org_id = uuid4()
        mock_sub = Subscription(org_id=org_id, tier=SubscriptionTier.STANDARD)

        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = mock_sub
        mock_db = AsyncMock()
        mock_db.add = MagicMock()
        mock_db.execute.return_value = mock_result

        result = await get_or_create_subscription(mock_db, org_id)

        assert result is mock_sub
        mock_db.add.assert_not_called()

    @pytest.mark.asyncio
    async def test_creates_free_subscription_when_none_exists(self):
        from app.middleware.subscription import get_or_create_subscription
        org_id = uuid4()

        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = None
        mock_db = AsyncMock()
        mock_db.add = MagicMock()
        mock_db.execute.return_value = mock_result

        result = await get_or_create_subscription(mock_db, org_id)

        assert result.org_id == org_id
        assert result.tier == SubscriptionTier.FREE
        mock_db.add.assert_called_once()
        mock_db.commit.assert_awaited_once()
        mock_db.refresh.assert_awaited_once()


# ---------------------------------------------------------------------------
# Middleware: require_tier
# ---------------------------------------------------------------------------

class TestRequireTier:
    @pytest.mark.asyncio
    async def test_allows_sufficient_tier(self):
        """A STANDARD user can access a STANDARD-gated endpoint."""
        from app.middleware.subscription import require_tier

        org_id = uuid4()
        mock_user = MagicMock()
        mock_user.organization_id = org_id

        mock_sub = MagicMock()
        mock_sub.tier = SubscriptionTier.STANDARD

        mock_db = AsyncMock()
        mock_db.add = MagicMock()

        with patch("app.middleware.subscription.get_or_create_subscription", return_value=mock_sub):
            dep_fn = require_tier(SubscriptionTier.STANDARD)
            result = await dep_fn(current_user=mock_user, db=mock_db)
            assert result is mock_sub

    @pytest.mark.asyncio
    async def test_allows_higher_tier(self):
        """An ENTERPRISE user can access a STANDARD-gated endpoint."""
        from app.middleware.subscription import require_tier

        org_id = uuid4()
        mock_user = MagicMock()
        mock_user.organization_id = org_id

        mock_sub = MagicMock()
        mock_sub.tier = SubscriptionTier.ENTERPRISE

        mock_db = AsyncMock()
        mock_db.add = MagicMock()

        with patch("app.middleware.subscription.get_or_create_subscription", return_value=mock_sub):
            dep_fn = require_tier(SubscriptionTier.STANDARD)
            result = await dep_fn(current_user=mock_user, db=mock_db)
            assert result is mock_sub

    @pytest.mark.asyncio
    async def test_blocks_insufficient_tier(self):
        """A FREE user cannot access a STANDARD-gated endpoint."""
        from app.middleware.subscription import require_tier
        from fastapi import HTTPException

        org_id = uuid4()
        mock_user = MagicMock()
        mock_user.organization_id = org_id

        mock_sub = MagicMock()
        mock_sub.tier = SubscriptionTier.FREE

        mock_db = AsyncMock()
        mock_db.add = MagicMock()

        with patch("app.middleware.subscription.get_or_create_subscription", return_value=mock_sub):
            dep_fn = require_tier(SubscriptionTier.STANDARD)
            with pytest.raises(HTTPException) as exc_info:
                await dep_fn(current_user=mock_user, db=mock_db)
            assert exc_info.value.status_code == 403
            assert "standard" in exc_info.value.detail.lower()
            assert "free" in exc_info.value.detail.lower()

    @pytest.mark.asyncio
    async def test_blocks_standard_from_enterprise_endpoint(self):
        """A STANDARD user cannot access an ENTERPRISE-gated endpoint."""
        from app.middleware.subscription import require_tier
        from fastapi import HTTPException

        org_id = uuid4()
        mock_user = MagicMock()
        mock_user.organization_id = org_id

        mock_sub = MagicMock()
        mock_sub.tier = SubscriptionTier.STANDARD

        mock_db = AsyncMock()
        mock_db.add = MagicMock()

        with patch("app.middleware.subscription.get_or_create_subscription", return_value=mock_sub):
            dep_fn = require_tier(SubscriptionTier.ENTERPRISE)
            with pytest.raises(HTTPException) as exc_info:
                await dep_fn(current_user=mock_user, db=mock_db)
            assert exc_info.value.status_code == 403
            assert "enterprise" in exc_info.value.detail.lower()

    @pytest.mark.asyncio
    async def test_error_message_format(self):
        """Error message includes required tier and current tier."""
        from app.middleware.subscription import require_tier
        from fastapi import HTTPException

        org_id = uuid4()
        mock_user = MagicMock()
        mock_user.organization_id = org_id

        mock_sub = MagicMock()
        mock_sub.tier = SubscriptionTier.FREE

        mock_db = AsyncMock()
        mock_db.add = MagicMock()

        with patch("app.middleware.subscription.get_or_create_subscription", return_value=mock_sub):
            dep_fn = require_tier(SubscriptionTier.ENTERPRISE)
            with pytest.raises(HTTPException) as exc_info:
                await dep_fn(current_user=mock_user, db=mock_db)
            detail = exc_info.value.detail
            assert "enterprise" in detail.lower()
            assert "free" in detail.lower()
            assert "contact sales" in detail.lower()

    @pytest.mark.asyncio
    async def test_free_tier_endpoint_allows_all(self):
        """A FREE endpoint allows all tiers."""
        from app.middleware.subscription import require_tier

        org_id = uuid4()
        mock_user = MagicMock()
        mock_user.organization_id = org_id

        mock_sub = MagicMock()
        mock_sub.tier = SubscriptionTier.FREE

        mock_db = AsyncMock()
        mock_db.add = MagicMock()

        with patch("app.middleware.subscription.get_or_create_subscription", return_value=mock_sub):
            dep_fn = require_tier(SubscriptionTier.FREE)
            result = await dep_fn(current_user=mock_user, db=mock_db)
            assert result is mock_sub


# ---------------------------------------------------------------------------
# Router tests
# ---------------------------------------------------------------------------

class TestSubscriptionRouter:
    @pytest.mark.asyncio
    async def test_get_my_subscription_returns_subscription(self):
        from app.routers.subscriptions import get_my_subscription
        org_id = uuid4()
        mock_user = MagicMock()
        mock_user.organization_id = org_id

        mock_sub = Subscription(org_id=org_id, tier=SubscriptionTier.FREE)
        mock_db = AsyncMock()
        mock_db.add = MagicMock()

        with patch("app.routers.subscriptions.get_or_create_subscription", return_value=mock_sub):
            result = await get_my_subscription(current_user=mock_user, db=mock_db)
            assert result.org_id == org_id
            assert result.tier == SubscriptionTier.FREE

    @pytest.mark.asyncio
    async def test_admin_list_subscriptions(self):
        from app.routers.subscriptions import list_subscriptions
        mock_admin = MagicMock()
        mock_admin.role = "ADMIN"

        subs = [
            Subscription(org_id=uuid4(), tier=SubscriptionTier.FREE),
            Subscription(org_id=uuid4(), tier=SubscriptionTier.STANDARD),
        ]

        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = subs
        mock_db = AsyncMock()
        mock_db.add = MagicMock()
        mock_db.execute.return_value = mock_result

        result = await list_subscriptions(current_user=mock_admin, db=mock_db)
        assert len(result) == 2

    @pytest.mark.asyncio
    async def test_admin_get_subscription_by_org(self):
        from app.routers.subscriptions import get_subscription_by_org
        org_id = uuid4()
        mock_admin = MagicMock()
        mock_sub = Subscription(org_id=org_id, tier=SubscriptionTier.ENTERPRISE)

        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = mock_sub
        mock_db = AsyncMock()
        mock_db.add = MagicMock()
        mock_db.execute.return_value = mock_result

        result = await get_subscription_by_org(org_id=org_id, current_user=mock_admin, db=mock_db)
        assert result.tier == SubscriptionTier.ENTERPRISE

    @pytest.mark.asyncio
    async def test_admin_get_subscription_404_when_not_found(self):
        from app.routers.subscriptions import get_subscription_by_org
        from fastapi import HTTPException
        org_id = uuid4()
        mock_admin = MagicMock()

        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = None
        mock_db = AsyncMock()
        mock_db.add = MagicMock()
        mock_db.execute.return_value = mock_result

        with pytest.raises(HTTPException) as exc_info:
            await get_subscription_by_org(org_id=org_id, current_user=mock_admin, db=mock_db)
        assert exc_info.value.status_code == 404

    @pytest.mark.asyncio
    async def test_admin_update_subscription_tier(self):
        from app.routers.subscriptions import update_subscription
        from app.schemas.subscription import SubscriptionUpdate
        org_id = uuid4()
        mock_admin = MagicMock()

        existing_sub = Subscription(org_id=org_id, tier=SubscriptionTier.FREE)
        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = existing_sub
        mock_db = AsyncMock()
        mock_db.add = MagicMock()
        mock_db.execute.return_value = mock_result

        update = SubscriptionUpdate(tier=SubscriptionTier.ENTERPRISE)
        result = await update_subscription(org_id=org_id, body=update, current_user=mock_admin, db=mock_db)

        assert result.tier == SubscriptionTier.ENTERPRISE
        mock_db.commit.assert_awaited_once()
        mock_db.refresh.assert_awaited_once()

    @pytest.mark.asyncio
    async def test_admin_update_subscription_creates_when_not_found(self):
        """PUT on a non-existent org creates the subscription at the desired tier."""
        from app.routers.subscriptions import update_subscription
        from app.schemas.subscription import SubscriptionUpdate
        org_id = uuid4()
        mock_admin = MagicMock()

        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = None
        mock_db = AsyncMock()
        mock_db.add = MagicMock()
        mock_db.execute.return_value = mock_result

        update = SubscriptionUpdate(tier=SubscriptionTier.STANDARD)
        result = await update_subscription(org_id=org_id, body=update, current_user=mock_admin, db=mock_db)

        assert result.org_id == org_id
        assert result.tier == SubscriptionTier.STANDARD
        mock_db.add.assert_called_once()
        mock_db.commit.assert_awaited_once()
