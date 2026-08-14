"""Unit tests for news feed dedupe hardening."""
from datetime import UTC, datetime
from types import SimpleNamespace

import httpx
import pytest
from pydantic import SecretStr
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine
from sqlalchemy.pool import StaticPool

from app.config import Settings
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


def test_webz_endpoint_is_locked_to_prevent_token_exfiltration():
    with pytest.raises(ValueError, match="WEBZ_API_URL"):
        Settings(
            ENVIRONMENT="test",
            JWT_SECRET="x" * 32,
            WEBZ_API_URL="https://attacker.example/collect",
        )


def _webz_post(**overrides) -> dict:
    post = {
        "title": "Bio-methanol bunkering expands in Singapore",
        "url": "https://publisher.example/news/bio-methanol-singapore",
        "published": "2026-08-14T01:02:03.000+0000",
        "summary": "A supplier announced additional certified marine-fuel capacity.",
        "thread": {
            "site": "Publisher News",
            "site_full": "publisher.example",
        },
    }
    post.update(overrides)
    return post


class _WebzClient:
    def __init__(self, response: httpx.Response | Exception, captured: dict):
        self.response = response
        self.captured = captured

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_args):
        return None

    def stream(self, _method, url, *, params):
        self.captured.update({"url": url, "params": params})
        response = self.response

        class Stream:
            async def __aenter__(self):
                if isinstance(response, Exception):
                    raise response
                return response

            async def __aexit__(self, *_args):
                return None

        return Stream()


@pytest.mark.asyncio
async def test_webz_is_primary_and_normalizes_bounded_provider_records(monkeypatch):
    captured = {}
    request = httpx.Request("GET", "https://api.webz.io/filterWebContent")
    response = httpx.Response(200, request=request, json={"posts": [_webz_post()]})
    monkeypatch.setattr(news_feed.settings, "WEBZ_API_TOKEN", SecretStr("provider-secret"))
    monkeypatch.setattr(
        news_feed.httpx,
        "AsyncClient",
        lambda **_kwargs: _WebzClient(response, captured),
    )

    async def rss_must_not_run():
        raise AssertionError("RSS should not run after a usable Webz response")

    monkeypatch.setattr(news_feed, "fetch_all_feeds", rss_must_not_run)

    assert await news_feed.fetch_news_items() == [
        {
            "title": "Bio-methanol bunkering expands in Singapore",
            "url": "https://publisher.example/news/bio-methanol-singapore",
            "source": "Publisher News",
            "source_url": "https://publisher.example",
            "published_at": datetime(2026, 8, 14, 1, 2, 3, tzinfo=UTC),
            "summary": "A supplier announced additional certified marine-fuel capacity.",
        }
    ]
    assert captured["url"] == "https://api.webz.io/filterWebContent"
    assert captured["params"]["token"] == "provider-secret"
    assert captured["params"]["size"] == news_feed.WEBZ_MAX_ENTRIES_PER_RUN
    assert captured["params"]["includeSyndicated"] == "false"
    assert "bio methanol" in captured["params"]["q"]
    assert "synthetic ethanol" in captured["params"]["q"]


@pytest.mark.asyncio
async def test_webz_rejects_malformed_records_but_keeps_valid_records(monkeypatch):
    request = httpx.Request("GET", "https://api.webz.io/filterWebContent")
    response = httpx.Response(
        200,
        request=request,
        json={
            "posts": [
                _webz_post(url="http://127.0.0.1/private"),
                _webz_post(title="Bad\nTitle"),
                _webz_post(url="https://publisher.example/news/valid"),
            ]
        },
    )
    monkeypatch.setattr(news_feed.settings, "WEBZ_API_TOKEN", SecretStr("provider-secret"))
    monkeypatch.setattr(
        news_feed.httpx,
        "AsyncClient",
        lambda **_kwargs: _WebzClient(response, {}),
    )

    items = await news_feed.fetch_webz_items()

    assert items is not None
    assert [item["url"] for item in items] == ["https://publisher.example/news/valid"]


@pytest.mark.asyncio
async def test_webz_response_body_is_streamed_under_a_hard_byte_cap(monkeypatch):
    request = httpx.Request("GET", "https://api.webz.io/filterWebContent")
    response = httpx.Response(200, request=request, content=b"123456789")
    monkeypatch.setattr(news_feed.settings, "WEBZ_API_TOKEN", SecretStr("provider-secret"))
    monkeypatch.setattr(news_feed, "WEBZ_MAX_RESPONSE_BYTES", 8)
    monkeypatch.setattr(
        news_feed.httpx,
        "AsyncClient",
        lambda **_kwargs: _WebzClient(response, {}),
    )

    assert await news_feed.fetch_webz_items() is None


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "provider_result",
    [
        httpx.ConnectTimeout("provider timed out"),
        httpx.Response(
            503,
            request=httpx.Request("GET", "https://api.webz.io/filterWebContent"),
        ),
        httpx.Response(
            200,
            request=httpx.Request("GET", "https://api.webz.io/filterWebContent"),
            content=b"not-json",
        ),
        httpx.Response(
            200,
            request=httpx.Request("GET", "https://api.webz.io/filterWebContent"),
            json={"posts": []},
        ),
    ],
)
async def test_webz_failure_or_empty_result_falls_back_to_rss(
    monkeypatch, provider_result
):
    monkeypatch.setattr(news_feed.settings, "WEBZ_API_TOKEN", SecretStr("provider-secret"))
    monkeypatch.setattr(
        news_feed.httpx,
        "AsyncClient",
        lambda **_kwargs: _WebzClient(provider_result, {}),
    )
    rss_items = [_raw_item("https://shipandbunker.com/news/fallback")]

    async def fake_rss():
        return rss_items

    monkeypatch.setattr(news_feed, "fetch_all_feeds", fake_rss)

    assert await news_feed.fetch_news_items() == rss_items


@pytest.mark.asyncio
async def test_unconfigured_webz_uses_rss_without_provider_request(monkeypatch):
    monkeypatch.setattr(news_feed.settings, "WEBZ_API_TOKEN", None)
    rss_items = [_raw_item("https://shipandbunker.com/news/unconfigured")]

    async def fake_rss():
        return rss_items

    monkeypatch.setattr(news_feed, "fetch_all_feeds", fake_rss)
    monkeypatch.setattr(
        news_feed.httpx,
        "AsyncClient",
        lambda **_kwargs: (_ for _ in ()).throw(
            AssertionError("Webz client must not be constructed without a token")
        ),
    )

    assert await news_feed.fetch_news_items() == rss_items


def test_malicious_and_oversized_feed_entries_are_rejected_before_storage():
    feed = {
        "name": "Publisher",
        "url": "https://publisher.example/rss",
        "article_domains": ("publisher.example",),
    }
    malicious_entries = (
        SimpleNamespace(title="Script", link="javascript:alert(1)"),
        SimpleNamespace(title="Data", link="data:text/html,unsafe"),
        SimpleNamespace(title="File", link="file:///etc/passwd"),
        SimpleNamespace(title="Private", link="http://127.0.0.1/admin"),
        SimpleNamespace(title="Credentials", link="https://user:pass@publisher.example/story"),
        SimpleNamespace(title="Wrong host", link="https://attacker.example/story"),
        SimpleNamespace(title="Log\ninjection", link="https://publisher.example/story"),
        SimpleNamespace(title="x" * (news_feed.NEWS_TITLE_MAX_CHARS + 1), link="https://publisher.example/story"),
        SimpleNamespace(title="Long URL", link="https://publisher.example/" + "x" * news_feed.NEWS_URL_MAX_CHARS),
    )

    assert all(news_feed._validated_feed_entry(entry, feed) is None for entry in malicious_entries)
    item = news_feed._validated_feed_entry(
        SimpleNamespace(title=" Safe headline ", link="https://news.publisher.example/story?id=1"),
        feed,
    )
    assert item is not None
    published_at = item.pop("published_at")
    assert item == {
        "title": "Safe headline",
        "url": "https://news.publisher.example/story?id=1",
        "source": "Publisher",
        "source_url": "https://publisher.example/rss",
    }
    assert abs((datetime.now(UTC) - published_at).total_seconds()) < 2


@pytest.mark.asyncio
async def test_streamed_feed_reader_rejects_body_over_byte_cap():
    class Response:
        async def aiter_bytes(self, **_kwargs):
            yield b"x" * news_feed.RSS_MAX_BYTES_PER_FEED
            yield b"overflow"

    assert await news_feed._read_feed_body(Response()) is None


def test_high_item_feed_and_run_are_bounded():
    entries = list(range(news_feed.RSS_MAX_ENTRIES_PER_FEED + 50))
    assert len(news_feed._bounded_feed_entries(entries)) == news_feed.RSS_MAX_ENTRIES_PER_FEED
    items = [
        _raw_item(f"https://example.com/{index}", title=f"Headline {index}")
        for index in range(news_feed.RSS_MAX_ENTRIES_PER_RUN + 50)
    ]
    assert len(news_feed._dedupe_fetched_items(items)) == news_feed.RSS_MAX_ENTRIES_PER_RUN


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
    async def test_deduplicates_same_source_and_title_under_different_urls(
        self, db, monkeypatch
    ):
        first = _raw_item(
            "https://publisher.example/news/story",
            title="FuelEU Maritime update",
        )
        first["source"] = "Publisher"
        second = {
            **first,
            "url": "https://publisher.example/news/story?output=amp",
        }

        async def fake_fetch_news_items():
            return [first, second]

        async def fake_categorize_headline(_title: str):
            return {"category": "regulation", "relevance": 4, "summary": None}

        monkeypatch.setattr(news_feed, "fetch_news_items", fake_fetch_news_items)
        monkeypatch.setattr(news_feed, "categorize_headline", fake_categorize_headline)

        assert await news_feed.refresh_news(db) == 1
        assert await db.scalar(select(func.count()).select_from(NewsItem)) == 1

    @pytest.mark.asyncio
    async def test_preserves_validated_provider_summary_when_classifier_has_none(
        self, db, monkeypatch
    ):
        item = _raw_item("https://example.com/news-summary")
        item["summary"] = "Licensed provider summary"

        async def fake_fetch_news_items():
            return [item]

        async def fake_categorize_headline(_title: str):
            return {"category": "bunkers", "relevance": 5, "summary": None}

        monkeypatch.setattr(news_feed, "fetch_news_items", fake_fetch_news_items)
        monkeypatch.setattr(news_feed, "categorize_headline", fake_categorize_headline)

        assert await news_feed.refresh_news(db) == 1
        stored = await db.scalar(select(NewsItem))
        assert stored is not None
        assert stored.summary == "Licensed provider summary"

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
