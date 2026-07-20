"""
Unit tests for the price discovery router logic.
Tests the aggregation query builder without a live DB by mocking the session.
"""
import pytest
from unittest.mock import AsyncMock, MagicMock, patch
from datetime import date, datetime, timedelta, UTC
from decimal import Decimal
from uuid import UUID, uuid4

from httpx import ASGITransport, AsyncClient
from sqlalchemy.dialects import postgresql
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.main import app
from app.market_catalog import (
    DELIVERY_POINTS_BY_NAME,
    PRODUCTS_BY_NAME,
)
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import (
    Initiator,
    OrderBookOrder,
    OrderBookStatus,
    OrderSide,
    Trade,
    TradeStatus,
)
from app.models.user import Organization, OrgType, OrganizationProvenance
from app.routers.price_discovery import aggregate_trade_prices, compute_reference_prices
from app.schemas.market_activity import MarketDemoStatus, MarketSourceKind
from app.schemas.orderbook import ReferencePriceItem
from app.services.demo_market import DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID


_PRICE_DISCOVERY_TABLES = [
    "organizations",
    "products",
    "delivery_points",
    "orderbook_orders",
    "trades",
]


async def _seed_confirmed_trade(
    db: AsyncSession,
    *,
    product_name: str,
    fuel_type: str,
    fuel_grade: str,
    delivery_point_name: str,
    region: str,
    availability_window: str,
    price: str,
    quantity: str = "100.00",
    product: Product | None = None,
    delivery_point: DeliveryPoint | None = None,
    buyer_id=None,
    seller_id=None,
    trade_created_at: datetime | None = None,
) -> tuple[Product, DeliveryPoint, OrderBookOrder, Trade]:
    created_at = trade_created_at or datetime.now(UTC)
    buyer = Organization(id=buyer_id or uuid4(), name=f"Buyer-{uuid4().hex[:6]}", type=OrgType.SHIPPING_LINE, verification_status="APPROVED", provenance=(OrganizationProvenance.DEMO if (buyer_id or UUID(int=0)) in {DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID} else OrganizationProvenance.REAL))
    seller = Organization(id=seller_id or uuid4(), name=f"Seller-{uuid4().hex[:6]}", type=OrgType.FUEL_SUPPLIER, verification_status="APPROVED", provenance=(OrganizationProvenance.DEMO if (seller_id or UUID(int=0)) in {DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID} else OrganizationProvenance.REAL))
    should_add_product = product is None
    if should_add_product:
        product_spec = PRODUCTS_BY_NAME[product_name]
        product = Product(
            id=product_spec.id,
            name=product_name,
            fuel_type=fuel_type,
            fuel_grade=fuel_grade,
            unit="MT",
            min_lot_size=100,
            is_active=True,
        )
    should_add_delivery_point = delivery_point is None
    if should_add_delivery_point:
        point_spec = DELIVERY_POINTS_BY_NAME[delivery_point_name]
        delivery_point = DeliveryPoint(
            id=point_spec.id,
            name=delivery_point_name,
            region=region,
            timezone="UTC",
            is_active=True,
        )
    order = OrderBookOrder(
        organization_id=seller.id,
        provenance=seller.provenance,
        side=OrderSide.ASK,
        product_id=product.id,
        delivery_point_id=delivery_point.id,
        quantity_mt=Decimal(quantity),
        remaining_quantity_mt=Decimal("0.00"),
        price_per_mt_usd=Decimal(price),
        availability_window=availability_window,
        status=OrderBookStatus.FILLED,
        created_at=created_at,
        expires_at=(
            created_at + timedelta(days=1)
            if seller.provenance == OrganizationProvenance.DEMO
            else None
        ),
        certification_declared=True,
        certification_scheme="ISCC EU",
        certifications=["ISCC EU"],
    )
    objects = [buyer, seller, order]
    if should_add_product:
        objects.append(product)
    if should_add_delivery_point:
        objects.append(delivery_point)
    db.add_all(objects)
    await db.flush()

    trade = Trade(
        bid_order_id=None,
        ask_order_id=order.id,
        buyer_id=buyer.id,
        seller_id=seller.id,
        initiator_org_id=buyer.id,
        buyer_provenance=buyer.provenance,
        seller_provenance=seller.provenance,
        initiated_by=Initiator.BUYER,
        product_id=product.id,
        product_name=product.name,
        fuel_type=product.fuel_type,
        fuel_grade=product.fuel_grade,
        market_product=product.market_product,
        delivery_point_id=delivery_point.id,
        delivery_point_name=delivery_point.name,
        delivery_point_region=delivery_point.region,
        availability_window=order.availability_window,
        quantity_mt=Decimal(quantity),
        price_per_mt_usd=Decimal(price),
        status=TradeStatus.CONFIRMED,
        confirmed_at=created_at,
        created_at=created_at,
    )
    db.add(trade)
    await db.commit()
    return product, delivery_point, order, trade


class TestAggregateFunction:
    """Test the pure aggregation logic."""

    @pytest.mark.asyncio
    async def test_returns_empty_list_when_no_trades(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        summaries = await aggregate_trade_prices(mock_db)
        assert summaries == []

    @pytest.mark.asyncio
    async def test_filters_by_fuel_type(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        await aggregate_trade_prices(mock_db, fuel_type="Methanol")
        # Verify query execution occurred.
        assert mock_db.execute.call_count >= 1

    @pytest.mark.asyncio
    async def test_filters_by_region(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        await aggregate_trade_prices(mock_db, region="Singapore")
        assert mock_db.execute.call_count >= 1

    @pytest.mark.asyncio
    async def test_filters_by_product_id(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        pid = uuid4()
        await aggregate_trade_prices(mock_db, product_id=pid)
        assert mock_db.execute.call_count >= 1

    @pytest.mark.asyncio
    async def test_filters_by_delivery_point_id(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        dpid = uuid4()
        await aggregate_trade_prices(mock_db, delivery_point_id=dpid)
        assert mock_db.execute.call_count >= 1

    @pytest.mark.asyncio
    async def test_includes_seller_initiated_bid_hit_trade(self):
        """Trades created by a supplier hitting a BID only have bid_order_id."""
        engine = create_async_engine("sqlite+aiosqlite://", echo=False)
        tables = [Base.metadata.tables[name] for name in _PRICE_DISCOVERY_TABLES]

        async with engine.begin() as conn:
            await conn.run_sync(Base.metadata.create_all, tables=tables)

        session_factory = async_sessionmaker(
            bind=engine,
            class_=AsyncSession,
            expire_on_commit=False,
        )

        try:
            async with session_factory() as db:
                buyer = Organization(id=uuid4(), name="Buyer", type=OrgType.SHIPPING_LINE, provenance=OrganizationProvenance.REAL)
                seller = Organization(id=uuid4(), name="Seller", type=OrgType.FUEL_SUPPLIER, provenance=OrganizationProvenance.REAL)
                product = Product(
                    id=PRODUCTS_BY_NAME["Bio Methanol"].id,
                    name="Bio Methanol",
                    fuel_type="Methanol",
                    fuel_grade="Bio",
                    unit="MT",
                    min_lot_size=100,
                )
                delivery_point = DeliveryPoint(
                    id=DELIVERY_POINTS_BY_NAME["Singapore"].id,
                    name="Singapore",
                    region="Asia",
                    timezone="Asia/Singapore",
                )
                bid = OrderBookOrder(
                    organization_id=buyer.id,
                    side=OrderSide.BID,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    quantity_mt=Decimal("1000.00"),
                    remaining_quantity_mt=Decimal("500.00"),
                    price_per_mt_usd=Decimal("1100.00"),
                    created_at=datetime.now(UTC),
                )
                db.add_all([buyer, seller, product, delivery_point, bid])
                await db.flush()

                trade = Trade(
                    bid_order_id=bid.id,
                    ask_order_id=None,
                    buyer_id=buyer.id,
                    seller_id=seller.id,
                    initiator_org_id=seller.id,
                    buyer_provenance=OrganizationProvenance.REAL,
                    seller_provenance=OrganizationProvenance.REAL,
                    initiated_by=Initiator.SELLER,
                    product_id=product.id,
                    product_name=product.name,
                    fuel_type=product.fuel_type,
                    fuel_grade=product.fuel_grade,
                    market_product=product.market_product,
                    delivery_point_id=delivery_point.id,
                    delivery_point_name=delivery_point.name,
                    delivery_point_region=delivery_point.region,
                    availability_window=bid.availability_window,
                    quantity_mt=Decimal("250.00"),
                    price_per_mt_usd=Decimal("1100.00"),
                    status=TradeStatus.CONFIRMED,
                    confirmed_at=datetime.now(UTC),
                    created_at=datetime.now(UTC),
                )
                db.add(trade)
                await db.commit()

                summaries = await aggregate_trade_prices(db, hours=24)

            assert len(summaries) == 1
            assert summaries[0].product_id == product.id
            assert summaries[0].delivery_point_id == delivery_point.id
            assert summaries[0].volume_24h == Decimal("250.00")
            assert summaries[0].trade_count_24h == 1
            assert summaries[0].source_kind == MarketSourceKind.CONFIRMED_TRADE
            assert summaries[0].demo_status == MarketDemoStatus.REAL_ONLY
            assert summaries[0].real_trade_count_24h == 1
            assert summaries[0].demo_trade_count_24h == 0
            assert summaries[0].unknown_trade_count_24h == 0
        finally:
            async with engine.begin() as conn:
                await conn.run_sync(Base.metadata.drop_all, tables=tables)
            await engine.dispose()

    @pytest.mark.asyncio
    async def test_demo_trade_summary_is_marked_demo_seed(self):
        engine = create_async_engine("sqlite+aiosqlite://", echo=False)
        tables = [Base.metadata.tables[name] for name in _PRICE_DISCOVERY_TABLES]

        async with engine.begin() as conn:
            await conn.run_sync(Base.metadata.create_all, tables=tables)

        session_factory = async_sessionmaker(bind=engine, class_=AsyncSession, expire_on_commit=False)

        try:
            async with session_factory() as db:
                await _seed_confirmed_trade(
                    db,
                    product_name="Bio Methanol",
                    fuel_type="Methanol",
                    fuel_grade="Bio",
                    delivery_point_name="Singapore",
                    region="Asia",
                    availability_window="SPOT",
                    price="1100.00",
                    buyer_id=DEMO_ACTIVITY_BUYER_ORG_ID,
                    seller_id=DEMO_ACTIVITY_SELLER_ORG_ID,
                )

                summaries = await aggregate_trade_prices(db, hours=24)

            assert len(summaries) == 1
            assert summaries[0].source_kind == MarketSourceKind.DEMO_SEED
            assert summaries[0].demo_status == MarketDemoStatus.DEMO_ONLY
            assert summaries[0].real_trade_count_24h == 0
            assert summaries[0].demo_trade_count_24h == 1
            assert summaries[0].unknown_trade_count_24h == 0
        finally:
            async with engine.begin() as conn:
                await conn.run_sync(Base.metadata.drop_all, tables=tables)
            await engine.dispose()

    @pytest.mark.asyncio
    async def test_real_and_demo_trade_summaries_are_separate(self):
        engine = create_async_engine("sqlite+aiosqlite://", echo=False)
        tables = [Base.metadata.tables[name] for name in _PRICE_DISCOVERY_TABLES]

        async with engine.begin() as conn:
            await conn.run_sync(Base.metadata.create_all, tables=tables)

        session_factory = async_sessionmaker(bind=engine, class_=AsyncSession, expire_on_commit=False)

        try:
            async with session_factory() as db:
                product, delivery_point, _, _ = await _seed_confirmed_trade(
                    db,
                    product_name="Bio Methanol",
                    fuel_type="Methanol",
                    fuel_grade="Bio",
                    delivery_point_name="Singapore",
                    region="Asia",
                    availability_window="SPOT",
                    price="1100.00",
                )
                await _seed_confirmed_trade(
                    db,
                    product_name="Bio Methanol",
                    fuel_type="Methanol",
                    fuel_grade="Bio",
                    delivery_point_name="Singapore",
                    region="Asia",
                    availability_window="SPOT",
                    price="1115.00",
                    product=product,
                    delivery_point=delivery_point,
                    buyer_id=DEMO_ACTIVITY_BUYER_ORG_ID,
                    seller_id=DEMO_ACTIVITY_SELLER_ORG_ID,
                )

                summaries = await aggregate_trade_prices(db, hours=24)

            assert len(summaries) == 2
            by_status = {summary.demo_status: summary for summary in summaries}
            real = by_status[MarketDemoStatus.REAL_ONLY]
            demo = by_status[MarketDemoStatus.DEMO_ONLY]
            assert real.source_kind == MarketSourceKind.CONFIRMED_TRADE
            assert real.last_price == Decimal("1100.00")
            assert real.volume_24h == Decimal("100.00")
            assert real.real_trade_count_24h == 1
            assert real.demo_trade_count_24h == 0
            assert demo.source_kind == MarketSourceKind.DEMO_SEED
            assert demo.last_price == Decimal("1115.00")
            assert demo.volume_24h == Decimal("100.00")
            assert demo.real_trade_count_24h == 0
            assert demo.demo_trade_count_24h == 1
        finally:
            async with engine.begin() as conn:
                await conn.run_sync(Base.metadata.drop_all, tables=tables)
            await engine.dispose()

    @pytest.mark.asyncio
    async def test_one_sided_demo_trade_is_quarantined_without_economic_values(self):
        engine = create_async_engine("sqlite+aiosqlite://", echo=False)
        tables = [Base.metadata.tables[name] for name in _PRICE_DISCOVERY_TABLES]

        async with engine.begin() as conn:
            await conn.run_sync(Base.metadata.create_all, tables=tables)

        session_factory = async_sessionmaker(bind=engine, class_=AsyncSession, expire_on_commit=False)

        try:
            async with session_factory() as db:
                await _seed_confirmed_trade(
                    db,
                    product_name="Bio Methanol",
                    fuel_type="Methanol",
                    fuel_grade="Bio",
                    delivery_point_name="Singapore",
                    region="Asia",
                    availability_window="SPOT",
                    price="1100.00",
                    buyer_id=DEMO_ACTIVITY_BUYER_ORG_ID,
                )

                summaries = await aggregate_trade_prices(db, hours=24)

            assert len(summaries) == 1
            summary = summaries[0]
            assert summary.source_kind == MarketSourceKind.UNKNOWN
            assert summary.demo_status == MarketDemoStatus.UNKNOWN
            assert summary.last_price is None
            assert summary.avg_price_24h is None
            assert summary.high_24h is None
            assert summary.low_24h is None
            assert summary.volume_24h == Decimal("0")
            assert summary.trade_count_24h == 0
            assert summary.unknown_trade_count_24h == 1
        finally:
            async with engine.begin() as conn:
                await conn.run_sync(Base.metadata.drop_all, tables=tables)
            await engine.dispose()

    @pytest.mark.asyncio
    async def test_filters_by_market_product_and_availability_window(self):
        """Price summaries must stay scoped to a canonical market slice."""
        engine = create_async_engine("sqlite+aiosqlite://", echo=False)
        tables = [Base.metadata.tables[name] for name in _PRICE_DISCOVERY_TABLES]

        async with engine.begin() as conn:
            await conn.run_sync(Base.metadata.create_all, tables=tables)

        session_factory = async_sessionmaker(bind=engine, class_=AsyncSession, expire_on_commit=False)

        try:
            async with session_factory() as db:
                _, delivery_point, _, _ = await _seed_confirmed_trade(
                    db,
                    product_name="Bio Methanol",
                    fuel_type="Methanol",
                    fuel_grade="Bio",
                    delivery_point_name="Singapore",
                    region="Asia",
                    availability_window="SPOT",
                    price="1100.00",
                )
                await _seed_confirmed_trade(
                    db,
                    product_name="e-Methanol",
                    fuel_type="Methanol",
                    fuel_grade="E",
                    delivery_point_name="Singapore",
                    region="Asia",
                    availability_window="2026-06",
                    price="1250.00",
                    delivery_point=delivery_point,
                )

                summaries = await aggregate_trade_prices(
                    db,
                    market_product="BIO_METHANOL",
                    availability_window="SPOT",
                    hours=24,
                )

            assert len(summaries) == 1
            assert summaries[0].product_name == "Bio Methanol"
            assert summaries[0].market_product == "BIO_METHANOL"
            assert summaries[0].availability_window == "SPOT"
            assert summaries[0].last_price == Decimal("1100.00")
        finally:
            async with engine.begin() as conn:
                await conn.run_sync(Base.metadata.drop_all, tables=tables)
            await engine.dispose()


class TestComputeReferencePrices:
    """Test the VWAP reference price computation."""

    @pytest.mark.asyncio
    async def test_returns_empty_list_when_no_trades(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        prices = await compute_reference_prices(mock_db)
        assert prices == []

    @pytest.mark.asyncio
    async def test_filters_by_fuel_type(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        await compute_reference_prices(mock_db, fuel_type="Methanol")
        assert mock_db.execute.call_count == 1

    @pytest.mark.asyncio
    async def test_filters_by_region(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        await compute_reference_prices(mock_db, region="ARA")
        assert mock_db.execute.call_count == 1

    @pytest.mark.asyncio
    async def test_filters_by_date_range(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        await compute_reference_prices(
            mock_db,
            date_from=date(2026, 1, 1),
            date_to=date(2026, 3, 1),
        )
        assert mock_db.execute.call_count == 1

    @pytest.mark.asyncio
    async def test_rejects_invalid_date_range_before_query(self):
        mock_db = AsyncMock()

        with pytest.raises(ValueError, match="date_from must be before or equal to date_to"):
            await compute_reference_prices(
                mock_db,
                date_from=date(2026, 3, 1),
                date_to=date(2026, 1, 1),
            )

        mock_db.execute.assert_not_called()

    @pytest.mark.asyncio
    async def test_date_range_excludes_out_of_range_trades(self):
        engine = create_async_engine("sqlite+aiosqlite://", echo=False)
        tables = [Base.metadata.tables[name] for name in _PRICE_DISCOVERY_TABLES]

        async with engine.begin() as conn:
            await conn.run_sync(Base.metadata.create_all, tables=tables)

        session_factory = async_sessionmaker(bind=engine, class_=AsyncSession, expire_on_commit=False)

        try:
            async with session_factory() as db:
                product, delivery_point, _, _ = await _seed_confirmed_trade(
                    db,
                    product_name="Bio Methanol",
                    fuel_type="Methanol",
                    fuel_grade="Bio",
                    delivery_point_name="Singapore",
                    region="Asia",
                    availability_window="SPOT",
                    price="500.00",
                    trade_created_at=datetime(2026, 2, 10, 9, 0, tzinfo=UTC),
                )
                await _seed_confirmed_trade(
                    db,
                    product_name="Bio Methanol",
                    fuel_type="Methanol",
                    fuel_grade="Bio",
                    delivery_point_name="Singapore",
                    region="Asia",
                    availability_window="SPOT",
                    price="550.00",
                    product=product,
                    delivery_point=delivery_point,
                    trade_created_at=datetime(2026, 2, 15, 9, 0, tzinfo=UTC),
                )
                await _seed_confirmed_trade(
                    db,
                    product_name="Bio Methanol",
                    fuel_type="Methanol",
                    fuel_grade="Bio",
                    delivery_point_name="Singapore",
                    region="Asia",
                    availability_window="SPOT",
                    price="650.00",
                    product=product,
                    delivery_point=delivery_point,
                    trade_created_at=datetime(2026, 3, 3, 9, 0, tzinfo=UTC),
                )

                prices = await compute_reference_prices(
                    db,
                    date_from=date(2026, 2, 15),
                    date_to=date(2026, 2, 15),
                )

            assert len(prices) == 1
            assert prices[0].date == date(2026, 2, 15)
            assert prices[0].vwap_usd == Decimal("550.00")
        finally:
            async with engine.begin() as conn:
                await conn.run_sync(Base.metadata.drop_all, tables=tables)
            await engine.dispose()

    @pytest.mark.asyncio
    async def test_date_filters_bind_python_date_values_for_postgres(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        await compute_reference_prices(
            mock_db,
            date_from=date(2026, 1, 1),
            date_to=date(2026, 3, 1),
        )

        stmt = mock_db.execute.call_args.args[0]
        compiled = stmt.compile(dialect=postgresql.dialect())
        bind_values = list(compiled.params.values())

        assert date(2026, 1, 1) in bind_values
        assert date(2026, 3, 1) in bind_values
        assert "2026-01-01" not in bind_values
        assert "2026-03-01" not in bind_values

    @pytest.mark.asyncio
    async def test_filters_by_product_id(self):
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        pid = uuid4()
        await compute_reference_prices(mock_db, product_id=pid)
        assert mock_db.execute.call_count == 1

    @pytest.mark.asyncio
    async def test_computes_vwap_correctly(self):
        """VWAP = sum(price * qty) / sum(qty)."""
        mock_db = AsyncMock()
        mock_result = MagicMock()
        # Simulate: 2 trades for Methanol/Singapore on 2026-02-15
        # Trade 1: 100 MT @ $500 = $50,000
        # Trade 2: 200 MT @ $550 = $110,000
        # VWAP = $160,000 / 300 = $533.33
        pid = PRODUCTS_BY_NAME["Bio Methanol"].id
        dpid = DELIVERY_POINTS_BY_NAME["Singapore"].id
        mock_row = MagicMock()
        mock_row.product_id = pid
        mock_row.product_name = "Bio Methanol"
        mock_row.fuel_type = "Methanol"
        mock_row.market_product = "BIO_METHANOL"
        mock_row.delivery_point_id = dpid
        mock_row.delivery_point_name = "Singapore"
        mock_row.region = "Asia"
        mock_row.availability_window = "SPOT"
        mock_row.trade_date = date(2026, 2, 15)
        mock_row.weighted_sum = Decimal("160000.00")
        mock_row.total_volume = Decimal("300.00")
        mock_row.trade_count = 2
        mock_result.all.return_value = [mock_row]
        mock_db.execute.return_value = mock_result

        prices = await compute_reference_prices(mock_db)

        assert len(prices) == 1
        assert prices[0].product_id == pid
        assert prices[0].product_name == "Bio Methanol"
        assert prices[0].fuel_type == "Methanol"
        assert prices[0].delivery_point_id == dpid
        assert prices[0].delivery_point_name == "Singapore"
        assert prices[0].region == "Asia"
        assert prices[0].vwap_usd == Decimal("533.33")
        assert prices[0].total_volume_mt == Decimal("300.00")
        assert prices[0].trade_count == 2
        assert prices[0].date == date(2026, 2, 15)

    @pytest.mark.asyncio
    async def test_multiple_markets_returned(self):
        """Multiple product/delivery_point/date groups are returned."""
        mock_db = AsyncMock()
        mock_result = MagicMock()

        pid1 = PRODUCTS_BY_NAME["Bio Methanol"].id
        pid2 = PRODUCTS_BY_NAME["e-Methanol"].id
        dpid1 = DELIVERY_POINTS_BY_NAME["Singapore"].id
        dpid2 = DELIVERY_POINTS_BY_NAME["Rotterdam"].id

        row1 = MagicMock()
        row1.product_id = pid1
        row1.product_name = "Bio Methanol"
        row1.fuel_type = "Methanol"
        row1.market_product = "BIO_METHANOL"
        row1.delivery_point_id = dpid1
        row1.delivery_point_name = "Singapore"
        row1.region = "Asia"
        row1.availability_window = "SPOT"
        row1.trade_date = date(2026, 2, 15)
        row1.weighted_sum = Decimal("100000.00")
        row1.total_volume = Decimal("200.00")
        row1.trade_count = 3

        row2 = MagicMock()
        row2.product_id = pid2
        row2.product_name = "e-Methanol"
        row2.fuel_type = "Methanol"
        row2.market_product = "E_METHANOL"
        row2.delivery_point_id = dpid2
        row2.delivery_point_name = "Rotterdam"
        row2.region = "Europe"
        row2.availability_window = "SPOT"
        row2.trade_date = date(2026, 2, 15)
        row2.weighted_sum = Decimal("75000.00")
        row2.total_volume = Decimal("100.00")
        row2.trade_count = 1

        mock_result.all.return_value = [row1, row2]
        mock_db.execute.return_value = mock_result

        prices = await compute_reference_prices(mock_db)

        assert len(prices) == 2
        assert prices[0].fuel_type == "Methanol"
        assert prices[0].vwap_usd == Decimal("500.00")
        assert prices[1].market_product == "E_METHANOL"
        assert prices[1].vwap_usd == Decimal("750.00")

    @pytest.mark.asyncio
    async def test_zero_volume_returns_zero_vwap(self):
        """Edge case: if total_volume is 0 (should not normally happen), VWAP is 0."""
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_row = MagicMock()
        mock_row.product_id = PRODUCTS_BY_NAME["Bio Methanol"].id
        mock_row.product_name = "Bio Methanol"
        mock_row.fuel_type = "Methanol"
        mock_row.market_product = "BIO_METHANOL"
        mock_row.delivery_point_id = DELIVERY_POINTS_BY_NAME["Singapore"].id
        mock_row.delivery_point_name = "Singapore"
        mock_row.region = "Asia"
        mock_row.availability_window = "SPOT"
        mock_row.trade_date = date(2026, 3, 1)
        mock_row.weighted_sum = Decimal("0")
        mock_row.total_volume = Decimal("0")
        mock_row.trade_count = 0
        mock_result.all.return_value = [mock_row]
        mock_db.execute.return_value = mock_result

        prices = await compute_reference_prices(mock_db)

        assert len(prices) == 1
        assert prices[0].vwap_usd == Decimal("0")
        assert prices[0].total_volume_mt == Decimal("0")

    @pytest.mark.asyncio
    async def test_combined_filters(self):
        """All filters can be used together."""
        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.all.return_value = []
        mock_db.execute.return_value = mock_result

        await compute_reference_prices(
            mock_db,
            date_from=date(2026, 1, 1),
            date_to=date(2026, 2, 28),
            fuel_type="Methanol",
            region="ARA",
            product_id=uuid4(),
            delivery_point_id=uuid4(),
        )
        assert mock_db.execute.call_count == 1

    @pytest.mark.asyncio
    async def test_reference_filters_by_market_product_and_availability_window(self):
        """Reference prices must preserve the same market slice filters as summaries."""
        engine = create_async_engine("sqlite+aiosqlite://", echo=False)
        tables = [Base.metadata.tables[name] for name in _PRICE_DISCOVERY_TABLES]

        async with engine.begin() as conn:
            await conn.run_sync(Base.metadata.create_all, tables=tables)

        session_factory = async_sessionmaker(bind=engine, class_=AsyncSession, expire_on_commit=False)

        try:
            async with session_factory() as db:
                _, delivery_point, _, _ = await _seed_confirmed_trade(
                    db,
                    product_name="Bio Ethanol",
                    fuel_type="Ethanol",
                    fuel_grade="Bio",
                    delivery_point_name="Santos",
                    region="Americas",
                    availability_window="2026-06",
                    price="540.00",
                )
                await _seed_confirmed_trade(
                    db,
                    product_name="Synthetic Ethanol",
                    fuel_type="Ethanol",
                    fuel_grade="Synthetic",
                    delivery_point_name="Santos",
                    region="Americas",
                    availability_window="SPOT",
                    price="650.00",
                    delivery_point=delivery_point,
                )

                prices = await compute_reference_prices(
                    db,
                    market_product="BIO_ETHANOL",
                    availability_window="2026-06",
                )

            assert len(prices) == 1
            assert prices[0].product_name == "Bio Ethanol"
            assert prices[0].market_product == "BIO_ETHANOL"
            assert prices[0].availability_window == "2026-06"
            assert prices[0].vwap_usd == Decimal("540.00")
        finally:
            async with engine.begin() as conn:
                await conn.run_sync(Base.metadata.drop_all, tables=tables)
            await engine.dispose()

    @pytest.mark.asyncio
    async def test_reference_prices_use_immutable_snapshots_and_real_scope_only(self):
        engine = create_async_engine("sqlite+aiosqlite://", echo=False)
        tables = [Base.metadata.tables[name] for name in _PRICE_DISCOVERY_TABLES]
        async with engine.begin() as conn:
            await conn.run_sync(Base.metadata.create_all, tables=tables)
        session_factory = async_sessionmaker(bind=engine, class_=AsyncSession, expire_on_commit=False)

        try:
            async with session_factory() as db:
                product, point, order, real_trade = await _seed_confirmed_trade(
                    db,
                    product_name="Bio Methanol",
                    fuel_type="Methanol",
                    fuel_grade="Bio",
                    delivery_point_name="Singapore",
                    region="Asia",
                    availability_window="SPOT",
                    price="700.00",
                )
                await _seed_confirmed_trade(
                    db,
                    product_name="Bio Methanol",
                    fuel_type="Methanol",
                    fuel_grade="Bio",
                    delivery_point_name="Singapore",
                    region="Asia",
                    availability_window="SPOT",
                    price="999.00",
                    product=product,
                    delivery_point=point,
                    buyer_id=DEMO_ACTIVITY_BUYER_ORG_ID,
                    seller_id=DEMO_ACTIVITY_SELLER_ORG_ID,
                )

                # Mutable catalog/order/current-org changes cannot rewrite history.
                product.name = "Mutated Product"
                product.fuel_type = "Ammonia"
                product.fuel_grade = "Synthetic"
                point.name = "Mutated Port"
                point.region = "Mutated Region"
                order.availability_window = "2026-Q4"
                buyer = await db.get(Organization, real_trade.buyer_id)
                seller = await db.get(Organization, real_trade.seller_id)
                buyer.provenance = OrganizationProvenance.DEMO
                seller.provenance = OrganizationProvenance.DEMO
                await db.commit()

                prices = await compute_reference_prices(
                    db,
                    market_product="BIO_METHANOL",
                    region="Asia",
                    availability_window="SPOT",
                )

            assert len(prices) == 1
            assert prices[0].product_name == "Bio Methanol"
            assert prices[0].delivery_point_name == "Singapore"
            assert prices[0].region == "Asia"
            assert prices[0].vwap_usd == Decimal("700.00")
            assert prices[0].demo_status == MarketDemoStatus.REAL_ONLY
        finally:
            async with engine.begin() as conn:
                await conn.run_sync(Base.metadata.drop_all, tables=tables)
            await engine.dispose()


class TestReferencePriceRoute:
    """Test public route contract for reference price date filters."""

    @pytest.mark.asyncio
    async def test_preferred_date_params_are_forwarded_to_compute(self):
        with patch(
            "app.routers.price_discovery.compute_reference_prices",
            new_callable=AsyncMock,
            return_value=[],
        ) as mock_compute:
            async with AsyncClient(
                transport=ASGITransport(app=app), base_url="http://test"
            ) as client:
                resp = await client.get(
                    "/api/prices/reference",
                    params={"date_from": "2026-01-01", "date_to": "2026-03-01"},
                )

        assert resp.status_code == 200
        call_kwargs = mock_compute.call_args.kwargs
        assert call_kwargs["date_from"] == date(2026, 1, 1)
        assert call_kwargs["date_to"] == date(2026, 3, 1)

    @pytest.mark.asyncio
    async def test_deprecated_date_aliases_are_forwarded_to_compute(self):
        with patch(
            "app.routers.price_discovery.compute_reference_prices",
            new_callable=AsyncMock,
            return_value=[],
        ) as mock_compute:
            async with AsyncClient(
                transport=ASGITransport(app=app), base_url="http://test"
            ) as client:
                resp = await client.get(
                    "/api/prices/reference",
                    params={"from": "2026-01-01", "to": "2026-03-01"},
                )

        assert resp.status_code == 200
        call_kwargs = mock_compute.call_args.kwargs
        assert call_kwargs["date_from"] == date(2026, 1, 1)
        assert call_kwargs["date_to"] == date(2026, 3, 1)

    @pytest.mark.asyncio
    async def test_matching_preferred_and_deprecated_aliases_are_accepted(self):
        with patch(
            "app.routers.price_discovery.compute_reference_prices",
            new_callable=AsyncMock,
            return_value=[],
        ) as mock_compute:
            async with AsyncClient(
                transport=ASGITransport(app=app), base_url="http://test"
            ) as client:
                resp = await client.get(
                    "/api/prices/reference",
                    params={
                        "date_from": "2026-01-01",
                        "from": "2026-01-01",
                        "date_to": "2026-03-01",
                        "to": "2026-03-01",
                    },
                )

        assert resp.status_code == 200
        call_kwargs = mock_compute.call_args.kwargs
        assert call_kwargs["date_from"] == date(2026, 1, 1)
        assert call_kwargs["date_to"] == date(2026, 3, 1)

    @pytest.mark.asyncio
    async def test_conflicting_preferred_and_deprecated_aliases_return_422(self):
        with patch(
            "app.routers.price_discovery.compute_reference_prices",
            new_callable=AsyncMock,
            return_value=[],
        ) as mock_compute:
            async with AsyncClient(
                transport=ASGITransport(app=app), base_url="http://test"
            ) as client:
                resp = await client.get(
                    "/api/prices/reference",
                    params={"date_from": "2026-01-01", "from": "2026-01-02"},
                )

        assert resp.status_code == 422
        assert "date_from and from must match" in resp.text
        mock_compute.assert_not_called()

    @pytest.mark.asyncio
    async def test_invalid_preferred_date_range_returns_422(self):
        with patch(
            "app.routers.price_discovery.compute_reference_prices",
            new_callable=AsyncMock,
            return_value=[],
        ) as mock_compute:
            async with AsyncClient(
                transport=ASGITransport(app=app), base_url="http://test"
            ) as client:
                resp = await client.get(
                    "/api/prices/reference",
                    params={"date_from": "2026-03-01", "date_to": "2026-01-01"},
                )

        assert resp.status_code == 422
        assert "date_from must be before or equal to date_to" in resp.text
        mock_compute.assert_not_called()

    def test_reference_price_openapi_date_contract(self):
        schema = app.openapi()
        parameters = {
            param["name"]: param
            for param in schema["paths"]["/api/prices/reference"]["get"]["parameters"]
        }

        assert "date_from" in parameters
        assert "date_to" in parameters
        assert parameters["from"]["deprecated"] is True
        assert parameters["to"]["deprecated"] is True

    def test_reference_export_openapi_uses_export_date_names(self):
        schema = app.openapi()
        operation = schema["paths"]["/api/prices/reference/export"]["get"]
        parameters = {param["name"]: param for param in operation["parameters"]}

        assert "from_date" in parameters
        assert "to_date" in parameters
        assert "same market filters" in operation["description"]
        assert "from_date/to_date" in operation["description"]


class TestReferencePriceItemSchema:
    """Test the Pydantic schema for reference price items."""

    def test_valid_schema(self):
        pid = uuid4()
        dpid = uuid4()
        item = ReferencePriceItem(
            product_id=pid,
            product_name="Methanol Green",
            fuel_type="Methanol",
            delivery_point_id=dpid,
            delivery_point_name="Singapore",
            region="Asia",
            vwap_usd=Decimal("533.33"),
            total_volume_mt=Decimal("300.00"),
            trade_count=2,
            date=date(2026, 2, 15),
        )
        assert item.product_id == pid
        assert item.fuel_type == "Methanol"
        assert item.vwap_usd == Decimal("533.33")
        assert item.date == date(2026, 2, 15)

    def test_schema_serialization(self):
        item = ReferencePriceItem(
            product_id=uuid4(),
            product_name="Ammonia Green",
            fuel_type="Ammonia",
            delivery_point_id=uuid4(),
            delivery_point_name="ARA",
            region="Europe",
            vwap_usd=Decimal("750.00"),
            total_volume_mt=Decimal("100.00"),
            trade_count=1,
            date=date(2026, 3, 1),
        )
        data = item.model_dump()
        assert data["fuel_type"] == "Ammonia"
        assert data["trade_count"] == 1
        assert data["product_name"] == "Ammonia Green"


def test_reference_price_item_has_visibility_field():
    from app.schemas.orderbook import ReferencePriceItem
    from decimal import Decimal
    from datetime import date
    item = ReferencePriceItem(
        fuel_type="Methanol", region="Europe",
        vwap_usd=Decimal("525.50"), total_volume_mt=Decimal("5000"),
        trade_count=3, date=date(2026, 3, 12), visibility="internal",
    )
    assert item.visibility == "internal"


def test_reference_price_item_defaults_to_external():
    from app.schemas.orderbook import ReferencePriceItem
    from decimal import Decimal
    from datetime import date
    item = ReferencePriceItem(
        fuel_type="Methanol", region="Europe",
        vwap_usd=Decimal("525.50"), total_volume_mt=Decimal("5000"),
        trade_count=3, date=date(2026, 3, 12),
    )
    assert item.visibility == "external"


def test_get_reference_prices_accepts_visibility_param():
    import inspect
    from app.routers.price_discovery import get_reference_prices
    assert "visibility" in inspect.signature(get_reference_prices).parameters
