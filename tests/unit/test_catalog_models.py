"""Unit tests for Product and DeliveryPoint catalog models.

Uses an in-memory SQLite database following the same pattern as test_matching_engine.py.
"""
import pytest
from decimal import Decimal

from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession, async_sessionmaker

from app.database import Base
from app.models.catalog import Product, DeliveryPoint


_REQUIRED_TABLES = [
    "products",
    "delivery_points",
]


# --------------- Fixtures ---------------


@pytest.fixture(scope="module")
def async_engine():
    """Create an async SQLite engine for tests."""
    engine = create_async_engine(
        "sqlite+aiosqlite://",
        echo=False,
        future=True,
    )
    return engine


@pytest.fixture(scope="module")
async def setup_tables(async_engine):
    """Create only the tables we need."""
    tables = [Base.metadata.tables[t] for t in _REQUIRED_TABLES if t in Base.metadata.tables]
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    yield
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.drop_all, tables=tables)


@pytest.fixture
async def db(async_engine, setup_tables):
    """Provide a fresh async session that rolls back after each test."""
    session_factory = async_sessionmaker(
        bind=async_engine,
        class_=AsyncSession,
        expire_on_commit=False,
        autoflush=False,
    )
    async with session_factory() as session:
        yield session
        await session.rollback()


# --------------- Tests ---------------


class TestProduct:
    """Product model CRUD and constraints."""

    @pytest.mark.asyncio
    async def test_create_product(self, db):
        """Create a Product and verify all fields are persisted correctly."""
        product = Product(
            name="VLSFO Conventional",
            fuel_type="VLSFO",
            fuel_grade="Conventional",
            unit="MT",
            min_lot_size=Decimal("100.00"),
            spec_description="Very Low Sulphur Fuel Oil, max 0.5% sulphur",
            is_active=True,
        )
        db.add(product)
        await db.flush()

        assert product.id is not None
        assert product.name == "VLSFO Conventional"
        assert product.fuel_type == "VLSFO"
        assert product.fuel_grade == "Conventional"
        assert product.unit == "MT"
        assert product.min_lot_size == Decimal("100.00")
        assert product.spec_description == "Very Low Sulphur Fuel Oil, max 0.5% sulphur"
        assert product.is_active is True
        assert repr(product) == "<Product VLSFO Conventional>"

    @pytest.mark.asyncio
    async def test_product_defaults(self, db):
        """Product should use sensible defaults for optional fields."""
        product = Product(
            name="MGO Standard",
            fuel_type="MGO",
            fuel_grade="Conventional",
        )
        db.add(product)
        await db.flush()

        assert product.unit == "MT"
        assert product.min_lot_size == Decimal("100")
        assert product.is_active is True
        assert product.spec_description is None

    @pytest.mark.asyncio
    async def test_product_name_unique(self, db):
        """Duplicate product names should raise IntegrityError."""
        from sqlalchemy.exc import IntegrityError

        p1 = Product(
            name="Unique Product Test",
            fuel_type="VLSFO",
            fuel_grade="Conventional",
        )
        db.add(p1)
        await db.flush()

        p2 = Product(
            name="Unique Product Test",
            fuel_type="MGO",
            fuel_grade="Bio",
        )
        db.add(p2)
        with pytest.raises(IntegrityError):
            await db.flush()
        await db.rollback()

    @pytest.mark.parametrize(
        ("name", "fuel_type", "fuel_grade", "expected_market_product"),
        [
            ("Bio Methanol", "Methanol", "Bio", "BIO_METHANOL"),
            ("Bio Ethanol", "Ethanol", "Bio", "BIO_ETHANOL"),
            ("e-Methanol", "Methanol", "E", "E_METHANOL"),
            ("Synthetic Ethanol", "Ethanol", "Synthetic", "SYNTHETIC_ETHANOL"),
        ],
    )
    def test_market_product_maps_supported_products(
        self,
        name,
        fuel_type,
        fuel_grade,
        expected_market_product,
    ):
        product = Product(
            name=name,
            fuel_type=fuel_type,
            fuel_grade=fuel_grade,
        )

        assert product.market_product == expected_market_product

    def test_market_product_is_none_for_unsupported_products(self):
        product = Product(
            name="Ammonia Green",
            fuel_type="Ammonia",
            fuel_grade="Green",
        )

        assert product.market_product is None

    @pytest.mark.parametrize(
        ("name", "fuel_type", "fuel_grade"),
        [
            ("Methanol Green", "Methanol", "Green"),
            ("Ethanol Green", "Ethanol", "Green"),
            ("Bio Methanol", "Methanol", "Green"),
        ],
    )
    def test_market_product_does_not_reinterpret_legacy_or_forged_labels(
        self,
        name,
        fuel_type,
        fuel_grade,
    ):
        assert Product(
            name=name,
            fuel_type=fuel_type,
            fuel_grade=fuel_grade,
        ).market_product is None


class TestDeliveryPoint:
    """DeliveryPoint model CRUD and constraints."""

    @pytest.mark.asyncio
    async def test_create_delivery_point(self, db):
        """Create a DeliveryPoint and verify all fields are persisted correctly."""
        dp = DeliveryPoint(
            name="Singapore",
            region="Asia",
            timezone="Asia/Singapore",
            is_active=True,
        )
        db.add(dp)
        await db.flush()

        assert dp.id is not None
        assert dp.name == "Singapore"
        assert dp.region == "Asia"
        assert dp.timezone == "Asia/Singapore"
        assert dp.is_active is True
        assert repr(dp) == "<DeliveryPoint Singapore (Asia)>"

    @pytest.mark.asyncio
    async def test_delivery_point_defaults(self, db):
        """DeliveryPoint should default is_active to True, timezone to None."""
        dp = DeliveryPoint(
            name="Test Port Defaults",
            region="Americas",
        )
        db.add(dp)
        await db.flush()

        assert dp.is_active is True
        assert dp.timezone is None

    @pytest.mark.asyncio
    async def test_delivery_point_name_unique(self, db):
        """Duplicate delivery point names should raise IntegrityError."""
        from sqlalchemy.exc import IntegrityError

        dp1 = DeliveryPoint(
            name="Unique DP Test",
            region="Europe",
        )
        db.add(dp1)
        await db.flush()

        dp2 = DeliveryPoint(
            name="Unique DP Test",
            region="Asia",
        )
        db.add(dp2)
        with pytest.raises(IntegrityError):
            await db.flush()
        await db.rollback()
