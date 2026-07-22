"""One fail-closed provenance policy for every aggregate market surface."""
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from types import SimpleNamespace
from uuid import uuid4

import pytest
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.market_catalog import DELIVERY_POINT_IDS, PRODUCT_IDS
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, Trade
from app.models.orderbook import OrderBookStatus, OrderSide
from app.models.user import Organization, OrganizationProvenance, OrgType
from app.routers.demand import get_demand_signals
from app.schemas.market_activity import MarketDemoStatus, MarketSourceKind
from app.services import market_provenance
from app.services.demo_market import DEMO_ACTIVITY_BUYER_ORG_ID
from app.services.provenance import snapshot_organization_provenance


def _policy_api():
    scope_type = getattr(market_provenance, "MarketEvidenceScope", None)
    policy_for = getattr(market_provenance, "evidence_policy_for_scope", None)
    order_clause = getattr(market_provenance, "order_evidence_clause", None)
    trade_clause = getattr(market_provenance, "trade_evidence_clause", None)
    assert scope_type is not None
    assert callable(policy_for)
    assert callable(order_clause)
    assert callable(trade_clause)
    return scope_type, policy_for, order_clause, trade_clause


def test_evidence_policy_has_only_separate_real_and_demo_scopes():
    scope_type, policy_for, _, _ = _policy_api()

    assert {scope.value for scope in scope_type} == {"REAL", "DEMO"}
    real = policy_for(scope_type.REAL, real_source=MarketSourceKind.CONFIRMED_TRADE)
    demo = policy_for(scope_type.DEMO, real_source=MarketSourceKind.CONFIRMED_TRADE)

    assert real.provenance == OrganizationProvenance.REAL
    assert real.source_kind == MarketSourceKind.CONFIRMED_TRADE
    assert real.demo_status == MarketDemoStatus.REAL_ONLY
    assert demo.provenance == OrganizationProvenance.DEMO
    assert demo.source_kind == MarketSourceKind.DEMO_SEED
    assert demo.demo_status == MarketDemoStatus.DEMO_ONLY


@pytest.mark.parametrize(
    ("counts", "scope", "source_kind", "demo_status", "value_prefix"),
    [
        ((2, 3, 4), "REAL", MarketSourceKind.LIVE_ORDER, MarketDemoStatus.REAL_ONLY, "real"),
        ((0, 3, 4), "DEMO", MarketSourceKind.DEMO_SEED, MarketDemoStatus.DEMO_ONLY, "demo"),
        ((0, 0, 4), None, MarketSourceKind.UNKNOWN, MarketDemoStatus.UNKNOWN, None),
        ((0, 0, 0), None, MarketSourceKind.NO_DATA, MarketDemoStatus.NOT_APPLICABLE, None),
    ],
)
def test_aggregate_selection_never_blends_evidence_classes(
    counts,
    scope,
    source_kind,
    demo_status,
    value_prefix,
):
    selected = market_provenance.select_aggregate_evidence(
        real_count=counts[0],
        demo_count=counts[1],
        unknown_count=counts[2],
        real_source=MarketSourceKind.LIVE_ORDER,
    )

    assert selected.scope is None if scope is None else selected.scope.value == scope
    assert selected.source_kind == source_kind
    assert selected.demo_status == demo_status
    assert selected.value_prefix == value_prefix
    assert selected.unknown_count == counts[2]


def test_stored_unknown_snapshot_is_never_upgraded_by_demo_id_registry():
    organization = SimpleNamespace(
        id=DEMO_ACTIVITY_BUYER_ORG_ID,
        provenance=OrganizationProvenance.UNKNOWN,
    )
    trade = SimpleNamespace(
        buyer_id=DEMO_ACTIVITY_BUYER_ORG_ID,
        seller_id=uuid4(),
        buyer_provenance=OrganizationProvenance.UNKNOWN,
        seller_provenance=OrganizationProvenance.UNKNOWN,
        delivery_point_id=None,
        product_id=None,
        market_product=None,
        availability_window=None,
    )

    assert snapshot_organization_provenance(organization) == OrganizationProvenance.UNKNOWN
    assert market_provenance.trade_market_provenance(trade)["source_kind"] == MarketSourceKind.UNKNOWN.value


def test_evidence_sql_clauses_select_one_class_before_aggregation():
    scope_type, _, order_clause, trade_clause = _policy_api()

    compile_options = {"compile_kwargs": {"literal_binds": True}}
    real_order_sql = str(
        order_clause(OrderBookOrder.provenance, scope_type.REAL).compile(**compile_options)
    )
    demo_order_sql = str(
        order_clause(OrderBookOrder.provenance, scope_type.DEMO).compile(**compile_options)
    )
    real_trade_sql = str(trade_clause(Trade, scope_type.REAL).compile(**compile_options))

    assert "orderbook_orders.provenance" in real_order_sql
    assert real_order_sql != demo_order_sql
    assert "trades.buyer_provenance" in real_trade_sql
    assert "trades.seller_provenance" in real_trade_sql
    assert "trades.buyer_provenance = 'REAL'" in real_trade_sql
    assert "trades.seller_provenance = 'REAL'" in real_trade_sql
    assert "trades.market_snapshot_version = 1" in real_trade_sql
    assert "trades.product_id" in real_trade_sql
    assert "trades.delivery_point_id" in real_trade_sql
    assert "trades.availability_window" in real_trade_sql
    assert "trades.confirmed_at IS NOT NULL" in real_trade_sql


def test_formal_trade_evidence_rejects_legacy_and_forged_snapshot_shapes():
    scope_type, _, _, trade_clause = _policy_api()
    sql = str(
        trade_clause(Trade, scope_type.REAL).compile(
            compile_kwargs={"literal_binds": True}
        )
    )

    assert "VLSFO" not in sql
    assert "Amsterdam" not in sql
    assert str(PRODUCT_IDS["BIO_METHANOL"]).replace("-", "") in sql
    assert str(DELIVERY_POINT_IDS["Singapore"]).replace("-", "") in sql
    assert "market_snapshot_version = 1" in sql


def test_unknown_test_and_canary_never_receive_public_evidence_labels():
    for provenance in (
        OrganizationProvenance.UNKNOWN,
        OrganizationProvenance.TEST,
        OrganizationProvenance.CANARY,
    ):
        order = SimpleNamespace(provenance=provenance, delivery_point_id=None)
        policy = market_provenance.order_market_provenance(order)
        assert policy["source_kind"] == MarketSourceKind.UNKNOWN.value
        assert policy["demo_status"] == MarketDemoStatus.UNKNOWN.value


def test_count_helpers_cannot_label_mixed_evidence_as_one_market():
    from app.schemas.market_activity import demo_status_from_counts, source_kind_from_counts

    assert demo_status_from_counts(real_count=1, demo_count=1) == MarketDemoStatus.UNKNOWN
    assert source_kind_from_counts(
        real_count=1,
        demo_count=1,
        real_source=MarketSourceKind.LIVE_ORDER,
    ) == MarketSourceKind.UNKNOWN


@pytest.mark.asyncio
async def test_demand_separates_real_demo_and_metadata_only_unknown():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    table_names = ["organizations", "users", "products", "delivery_points", "orderbook_orders"]
    tables = [Base.metadata.tables[name] for name in table_names]
    async with engine.begin() as connection:
        await connection.run_sync(Base.metadata.create_all, tables=tables)

    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    try:
        async with factory() as db:
            product = Product(
                id=PRODUCT_IDS["BIO_METHANOL"],
                name="Bio Methanol",
                fuel_type="Methanol",
                fuel_grade="Bio",
                unit="MT",
                is_active=True,
            )
            point = DeliveryPoint(
                id=DELIVERY_POINT_IDS["Singapore"],
                name="Singapore",
                region="Asia",
                is_active=True,
            )
            organizations = {
                provenance: Organization(
                    id=uuid4(),
                    name=f"{provenance.value} buyer",
                    type=OrgType.FUEL_BUYER,
                    provenance=provenance,
                    verification_status="APPROVED",
                )
                for provenance in OrganizationProvenance
            }
            db.add_all([product, point, *organizations.values()])
            await db.flush()
            quantities = {
                OrganizationProvenance.REAL: Decimal("100.00"),
                OrganizationProvenance.DEMO: Decimal("200.00"),
                OrganizationProvenance.UNKNOWN: Decimal("300.00"),
                OrganizationProvenance.TEST: Decimal("400.00"),
                OrganizationProvenance.CANARY: Decimal("500.00"),
            }
            for index, (provenance, quantity) in enumerate(quantities.items()):
                db.add(
                    OrderBookOrder(
                        organization_id=organizations[provenance].id,
                        provenance=provenance,
                        side=OrderSide.BID,
                        product_id=product.id,
                        delivery_point_id=point.id,
                        quantity_mt=quantity,
                        remaining_quantity_mt=quantity,
                        price_per_mt_usd=Decimal("700.00") + index,
                        availability_window="SPOT",
                        status=OrderBookStatus.OPEN,
                        created_at=datetime.now(UTC),
                        expires_at=(
                            datetime.now(UTC).replace(microsecond=0)
                            + timedelta(days=1)
                            if provenance == OrganizationProvenance.DEMO
                            else None
                        ),
                    )
                )
            await db.commit()

            signals = await get_demand_signals(fuel_type=None, region=None, db=db)

        assert len(signals) == 3
        by_status = {signal.demo_status: signal for signal in signals}
        assert by_status[MarketDemoStatus.REAL_ONLY].volume_mt == Decimal("100.00")
        assert by_status[MarketDemoStatus.REAL_ONLY].source_kind == MarketSourceKind.LIVE_ORDER
        assert by_status[MarketDemoStatus.REAL_ONLY].fuel_type == "BIO_METHANOL"
        assert by_status[MarketDemoStatus.REAL_ONLY].market_product_code == "BIO_METHANOL"
        assert by_status[MarketDemoStatus.DEMO_ONLY].volume_mt == Decimal("200.00")
        assert by_status[MarketDemoStatus.DEMO_ONLY].source_kind == MarketSourceKind.DEMO_SEED
        unknown = by_status[MarketDemoStatus.UNKNOWN]
        assert unknown.source_kind == MarketSourceKind.UNKNOWN
        assert unknown.volume_mt is None
        assert unknown.max_price_per_mt is None
        assert unknown.bid_count == 0
        assert unknown.unknown_count == 1
    finally:
        async with engine.begin() as connection:
            await connection.run_sync(Base.metadata.drop_all, tables=tables)
        await engine.dispose()


@pytest.mark.asyncio
async def test_demand_excludes_inactive_catalog_rows_and_never_emits_generic_taxonomy():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    table_names = ["organizations", "users", "products", "delivery_points", "orderbook_orders"]
    tables = [Base.metadata.tables[name] for name in table_names]
    async with engine.begin() as connection:
        await connection.run_sync(Base.metadata.create_all, tables=tables)

    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    try:
        async with factory() as db:
            organization = Organization(
                id=uuid4(),
                name="Real buyer",
                type=OrgType.FUEL_BUYER,
                provenance=OrganizationProvenance.REAL,
                verification_status="APPROVED",
            )
            active_product = Product(
                id=PRODUCT_IDS["E_METHANOL"],
                name="e-Methanol",
                fuel_type="Methanol",
                fuel_grade="E",
                unit="MT",
                is_active=True,
            )
            inactive_product = Product(
                id=uuid4(),
                name="Methanol Green",
                fuel_type="Methanol",
                fuel_grade="Green",
                unit="MT",
                is_active=False,
            )
            active_point = DeliveryPoint(
                id=DELIVERY_POINT_IDS["Singapore"],
                name="Singapore", region="Asia", is_active=True
            )
            inactive_point = DeliveryPoint(
                id=uuid4(), name="Fujairah", region="Middle East", is_active=False
            )
            db.add_all([organization, active_product, inactive_product, active_point, inactive_point])
            await db.flush()

            def order(product: Product, point: DeliveryPoint, quantity: str) -> OrderBookOrder:
                return OrderBookOrder(
                    organization_id=organization.id,
                    provenance=OrganizationProvenance.REAL,
                    side=OrderSide.BID,
                    product_id=product.id,
                    delivery_point_id=point.id,
                    quantity_mt=Decimal(quantity),
                    remaining_quantity_mt=Decimal(quantity),
                    price_per_mt_usd=Decimal("700.00"),
                    availability_window="SPOT",
                    status=OrderBookStatus.OPEN,
                    created_at=datetime.now(UTC),
                )

            db.add_all(
                [
                    order(active_product, active_point, "10.00"),
                    order(inactive_product, active_point, "262.00"),
                    order(active_product, inactive_point, "8.00"),
                ]
            )
            await db.commit()

            signals = await get_demand_signals(fuel_type=None, region=None, db=db)

        assert len(signals) == 1
        assert signals[0].volume_mt == Decimal("10.00")
        assert signals[0].fuel_type == "E_METHANOL"
        assert signals[0].market_product_code == "E_METHANOL"
        assert signals[0].delivery_point_name == "Singapore"
    finally:
        async with engine.begin() as connection:
            await connection.run_sync(Base.metadata.drop_all, tables=tables)
        await engine.dispose()
