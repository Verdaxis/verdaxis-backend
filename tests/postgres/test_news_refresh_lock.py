"""News CLI overlap is coordinated by PostgreSQL, not worker-local state."""

import asyncio

import pytest
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.services import news_feed


@pytest.mark.asyncio
async def test_news_refresh_advisory_lock_rejects_timer_cli_overlap(
    pg_session, monkeypatch
):
    engine, _session = pg_session
    first_fetch_started = asyncio.Event()
    release_first_fetch = asyncio.Event()

    async def blocking_empty_fetch():
        first_fetch_started.set()
        await release_first_fetch.wait()
        return []

    monkeypatch.setattr(news_feed, "fetch_all_feeds", blocking_empty_fetch)
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)

    async with factory() as first_session, factory() as second_session:
        first_refresh = asyncio.create_task(news_feed.refresh_news(first_session))
        await asyncio.wait_for(first_fetch_started.wait(), timeout=2)

        with pytest.raises(news_feed.NewsRefreshInProgress):
            await asyncio.wait_for(news_feed.refresh_news(second_session), timeout=1)
        await second_session.rollback()

        release_first_fetch.set()
        assert await first_refresh == 0
        await first_session.commit()
