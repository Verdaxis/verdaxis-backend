"""Unit tests for price alert CRUD endpoints and free-tier limits."""
import pytest
from unittest.mock import AsyncMock, MagicMock, patch
from uuid import uuid4
from decimal import Decimal
from datetime import datetime

from fastapi import HTTPException

from app.routers.alerts import create_alert, list_alerts, delete_alert
from app.schemas.alerts import AlertCreate
from app.models.alerts import PriceAlert
from app.models.user import User, UserRole, UserStatus


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def make_user(org_id=None):
    user = MagicMock(spec=User)
    user.id = uuid4()
    user.organization_id = org_id or uuid4()
    user.role = UserRole.BUYER
    user.status = UserStatus.APPROVED
    return user


def make_alert(org_id, product_id=None, direction="above", threshold=Decimal("750.00")):
    alert = MagicMock(spec=PriceAlert)
    alert.id = uuid4()
    alert.org_id = org_id
    alert.product_id = product_id or uuid4()
    alert.delivery_point_id = None
    alert.direction = direction
    alert.threshold_usd = threshold
    alert.is_active = True
    alert.triggered_at = None
    alert.created_at = datetime.utcnow()
    return alert


# ---------------------------------------------------------------------------
# POST /alerts — create
# ---------------------------------------------------------------------------

class TestCreateAlert:
    @pytest.mark.asyncio
    async def test_create_alert_success_returns_201_body(self):
        """Happy path: alert created when under free-tier limit."""
        user = make_user()
        product_id = uuid4()

        mock_db = AsyncMock()
        # count query returns 0 (no existing alerts)
        count_result = MagicMock()
        count_result.scalar.return_value = 0
        mock_db.execute.return_value = count_result
        mock_db.get.return_value = None

        alert_data = AlertCreate(
            product_id=product_id,
            direction="above",
            threshold_usd=Decimal("800.00"),
        )

        with patch("app.routers.alerts.PriceAlert") as MockAlert:
            instance = MagicMock()
            instance.id = uuid4()
            instance.org_id = user.organization_id
            instance.product_id = product_id
            instance.delivery_point_id = None
            instance.direction = "above"
            instance.threshold_usd = Decimal("800.00")
            instance.is_active = True
            instance.triggered_at = None
            instance.created_at = datetime.utcnow()
            instance.product = None
            MockAlert.return_value = instance

            result = await create_alert(alert_data=alert_data, current_user=user, db=mock_db)

        mock_db.add.assert_called_once()
        mock_db.commit.assert_awaited_once()

    @pytest.mark.asyncio
    async def test_create_alert_requires_org(self):
        """User without org should get 400."""
        user = make_user()
        user.organization_id = None
        mock_db = AsyncMock()

        alert_data = AlertCreate(
            product_id=uuid4(),
            direction="below",
            threshold_usd=Decimal("500.00"),
        )

        with pytest.raises(HTTPException) as exc_info:
            await create_alert(alert_data=alert_data, current_user=user, db=mock_db)

        assert exc_info.value.status_code == 400

    @pytest.mark.asyncio
    async def test_create_alert_free_tier_limit_enforced(self):
        """Org with 5 active alerts should get 403 on create."""
        user = make_user()

        mock_db = AsyncMock()
        count_result = MagicMock()
        count_result.scalar.return_value = 5  # at limit
        mock_db.execute.return_value = count_result

        alert_data = AlertCreate(
            product_id=uuid4(),
            direction="above",
            threshold_usd=Decimal("900.00"),
        )

        with pytest.raises(HTTPException) as exc_info:
            await create_alert(alert_data=alert_data, current_user=user, db=mock_db)

        assert exc_info.value.status_code == 403
        assert "limit" in exc_info.value.detail.lower()

    @pytest.mark.asyncio
    async def test_create_alert_at_4_allows_create(self):
        """Org with 4 active alerts (just under limit) should succeed."""
        user = make_user()
        product_id = uuid4()

        mock_db = AsyncMock()
        count_result = MagicMock()
        count_result.scalar.return_value = 4  # one slot remaining
        mock_db.execute.return_value = count_result
        mock_db.get.return_value = None

        alert_data = AlertCreate(
            product_id=product_id,
            direction="below",
            threshold_usd=Decimal("600.00"),
        )

        with patch("app.routers.alerts.PriceAlert") as MockAlert:
            instance = MagicMock()
            instance.id = uuid4()
            instance.org_id = user.organization_id
            instance.product_id = product_id
            instance.delivery_point_id = None
            instance.direction = "below"
            instance.threshold_usd = Decimal("600.00")
            instance.is_active = True
            instance.triggered_at = None
            instance.created_at = datetime.utcnow()
            instance.product = None
            MockAlert.return_value = instance

            result = await create_alert(alert_data=alert_data, current_user=user, db=mock_db)

        mock_db.add.assert_called_once()

    @pytest.mark.asyncio
    async def test_create_alert_direction_below(self):
        """Direction 'below' is accepted."""
        user = make_user()
        mock_db = AsyncMock()

        count_result = MagicMock()
        count_result.scalar.return_value = 0
        mock_db.execute.return_value = count_result
        mock_db.get.return_value = None

        alert_data = AlertCreate(
            product_id=uuid4(),
            direction="below",
            threshold_usd=Decimal("400.00"),
        )

        with patch("app.routers.alerts.PriceAlert") as MockAlert:
            instance = MagicMock()
            instance.id = uuid4()
            instance.org_id = user.organization_id
            instance.product_id = alert_data.product_id
            instance.delivery_point_id = None
            instance.direction = "below"
            instance.threshold_usd = Decimal("400.00")
            instance.is_active = True
            instance.triggered_at = None
            instance.created_at = datetime.utcnow()
            instance.product = None
            MockAlert.return_value = instance

            result = await create_alert(alert_data=alert_data, current_user=user, db=mock_db)


# ---------------------------------------------------------------------------
# GET /alerts — list
# ---------------------------------------------------------------------------

class TestListAlerts:
    @pytest.mark.asyncio
    async def test_list_alerts_returns_org_alerts(self):
        """Lists active alerts for the user's org."""
        user = make_user()
        mock_db = AsyncMock()

        alerts = [make_alert(user.organization_id) for _ in range(3)]
        mock_result = MagicMock()
        mock_result.all.return_value = [(alert, None) for alert in alerts]
        mock_db.execute.return_value = mock_result

        result = await list_alerts(current_user=user, db=mock_db)

        assert len(result) == 3

    @pytest.mark.asyncio
    async def test_list_alerts_requires_org(self):
        """User without org gets 400."""
        user = make_user()
        user.organization_id = None
        mock_db = AsyncMock()

        with pytest.raises(HTTPException) as exc_info:
            await list_alerts(current_user=user, db=mock_db)

        assert exc_info.value.status_code == 400

    @pytest.mark.asyncio
    async def test_list_alerts_empty_returns_empty_list(self):
        """No alerts returns empty list, not 404."""
        user = make_user()
        mock_db = AsyncMock()

        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        result = await list_alerts(current_user=user, db=mock_db)
        assert result == []


# ---------------------------------------------------------------------------
# DELETE /alerts/{alert_id}
# ---------------------------------------------------------------------------

class TestDeleteAlert:
    @pytest.mark.asyncio
    async def test_delete_own_alert_succeeds(self):
        """Owner can delete their own alert."""
        user = make_user()
        alert = make_alert(user.organization_id)
        alert_id = alert.id

        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.scalars.return_value.first.return_value = alert
        mock_db.execute.return_value = mock_result

        await delete_alert(alert_id=alert_id, current_user=user, db=mock_db)

        mock_db.delete.assert_called_once_with(alert)
        mock_db.commit.assert_awaited_once()

    @pytest.mark.asyncio
    async def test_delete_nonexistent_alert_returns_404(self):
        """Deleting a non-existent alert returns 404."""
        user = make_user()
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.scalars.return_value.first.return_value = None
        mock_db.execute.return_value = mock_result

        with pytest.raises(HTTPException) as exc_info:
            await delete_alert(alert_id=uuid4(), current_user=user, db=mock_db)

        assert exc_info.value.status_code == 404

    @pytest.mark.asyncio
    async def test_delete_other_orgs_alert_returns_403(self):
        """Cannot delete another org's alert."""
        user = make_user()
        other_org_id = uuid4()
        alert = make_alert(other_org_id)

        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.scalars.return_value.first.return_value = alert
        mock_db.execute.return_value = mock_result

        with pytest.raises(HTTPException) as exc_info:
            await delete_alert(alert_id=alert.id, current_user=user, db=mock_db)

        assert exc_info.value.status_code == 403

    @pytest.mark.asyncio
    async def test_delete_requires_org(self):
        """User without org gets 400."""
        user = make_user()
        user.organization_id = None
        mock_db = AsyncMock()

        with pytest.raises(HTTPException) as exc_info:
            await delete_alert(alert_id=uuid4(), current_user=user, db=mock_db)

        assert exc_info.value.status_code == 400
