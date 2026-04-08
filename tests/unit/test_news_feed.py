"""Unit tests for news feed dedupe hardening."""
from datetime import UTC, datetime

import pytest
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine
from sqlalchemy.pool import StaticPool

from app.database import Base
from app.models.news import NewsItem
from app.services import news_feed


@pytest.fixture(scope="module")
def async_engine():
    return create_async_engine(
        "sqlite+aiosqlite:///:memory:",
        future=True,
        connect_args={"check_same_thread": False},
        poolclass=StaticPool,
    )


@pytest.fixture(scope="module")
async def setup_tables(async_engine):
    tables = [Base.metadata.tables["news_items"]]
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    yield
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.drop_all, tables=tables)


@pytest.fixture
async def session_factory(async_engine, setup_tables):
    return async_sessionmaker(
        bind=async_engine,
        class_=AsyncSession,
        expire_on_commit=False,
        autoflush=False,
    )


@pytest.fixture
async def db(session_factory):
    async with session_factory() as session:
        yield session
        await session.rollback()
        await session.execute(NewsItem.__table__.delete())
        await session.commit()


def _raw_item(url: str, title: str = "Headline") -> dict:
    return {
        "title": title,
        "url": url,
        "source": "Ship & Bunker",
        "source_url": "https://shipandbunker.com/rss",
        "published_at": datetime.now(UTC),
    }


class TestRefreshNewsDedupe:
    @pytest.mark.asyncio
    async def test_deduplicates_duplicate_urls_within_single_fetch_batch(self, db, monkeypatch):
        async def fake_fetch_all_feeds():
            return [
                _raw_item("https://example.com/news-1", title="First title"),
                _raw_item("https://example.com/news-1", title="Second title"),
            ]

        async def fake_categorize_headline(_title: str):
            return {"category": "markets", "relevance": 3, "summary": None}

        monkeypatch.setattr(news_feed, "fetch_all_feeds", fake_fetch_all_feeds)
        monkeypatch.setattr(news_feed, "categorize_headline", fake_categorize_headline)

        inserted = await news_feed.refresh_news(db)

        assert inserted == 1
        count = await db.scalar(select(func.count()).select_from(NewsItem))
        assert count == 1

    @pytest.mark.asyncio
    async def test_ignores_duplicate_insert_race_after_initial_url_check(self, db, session_factory, monkeypatch):
        inserted_elsewhere = False

        async def fake_fetch_all_feeds():
            return [_raw_item("https://example.com/news-race", title="Race condition headline")]

        async def fake_categorize_headline(_title: str):
            nonlocal inserted_elsewhere
            if not inserted_elsewhere:
                inserted_elsewhere = True
                async with session_factory() as other_session:
                    other_session.add(
                        NewsItem(
                            title="Other worker insert",
                            url="https://example.com/news-race",
                            source="TradeWinds",
                            source_url="https://www.tradewindsnews.com/rss",
                            published_at=datetime.now(UTC),
                            category="markets",
                            relevance=4,
                            summary=None,
                            fetched_at=datetime.now(UTC),
                        )
                    )
                    await other_session.commit()
            return {"category": "markets", "relevance": 3, "summary": None}

        monkeypatch.setattr(news_feed, "fetch_all_feeds", fake_fetch_all_feeds)
        monkeypatch.setattr(news_feed, "categorize_headline", fake_categorize_headline)

        inserted = await news_feed.refresh_news(db)

        assert inserted == 0
        count = await db.scalar(select(func.count()).select_from(NewsItem))
        assert count == 1
