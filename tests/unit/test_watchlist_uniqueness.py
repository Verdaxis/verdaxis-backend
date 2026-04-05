"""SQLite-backed tests for watchlist entry uniqueness constraints."""
import pytest
import uuid

from sqlalchemy.exc import IntegrityError
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.models.user import User
from app.models.watchlist import Watchlist, WatchlistEntry
from app.models.catalog import Product, DeliveryPoint


REQUIRED_TABLES = [
    "users",
    "watchlists",
    "watchlist_entries",
    "products",
    "delivery_points",
]


@pytest.fixture(scope="module")
def async_engine():
    return create_async_engine("sqlite+aiosqlite://", echo=False, future=True)


@pytest.fixture(scope="module")
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
        yield session
        await session.rollback()


async def _make_user(db) -> User:
    user = User(email=f"user-{uuid.uuid4().hex[:8]}@example.com", password_hash="hashed")
    db.add(user)
    await db.flush()
    return user


async def _make_watchlist(db, user_id) -> Watchlist:
    watchlist = Watchlist(user_id=user_id, name="Fuel Watchlist")
    db.add(watchlist)
    await db.flush()
    return watchlist


async def _make_product(db) -> Product:
    product = Product(name=f"Product {uuid.uuid4().hex[:8]}", fuel_type="Methanol", fuel_grade="Green")
    db.add(product)
    await db.flush()
    return product


async def _make_delivery_point(db) -> DeliveryPoint:
    dp = DeliveryPoint(name=f"Delivery Point {uuid.uuid4().hex[:8]}", region="Asia")
    db.add(dp)
    await db.flush()
    return dp


class TestWatchlistEntryUniqueness:
    @pytest.mark.asyncio
    @pytest.mark.parametrize("delivery_point", [None, True])
    async def test_duplicate_watchlist_entry_combination_is_rejected(self, db, delivery_point):
        user = await _make_user(db)
        watchlist = await _make_watchlist(db, user.id)
        product = await _make_product(db)
        dp = await _make_delivery_point(db) if delivery_point else None

        first = WatchlistEntry(
            watchlist_id=watchlist.id,
            product_id=product.id,
            delivery_point_id=dp.id if dp else None,
        )
        db.add(first)
        await db.flush()

        duplicate = WatchlistEntry(
            watchlist_id=watchlist.id,
            product_id=product.id,
            delivery_point_id=dp.id if dp else None,
        )
        db.add(duplicate)

        with pytest.raises(IntegrityError):
            await db.flush()
