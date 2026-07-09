"""Unit tests for activity event publishers and price alert checking."""
import pytest
from unittest.mock import AsyncMock, MagicMock, patch, call
from uuid import uuid4
from decimal import Decimal
from datetime import datetime

from app.schemas.market_activity import MarketDemoStatus, MarketScope, MarketSourceKind
from app.services.activity import (
    order_activity_provenance,
    publish_new_listing,
    publish_price_crossing,
    publish_order_outbid,
    trade_activity_provenance,
    check_price_alerts,
)
from app.services.demo_market import DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def make_order(org_id=None):
    order = MagicMock()
    order.id = uuid4()
    order.organization_id = org_id or uuid4()
    order.side = MagicMock(value="ASK")
    order.price_per_mt_usd = Decimal("750.00")
    order.remaining_quantity_mt = Decimal("1000")
    order.created_at = datetime.utcnow()
    return order


def make_trade(*, buyer_id=None, seller_id=None):
    trade = MagicMock()
    trade.buyer_id = buyer_id or uuid4()
    trade.seller_id = seller_id or uuid4()
    trade.created_at = datetime.utcnow()
    trade.confirmed_at = None
    trade.delivered_at = None
    trade.paid_at = None
    trade.status = None
    trade.ask_order = None
    trade.bid_order = None
    return trade


def make_product(name="Green Methanol"):
    product = MagicMock()
    product.id = uuid4()
    product.name = name
    product.fuel_type = "methanol"
    return product


def make_dp(name="Singapore"):
    dp = MagicMock()
    dp.id = uuid4()
    dp.name = name
    dp.region = "APAC"
    return dp


# ---------------------------------------------------------------------------
# publish_new_listing
# ---------------------------------------------------------------------------

class TestPublishNewListing:
    @pytest.mark.asyncio
    async def test_publishes_to_activity_channel(self):
        order = make_order()
        product = make_product()
        dp = make_dp()

        with patch("app.services.activity.event_bus") as mock_bus:
            mock_bus.publish = AsyncMock()
            await publish_new_listing(order, product, dp)

        mock_bus.publish.assert_awaited_once()
        args = mock_bus.publish.call_args
        assert args[0][0] == "activity"
        assert args[0][1] == "new_listing"

    @pytest.mark.asyncio
    async def test_new_listing_data_contains_expected_keys(self):
        order = make_order()
        product = make_product("LNG Conventional")
        dp = make_dp("Rotterdam")

        with patch("app.services.activity.event_bus") as mock_bus:
            mock_bus.publish = AsyncMock()
            await publish_new_listing(order, product, dp)

        data = mock_bus.publish.call_args[0][2]
        assert "order_id" in data
        assert "product_name" in data
        assert "delivery_point" in data
        assert data["product_name"] == "LNG Conventional"
        assert data["delivery_point"] == "Rotterdam"
        assert data["source_kind"] == MarketSourceKind.LIVE_ORDER.value
        assert data["demo_status"] == MarketDemoStatus.REAL_ONLY.value
        assert data["scope"] == MarketScope.UNKNOWN.value

    @pytest.mark.asyncio
    async def test_new_listing_marks_demo_orders(self):
        order = make_order(DEMO_ACTIVITY_SELLER_ORG_ID)
        product = make_product("Bio Methanol")
        dp = make_dp("Singapore")

        with patch("app.services.activity.event_bus") as mock_bus:
            mock_bus.publish = AsyncMock()
            await publish_new_listing(order, product, dp)

        data = mock_bus.publish.call_args[0][2]
        assert data["source_kind"] == MarketSourceKind.DEMO_SEED.value
        assert data["demo_status"] == MarketDemoStatus.DEMO_ONLY.value

    @pytest.mark.asyncio
    async def test_new_listing_with_no_delivery_point(self):
        """publish_new_listing with dp=None should not raise."""
        order = make_order()
        product = make_product()

        with patch("app.services.activity.event_bus") as mock_bus:
            mock_bus.publish = AsyncMock()
            await publish_new_listing(order, product, None)

        mock_bus.publish.assert_awaited_once()


# ---------------------------------------------------------------------------
# publish_price_crossing
# ---------------------------------------------------------------------------

class TestPublishPriceCrossing:
    @pytest.mark.asyncio
    async def test_publishes_to_activity_channel(self):
        product = make_product()
        dp = make_dp()

        with patch("app.services.activity.event_bus") as mock_bus:
            mock_bus.publish = AsyncMock()
            await publish_price_crossing(product, dp)

        mock_bus.publish.assert_awaited_once()
        args = mock_bus.publish.call_args
        assert args[0][0] == "activity"
        assert args[0][1] == "price_crossing"

    @pytest.mark.asyncio
    async def test_price_crossing_data_keys(self):
        product = make_product("Ammonia Green")
        dp = make_dp("Fujairah")

        with patch("app.services.activity.event_bus") as mock_bus:
            mock_bus.publish = AsyncMock()
            await publish_price_crossing(product, dp)

        data = mock_bus.publish.call_args[0][2]
        assert "product_id" in data
        assert "product_name" in data
        assert data["product_name"] == "Ammonia Green"
        assert data["source_kind"] == MarketSourceKind.UNKNOWN.value
        assert data["demo_status"] == MarketDemoStatus.UNKNOWN.value
        assert data["observed_at"] is None

    @pytest.mark.asyncio
    async def test_price_crossing_with_no_delivery_point(self):
        product = make_product()
        with patch("app.services.activity.event_bus") as mock_bus:
            mock_bus.publish = AsyncMock()
            await publish_price_crossing(product, None)
        mock_bus.publish.assert_awaited_once()


class TestActivityProvenanceHelpers:
    def test_trade_activity_provenance_marks_demo_trade(self):
        trade = make_trade(
            buyer_id=DEMO_ACTIVITY_BUYER_ORG_ID,
            seller_id=DEMO_ACTIVITY_SELLER_ORG_ID,
        )

        payload = trade_activity_provenance(trade)

        assert payload["source_kind"] == MarketSourceKind.DEMO_SEED.value
        assert payload["demo_status"] == MarketDemoStatus.DEMO_ONLY.value

    def test_trade_activity_provenance_marks_one_sided_demo_unknown(self):
        trade = make_trade(buyer_id=DEMO_ACTIVITY_BUYER_ORG_ID)

        payload = trade_activity_provenance(trade)

        assert payload["source_kind"] == MarketSourceKind.UNKNOWN.value
        assert payload["demo_status"] == MarketDemoStatus.UNKNOWN.value

    def test_order_activity_provenance_preserves_existing_order_source(self):
        order = make_order()

        payload = order_activity_provenance(order)

        assert payload["source_kind"] == MarketSourceKind.LIVE_ORDER.value
        assert payload["demo_status"] == MarketDemoStatus.REAL_ONLY.value

    def test_trade_activity_provenance_uses_delivered_timestamp_for_delivered_trade(self):
        from app.models.orderbook import TradeStatus

        trade = make_trade()
        trade.status = TradeStatus.DELIVERED
        trade.confirmed_at = datetime(2026, 1, 1, 12, 0)
        trade.delivered_at = datetime(2026, 1, 2, 12, 0)

        payload = trade_activity_provenance(trade)

        assert payload["observed_at"] == "2026-01-02T12:00:00"

    def test_trade_activity_provenance_uses_paid_timestamp_for_paid_trade(self):
        from app.models.orderbook import TradeStatus

        trade = make_trade()
        trade.status = TradeStatus.PAID
        trade.confirmed_at = datetime(2026, 1, 1, 12, 0)
        trade.delivered_at = datetime(2026, 1, 2, 12, 0)
        trade.paid_at = datetime(2026, 1, 3, 12, 0)

        payload = trade_activity_provenance(trade)

        assert payload["observed_at"] == "2026-01-03T12:00:00"


# ---------------------------------------------------------------------------
# publish_order_outbid
# ---------------------------------------------------------------------------

class TestPublishOrderOutbid:
    @pytest.mark.asyncio
    async def test_publishes_to_org_specific_channel(self):
        org_id = uuid4()
        order = make_order(org_id)
        new_price = Decimal("720.00")

        with patch("app.services.activity.event_bus") as mock_bus:
            mock_bus.publish = AsyncMock()
            await publish_order_outbid(org_id, order, new_price)

        mock_bus.publish.assert_awaited_once()
        args = mock_bus.publish.call_args
        assert args[0][0] == f"activity:{org_id}"
        assert args[0][1] == "order_outbid"

    @pytest.mark.asyncio
    async def test_outbid_data_contains_new_price(self):
        org_id = uuid4()
        order = make_order(org_id)
        new_price = Decimal("699.50")

        with patch("app.services.activity.event_bus") as mock_bus:
            mock_bus.publish = AsyncMock()
            await publish_order_outbid(org_id, order, new_price)

        data = mock_bus.publish.call_args[0][2]
        assert "new_price" in data
        assert str(new_price) in str(data["new_price"])


# ---------------------------------------------------------------------------
# check_price_alerts
# ---------------------------------------------------------------------------

class TestCheckPriceAlerts:
    @pytest.mark.asyncio
    async def test_triggers_above_alert_when_price_exceeds_threshold(self):
        product_id = uuid4()
        dp_id = uuid4()
        price = Decimal("850.00")

        mock_db = AsyncMock()

        from app.models.alerts import PriceAlert
        alert = MagicMock(spec=PriceAlert)
        alert.id = uuid4()
        alert.org_id = uuid4()
        alert.product_id = product_id
        alert.delivery_point_id = dp_id
        alert.direction = "above"
        alert.threshold_usd = Decimal("800.00")
        alert.is_active = True
        alert.triggered_at = None

        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = [alert]
        mock_db.execute.return_value = mock_result

        with patch("app.services.activity.event_bus") as mock_bus:
            mock_bus.publish = AsyncMock()
            await check_price_alerts(mock_db, product_id, dp_id, price)

        # Alert should be marked triggered
        assert alert.triggered_at is not None
        assert alert.is_active is False
        mock_db.commit.assert_awaited_once()

    @pytest.mark.asyncio
    async def test_triggers_below_alert_when_price_drops_below_threshold(self):
        product_id = uuid4()
        price = Decimal("490.00")

        mock_db = AsyncMock()

        from app.models.alerts import PriceAlert
        alert = MagicMock(spec=PriceAlert)
        alert.id = uuid4()
        alert.org_id = uuid4()
        alert.product_id = product_id
        alert.delivery_point_id = None
        alert.direction = "below"
        alert.threshold_usd = Decimal("500.00")
        alert.is_active = True
        alert.triggered_at = None

        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = [alert]
        mock_db.execute.return_value = mock_result

        with patch("app.services.activity.event_bus") as mock_bus:
            mock_bus.publish = AsyncMock()
            await check_price_alerts(mock_db, product_id, None, price)

        assert alert.triggered_at is not None
        assert alert.is_active is False

    @pytest.mark.asyncio
    async def test_does_not_trigger_above_alert_when_price_below_threshold(self):
        product_id = uuid4()
        price = Decimal("750.00")

        mock_db = AsyncMock()

        from app.models.alerts import PriceAlert
        alert = MagicMock(spec=PriceAlert)
        alert.id = uuid4()
        alert.org_id = uuid4()
        alert.product_id = product_id
        alert.delivery_point_id = None
        alert.direction = "above"
        alert.threshold_usd = Decimal("800.00")
        alert.is_active = True
        alert.triggered_at = None

        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = [alert]
        mock_db.execute.return_value = mock_result

        with patch("app.services.activity.event_bus") as mock_bus:
            mock_bus.publish = AsyncMock()
            await check_price_alerts(mock_db, product_id, None, price)

        # Alert should NOT be triggered — price is below threshold
        assert alert.triggered_at is None
        assert alert.is_active is True

    @pytest.mark.asyncio
    async def test_no_alerts_is_noop(self):
        """When no alerts match, nothing is committed."""
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = []
        mock_db.execute.return_value = mock_result

        with patch("app.services.activity.event_bus") as mock_bus:
            mock_bus.publish = AsyncMock()
            await check_price_alerts(mock_db, uuid4(), None, Decimal("800.00"))

        mock_db.commit.assert_not_awaited()

    @pytest.mark.asyncio
    async def test_triggered_alert_publishes_to_org_channel(self):
        """Triggered alert publishes notification to org-specific activity channel."""
        product_id = uuid4()
        price = Decimal("900.00")

        mock_db = AsyncMock()

        from app.models.alerts import PriceAlert
        org_id = uuid4()
        alert = MagicMock(spec=PriceAlert)
        alert.id = uuid4()
        alert.org_id = org_id
        alert.product_id = product_id
        alert.delivery_point_id = None
        alert.direction = "above"
        alert.threshold_usd = Decimal("800.00")
        alert.is_active = True
        alert.triggered_at = None

        mock_result = MagicMock()
        mock_result.scalars.return_value.all.return_value = [alert]
        mock_db.execute.return_value = mock_result

        with patch("app.services.activity.event_bus") as mock_bus:
            mock_bus.publish = AsyncMock()
            await check_price_alerts(mock_db, product_id, None, price)

        mock_bus.publish.assert_awaited_once()
        channel = mock_bus.publish.call_args[0][0]
        assert channel == f"activity:{org_id}"
        assert mock_bus.publish.call_args[0][1] == "price_alert_triggered"
