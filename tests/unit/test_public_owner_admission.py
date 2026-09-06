"""Rejected owners' inert orders must vanish from every public surface.

Stage 6c security fix (phantom liquidity): KYC-/admin-reject paths leave the
target's OPEN orders in place by owner decision, relying on fail-closed
fill-time rechecks. These tests pin the query-side mirror: such orders never
appear in public order collections, never anchor best-bid/ask (is_crossed),
and never weigh in live VWAP benchmarks.
"""
import pytest
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import uuid4

from sqlalchemy import delete, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_NAME
from app.models.catalog import DeliveryPoint, Product
from app.models.live_slice_benchmark import LiveSliceBenchmark
from app.models.orderbook import (
    OrderBookOrder,
    OrderBookStatus,
    OrderCreationMethod,
    OrderSide,
)
from app.models.user import (
    Organization,
    OrganizationProvenance,
    OrgType,
    User,
    UserRole,
    UserStatus,
)
from app.routers.orderbook import list_aggregated_orderbook, list_asks, list_bids
from app.services.live_benchmarks import (
    get_live_slice_benchmark_price,
    rebuild_live_slice_benchmark,
)


REQUIRED_TABLES = [
    'organizations',
    'users',
    'products',
    'delivery_points',
    'orderbook_orders',
    'live_slice_benchmarks',
]


@pytest.fixture(scope='module')
def async_engine():
    return create_async_engine('sqlite+aiosqlite://', echo=False, future=True)


@pytest.fixture(scope='module')
async def setup_tables(async_engine):
    tables = [Base.metadata.tables[name] for name in REQUIRED_TABLES]
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    yield
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.drop_all, tables=tables)


@pytest.fixture
async def db(async_engine, setup_tables):
    session_factory = async_sessionmaker(
        bind=async_engine,
        class_=AsyncSession,
        expire_on_commit=False,
        autoflush=False,
    )
    async with session_factory() as session:
        for table in (
            'live_slice_benchmarks',
            'orderbook_orders',
            'users',
            'products',
            'delivery_points',
            'organizations',
        ):
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()
        yield session
        await session.rollback()
        for table in (
            'live_slice_benchmarks',
            'orderbook_orders',
            'users',
            'products',
            'delivery_points',
            'organizations',
        ):
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()


async def _make_org(db: AsyncSession, name: str, *, verification_status: str = 'APPROVED') -> Organization:
    org = Organization(
        name=name,
        type=OrgType.FUEL_SUPPLIER,
        provenance=OrganizationProvenance.REAL,
        verification_status=verification_status,
    )
    db.add(org)
    await db.flush()
    return org


async def _make_user(
    db: AsyncSession,
    org: Organization,
    *,
    role: UserRole = UserRole.SUPPLIER,
    status: UserStatus = UserStatus.APPROVED,
    kyc_status: str = 'APPROVED',
    email_verified: bool = True,
    organization_id=None,
) -> User:
    user = User(
        email=f'{uuid4().hex}@example.com',
        password_hash='x',
        role=role,
        status=status,
        organization_id=organization_id if organization_id is not None else org.id,
        email_verified=email_verified,
        kyc_status=kyc_status,
        kyc_organization_id=org.id,
    )
    db.add(user)
    await db.flush()
    return user


async def _make_catalog(db: AsyncSession) -> tuple[Product, DeliveryPoint]:
    spec = PRODUCTS_BY_NAME['Bio Methanol']
    product = Product(
        id=spec.id,
        name=spec.name,
        fuel_type=spec.fuel_type,
        fuel_grade=spec.fuel_grade,
        is_active=True,
    )
    dp_spec = DELIVERY_POINTS_BY_NAME['Singapore']
    delivery_point = DeliveryPoint(
        id=dp_spec.id,
        name=dp_spec.name,
        region=dp_spec.region,
        is_active=True,
    )
    db.add_all([product, delivery_point])
    await db.flush()
    return product, delivery_point


def _make_order(
    *,
    org_id,
    owner_user_id,
    product_id,
    delivery_point_id,
    side: OrderSide = OrderSide.ASK,
    price: str = '1000',
    quantity: str = '1000',
    creation_method: OrderCreationMethod = OrderCreationMethod.LEGACY_UNKNOWN,
    created_by_actor_user_id=None,
) -> OrderBookOrder:
    is_ask = side == OrderSide.ASK
    return OrderBookOrder(
        organization_id=org_id,
        owner_user_id=owner_user_id,
        provenance=OrganizationProvenance.REAL,
        side=side,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        quantity_mt=Decimal(quantity),
        remaining_quantity_mt=Decimal(quantity),
        price_per_mt_usd=Decimal(price),
        availability_window='SPOT',
        status=OrderBookStatus.OPEN,
        created_at=datetime.now(UTC),
        creation_method=creation_method,
        created_by_actor_user_id=created_by_actor_user_id,
        certification_scheme='ISCC EU' if is_ask else None,
        certification_declared=is_ask,
        specification_standard='ISO 8217' if is_ask else None,
        msds_available=is_ask,
        carbon_intensity_gco2_mj=Decimal('18.50') if is_ask else None,
        feedstock='Waste biomass' if is_ask else None,
        origin='Singapore' if is_ask else None,
    )


async def _list_asks(db: AsyncSession):
    return await list_asks(
        product_id=None,
        delivery_point_id=None,
        fuel_type=None,
        market_product=None,
        region=None,
        availability_window=None,
        include_off_spec=False,
        skip=0,
        limit=20,
        db=db,
    )


async def _list_bids(db: AsyncSession):
    return await list_bids(
        product_id=None,
        delivery_point_id=None,
        fuel_type=None,
        market_product=None,
        region=None,
        availability_window=None,
        include_off_spec=False,
        skip=0,
        limit=20,
        db=db,
    )


class TestRejectedOwnerPublicVisibility:
    @pytest.mark.parametrize(('provenance', 'has_expiry', 'expected_price'), [
        (OrganizationProvenance.UNKNOWN, True, Decimal('1000.00')),
        (OrganizationProvenance.DEMO, False, Decimal('1000.00')),
        (OrganizationProvenance.DEMO, True, Decimal('750.00')),
    ])
    @pytest.mark.asyncio
    async def test_benchmark_matches_visible_book_provenance(
        self, db: AsyncSession, provenance, has_expiry, expected_price
    ):
        organization = await _make_org(db, 'Supplier')
        supplier = await _make_user(db, organization, role=UserRole.SUPPLIER)
        product, delivery_point = await _make_catalog(db)
        live_order = _make_order(
            org_id=organization.id,
            owner_user_id=supplier.id,
            product_id=product.id,
            delivery_point_id=delivery_point.id,
            price='1000',
        )
        comparison_order = _make_order(
            org_id=organization.id,
            owner_user_id=None,
            product_id=product.id,
            delivery_point_id=delivery_point.id,
            price='500',
        )
        comparison_order.provenance = provenance
        comparison_order.expires_at = datetime.now(UTC) + timedelta(hours=1) if has_expiry else None
        db.add_all([live_order, comparison_order])
        await db.commit()

        price = await get_live_slice_benchmark_price(
            db,
            side=OrderSide.ASK,
            market_product='BIO_METHANOL',
            delivery_point_id=delivery_point.id,
            availability_window='SPOT',
        )

        assert price == expected_price

    @pytest.mark.asyncio
    async def test_approved_market_support_admin_owner_is_public(self, db: AsyncSession):
        target_org = await _make_org(db, 'Customer Org')
        staff_org = await _make_org(db, 'Support Org')
        support_admin = await _make_user(db, staff_org, role=UserRole.ADMIN)
        product, delivery_point = await _make_catalog(db)
        db.add(
            _make_order(
                org_id=target_org.id,
                owner_user_id=support_admin.id,
                created_by_actor_user_id=support_admin.id,
                creation_method=OrderCreationMethod.MARKET_SUPPORT,
                product_id=product.id,
                delivery_point_id=delivery_point.id,
            )
        )
        await db.commit()

        asks = await _list_asks(db)

        assert asks.total == 1

    @pytest.mark.asyncio
    async def test_market_support_visibility_matches_execution_admission(self, db: AsyncSession):
        target_org = await _make_org(db, 'Customer Org')
        unapproved_org = await _make_org(db, 'Unapproved Customer', verification_status='PENDING')
        unknown_org = await _make_org(db, 'Unknown Customer')
        unknown_org.provenance = OrganizationProvenance.UNKNOWN
        staff_org = await _make_org(db, 'Support Org')
        approved_admin = await _make_user(db, staff_org, role=UserRole.ADMIN)
        other_admin = await _make_user(db, staff_org, role=UserRole.ADMIN)
        rejected_admin = await _make_user(db, staff_org, role=UserRole.ADMIN, status=UserStatus.REJECTED)
        supplier = await _make_user(db, target_org, role=UserRole.SUPPLIER)
        product, delivery_point = await _make_catalog(db)
        db.add_all(
            [
                _make_order(
                    org_id=target_org.id,
                    owner_user_id=approved_admin.id,
                    created_by_actor_user_id=approved_admin.id,
                    creation_method=OrderCreationMethod.MARKET_SUPPORT,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='1100',
                ),
                _make_order(
                    org_id=target_org.id,
                    owner_user_id=approved_admin.id,
                    created_by_actor_user_id=None,
                    creation_method=OrderCreationMethod.SELF_SERVICE,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='900',
                ),
                _make_order(
                    org_id=target_org.id,
                    owner_user_id=rejected_admin.id,
                    created_by_actor_user_id=rejected_admin.id,
                    creation_method=OrderCreationMethod.MARKET_SUPPORT,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='800',
                ),
                _make_order(
                    org_id=target_org.id,
                    owner_user_id=approved_admin.id,
                    created_by_actor_user_id=other_admin.id,
                    creation_method=OrderCreationMethod.MARKET_SUPPORT,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='700',
                ),
                _make_order(
                    org_id=target_org.id,
                    owner_user_id=supplier.id,
                    created_by_actor_user_id=supplier.id,
                    creation_method=OrderCreationMethod.MARKET_SUPPORT,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='400',
                ),
                _make_order(
                    org_id=target_org.id,
                    owner_user_id=supplier.id,
                    created_by_actor_user_id=None,
                    creation_method=OrderCreationMethod.MARKET_SUPPORT,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='300',
                ),
                _make_order(
                    org_id=target_org.id,
                    owner_user_id=None,
                    created_by_actor_user_id=None,
                    creation_method=OrderCreationMethod.MARKET_SUPPORT,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='200',
                ),
                _make_order(
                    org_id=unapproved_org.id,
                    owner_user_id=approved_admin.id,
                    created_by_actor_user_id=approved_admin.id,
                    creation_method=OrderCreationMethod.MARKET_SUPPORT,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='600',
                ),
                _make_order(
                    org_id=unknown_org.id,
                    owner_user_id=approved_admin.id,
                    created_by_actor_user_id=approved_admin.id,
                    creation_method=OrderCreationMethod.MARKET_SUPPORT,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='500',
                ),
            ]
        )
        await db.commit()

        asks = await _list_asks(db)

        assert asks.total == 1
        assert asks.items[0].price_per_mt_usd == Decimal('1100')

    @pytest.mark.asyncio
    async def test_qualified_market_support_order_included_in_live_vwap(self, db: AsyncSession):
        target_org = await _make_org(db, 'Customer Org')
        staff_org = await _make_org(db, 'Support Org')
        support_admin = await _make_user(db, staff_org, role=UserRole.ADMIN)
        supplier = await _make_user(db, target_org, role=UserRole.SUPPLIER)
        product, delivery_point = await _make_catalog(db)
        db.add_all(
            [
                _make_order(
                    org_id=target_org.id,
                    owner_user_id=support_admin.id,
                    created_by_actor_user_id=support_admin.id,
                    creation_method=OrderCreationMethod.MARKET_SUPPORT,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='900',
                    quantity='100',
                ),
                _make_order(
                    org_id=target_org.id,
                    owner_user_id=supplier.id,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='1100',
                    quantity='100',
                ),
            ]
        )
        await db.commit()

        benchmark = await rebuild_live_slice_benchmark(
            db,
            side=OrderSide.ASK,
            market_product='BIO_METHANOL',
            delivery_point_id=delivery_point.id,
            availability_window='SPOT',
        )

        assert benchmark == Decimal('1000.00')

        supplier.kyc_status = 'REJECTED'
        await db.flush()

        fresh_price = await get_live_slice_benchmark_price(
            db,
            side=OrderSide.ASK,
            market_product='BIO_METHANOL',
            delivery_point_id=delivery_point.id,
            availability_window='SPOT',
        )

        assert fresh_price == Decimal('900.00')
        assert not db.new
        assert not db.dirty
        stored = (await db.execute(select(LiveSliceBenchmark))).scalar_one()
        assert stored.benchmark_price_per_mt_usd == Decimal('1000.00')

    @pytest.mark.asyncio
    async def test_kyc_rejected_owner_ask_hidden_from_asks_and_aggregated(self, db: AsyncSession):
        org = await _make_org(db, 'Supplier Org')
        eligible = await _make_user(db, org)
        rejected = await _make_user(db, org, kyc_status='REJECTED')
        product, delivery_point = await _make_catalog(db)
        db.add_all(
            [
                _make_order(
                    org_id=org.id,
                    owner_user_id=eligible.id,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='1100',
                ),
                _make_order(
                    org_id=org.id,
                    owner_user_id=rejected.id,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='900',
                ),
            ]
        )
        await db.commit()

        asks = await _list_asks(db)
        assert asks.total == 1
        assert [item.price_per_mt_usd for item in asks.items] == [Decimal('1100')]

        aggregated = await list_aggregated_orderbook(
            product_id=None,
            delivery_point_id=None,
            fuel_type=None,
            market_product=None,
            region=None,
            availability_window=None,
            include_off_spec=False,
            limit=256,
            db=db,
        )
        assert len(aggregated) == 1
        assert aggregated[0].order_count == 1
        assert aggregated[0].min_price == Decimal('1100')

    @pytest.mark.asyncio
    async def test_admin_rejected_owner_bid_hidden_from_bids(self, db: AsyncSession):
        org = await _make_org(db, 'Buyer Org')
        eligible = await _make_user(db, org, role=UserRole.BUYER)
        rejected = await _make_user(db, org, role=UserRole.BUYER, status=UserStatus.REJECTED)
        product, delivery_point = await _make_catalog(db)
        db.add_all(
            [
                _make_order(
                    org_id=org.id,
                    owner_user_id=eligible.id,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    side=OrderSide.BID,
                    price='950',
                ),
                _make_order(
                    org_id=org.id,
                    owner_user_id=rejected.id,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    side=OrderSide.BID,
                    price='990',
                ),
            ]
        )
        await db.commit()

        bids = await _list_bids(db)
        assert bids.total == 1
        assert [item.price_per_mt_usd for item in bids.items] == [Decimal('950')]

    @pytest.mark.asyncio
    async def test_is_crossed_ignores_rejected_owner_opposing_order(self, db: AsyncSession):
        buyer_org = await _make_org(db, 'Buyer Org')
        seller_org = await _make_org(db, 'Seller Org')
        buyer = await _make_user(db, buyer_org, role=UserRole.BUYER)
        rejected_seller = await _make_user(db, seller_org, kyc_status='REJECTED')
        product, delivery_point = await _make_catalog(db)
        db.add_all(
            [
                _make_order(
                    org_id=buyer_org.id,
                    owner_user_id=buyer.id,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    side=OrderSide.BID,
                    price='1000',
                ),
                # Crosses the bid on price, but its owner is rejected: the
                # bid must NOT report is_crossed against inert liquidity.
                _make_order(
                    org_id=seller_org.id,
                    owner_user_id=rejected_seller.id,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='900',
                ),
            ]
        )
        await db.commit()

        bids = await _list_bids(db)
        assert bids.total == 1
        assert bids.items[0].is_crossed is False

        # Control: the same crossing ask from an eligible owner does cross.
        eligible_seller = await _make_user(db, seller_org)
        db.add(
            _make_order(
                org_id=seller_org.id,
                owner_user_id=eligible_seller.id,
                product_id=product.id,
                delivery_point_id=delivery_point.id,
                price='900',
            )
        )
        await db.commit()
        bids = await _list_bids(db)
        assert bids.items[0].is_crossed is True

    @pytest.mark.asyncio
    async def test_benchmark_rebuild_excludes_rejected_owner(self, db: AsyncSession):
        org = await _make_org(db, 'Supplier Org')
        eligible = await _make_user(db, org)
        rejected = await _make_user(db, org, kyc_status='REJECTED')
        product, delivery_point = await _make_catalog(db)
        db.add_all(
            [
                _make_order(
                    org_id=org.id,
                    owner_user_id=eligible.id,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='1000',
                    quantity='100',
                ),
                _make_order(
                    org_id=org.id,
                    owner_user_id=rejected.id,
                    product_id=product.id,
                    delivery_point_id=delivery_point.id,
                    price='500',
                    quantity='100',
                ),
            ]
        )
        await db.commit()

        benchmark = await rebuild_live_slice_benchmark(
            db,
            side=OrderSide.ASK,
            market_product='BIO_METHANOL',
            delivery_point_id=delivery_point.id,
            availability_window='SPOT',
        )
        # VWAP over the eligible order only, never dragged down by the
        # rejected owner's inert 500-priced order.
        assert benchmark == Decimal('1000.00')

    @pytest.mark.asyncio
    async def test_owner_moved_to_other_org_is_hidden(self, db: AsyncSession):
        org = await _make_org(db, 'Supplier Org')
        other_org = await _make_org(db, 'Other Org')
        mover = await _make_user(db, org, organization_id=other_org.id)
        product, delivery_point = await _make_catalog(db)
        db.add(
            _make_order(
                org_id=org.id,
                owner_user_id=mover.id,
                product_id=product.id,
                delivery_point_id=delivery_point.id,
            )
        )
        await db.commit()

        asks = await _list_asks(db)
        assert asks.total == 0

    @pytest.mark.asyncio
    async def test_ownerless_legacy_order_keeps_existing_posture(self, db: AsyncSession):
        """Pre-ownership rows keep the provenance-clause posture (visible).

        Fill-time still fails closed on a null owner; the query-side mirror
        deliberately gates only orders with a recorded owner.
        """
        org = await _make_org(db, 'Legacy Org')
        product, delivery_point = await _make_catalog(db)
        db.add(
            _make_order(
                org_id=org.id,
                owner_user_id=None,
                product_id=product.id,
                delivery_point_id=delivery_point.id,
            )
        )
        await db.commit()

        asks = await _list_asks(db)
        assert asks.total == 1
