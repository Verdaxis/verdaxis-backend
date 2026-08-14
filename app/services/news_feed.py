import feedparser
import asyncio
import io
import ipaddress
import json
import re
import threading
import time
from contextvars import ContextVar
from dataclasses import dataclass, field
from datetime import datetime, UTC
from time import mktime
from typing import Optional
from urllib.parse import urlsplit, urlunsplit

import structlog
import httpx
from sqlalchemy import select, text
from sqlalchemy.dialects.postgresql import insert as postgresql_insert
from sqlalchemy.dialects.sqlite import insert as sqlite_insert
from sqlalchemy.ext.asyncio import AsyncSession

from app.config import settings
from app.models.news import NewsItem
from app.services.gemini_provider import (
    ProviderCapacity,
    get_gemini_model,
    request_options,
    run_provider_call,
)

logger = structlog.get_logger()

GEMINI_MODEL = "gemini-2.5-flash-lite"
NEWS_PROVIDER_TIMEOUT_SECONDS = 15
RSS_FETCH_TIMEOUT_SECONDS = 15
RSS_MAX_CONCURRENT = 4
RSS_MAX_BYTES_PER_FEED = 2 * 1024 * 1024
RSS_READ_CHUNK_BYTES = 64 * 1024
RSS_MAX_ENTRIES_PER_FEED = 100
RSS_MAX_ENTRIES_PER_RUN = 250
NEWS_MAX_PROVIDER_CALLS_PER_RUN = 50
NEWS_PROVIDER_MAX_CONCURRENT = 2
NEWS_REFRESH_ADVISORY_LOCK_ID = 6216461178696259923
NEWS_TITLE_MAX_CHARS = 500
NEWS_URL_MAX_CHARS = 1000
NEWS_SOURCE_MAX_CHARS = 100
NEWS_SUMMARY_MAX_CHARS = 1000
WEBZ_MAX_ENTRIES_PER_RUN = 100
WEBZ_MAX_RESPONSE_BYTES = 8 * 1024 * 1024
_WEBZ_FUELS = (
    '("bio methanol" OR biomethanol OR "e-methanol" OR "e methanol" OR '
    '"synthetic methanol" OR "bio ethanol" OR bioethanol OR "e-ethanol" '
    'OR "e ethanol" OR "synthetic ethanol")'
)
_WEBZ_MARITIME = "(marine OR maritime OR bunker OR bunkering OR shipping)"
_WEBZ_REGULATION = (
    '("FuelEU Maritime" OR "marine EU ETS" OR "IMO GFI" OR '
    '"marine fuel regulation")'
)
WEBZ_QUERY = (
    f"site_type:news AND (((title:{_WEBZ_FUELS} OR summary:{_WEBZ_FUELS}) "
    f"AND (title:{_WEBZ_MARITIME} OR summary:{_WEBZ_MARITIME})) OR "
    f"title:{_WEBZ_REGULATION} OR summary:{_WEBZ_REGULATION})"
)
_news_provider_capacity = ProviderCapacity(NEWS_PROVIDER_MAX_CONCURRENT)

RSS_FEEDS = [
    {
        "name": "TradeWinds",
        "url": "https://www.tradewindsnews.com/rss",
        "article_domains": ("tradewindsnews.com",),
    },
    {
        "name": "Ship & Bunker",
        "url": "https://shipandbunker.com/rss",
        "article_domains": ("shipandbunker.com",),
    },
    {
        "name": "Splash247",
        "url": "https://splash247.com/feed/",
        "article_domains": ("splash247.com",),
    },
    {
        "name": "Maritime Executive",
        "url": "https://maritime-executive.com/rss",
        "article_domains": ("maritime-executive.com",),
    },
    {
        "name": "Hellenic Shipping News",
        "url": "https://www.hellenicshippingnews.com/feed/",
        "article_domains": ("hellenicshippingnews.com",),
    },
]

VALID_CATEGORIES = {"shipping", "bunkers", "regulation", "carbon", "commodities", "markets"}


@dataclass
class _RefreshProviderState:
    """Run-scoped provider state; never carries a key or provider response."""

    provider_enabled: bool
    logged_reasons: set[str] = field(default_factory=set)
    provider_calls_remaining: int = NEWS_MAX_PROVIDER_CALLS_PER_RUN

    def disable(self, reason: str) -> None:
        self.provider_enabled = False
        if reason in self.logged_reasons:
            return
        self.logged_reasons.add(reason)
        logger.warning("news_feed.provider_disabled", reason=reason)

    def note(self, reason: str, *, level: str = "warning") -> None:
        if reason in self.logged_reasons:
            return
        self.logged_reasons.add(reason)
        getattr(logger, level)("news_feed.provider_unavailable", reason=reason)


_refresh_provider_state: ContextVar[_RefreshProviderState | None] = ContextVar(
    "news_refresh_provider_state", default=None
)


class _BoundedCircuitBreaker:
    """Thread-safe, process-local failure threshold with a bounded cooldown."""

    def __init__(self, threshold: int = 3, cooldown_seconds: float = 60.0):
        self.threshold = threshold
        self.cooldown_seconds = cooldown_seconds
        self.failures = 0
        self.opened_until = 0.0
        self._lock = threading.Lock()

    def allow(self) -> bool:
        with self._lock:
            return time.monotonic() >= self.opened_until

    def success(self) -> None:
        with self._lock:
            self.failures = 0
            self.opened_until = 0.0

    def failure(self) -> None:
        with self._lock:
            self.failures += 1
            if self.failures >= self.threshold:
                self.opened_until = time.monotonic() + self.cooldown_seconds


_gemini_breaker = _BoundedCircuitBreaker()


def _local_categorize_headline(title: str) -> dict:
    """Deterministic fallback used when Gemini is unavailable or disabled."""
    lowered = title.casefold()
    keyword_categories = (
        ("regulation", ("imo", "regulation", "sanction", "compliance", "mandate", "law")),
        ("carbon", ("carbon", "emission", "ets", "fuel eu", "decarbon")),
        ("bunkers", ("bunker", "marine fuel", "lng", "methanol", "biofuel")),
        ("commodities", ("commodity", "commodities", "crude", "oil price", "energy")),
        ("shipping", ("shipping", "vessel", "tanker", "freight", "port")),
        ("markets", ("market", "trading", "trade", "price")),
    )
    for category, keywords in keyword_categories:
        if any(keyword in lowered for keyword in keywords):
            return {"category": category, "relevance": 3, "summary": None}
    return {"category": "markets", "relevance": 3, "summary": None}


def _is_provider_auth_failure(exc: Exception) -> bool:
    """Classify auth failures without logging provider exception text."""
    status_code = getattr(exc, "code", None) or getattr(exc, "status_code", None)
    if status_code in {401, 403}:
        return True
    message = str(exc).casefold()
    return any(
        marker in message
        for marker in (
            "permission denied",
            "unauthorized",
            "unauthenticated",
            "invalid api key",
            "api key was reported as leaked",
            "reported as leaked",
        )
    )


def _parse_published(entry: dict) -> datetime:
    """Extract published datetime from a feed entry, falling back to now."""
    if hasattr(entry, "published_parsed") and entry.published_parsed:
        return datetime.fromtimestamp(mktime(entry.published_parsed), tz=UTC)
    if hasattr(entry, "updated_parsed") and entry.updated_parsed:
        return datetime.fromtimestamp(mktime(entry.updated_parsed), tz=UTC)
    return datetime.now(UTC)


def _validated_public_http_url(
    raw_url: object, *, max_chars: int = NEWS_URL_MAX_CHARS
) -> str | None:
    """Return a bounded public HTTP(S) URL safe to expose as a browser link."""
    if not isinstance(raw_url, str):
        return None
    candidate = raw_url.strip()
    if not candidate or len(candidate) > max_chars:
        return None
    if any(char.isspace() or ord(char) < 0x20 or ord(char) == 0x7F for char in candidate):
        return None
    try:
        parsed = urlsplit(candidate)
        hostname = (parsed.hostname or "").rstrip(".").casefold()
        # Accessing port validates malformed and out-of-range port values.
        parsed.port
    except ValueError:
        return None
    if parsed.scheme.casefold() not in {"http", "https"} or not hostname:
        return None
    if parsed.username is not None or parsed.password is not None:
        return None
    try:
        address = ipaddress.ip_address(hostname.strip("[]"))
    except ValueError:
        address = None
    if address is not None and not address.is_global:
        return None
    if hostname == "localhost" or hostname.endswith(
        (".localhost", ".local", ".internal", ".home", ".lan")
    ):
        return None
    return candidate


def _validated_article_url(raw_url: object, allowed_domains: tuple[str, ...]) -> str | None:
    """Return a bounded URL belonging to the configured RSS publisher."""
    candidate = _validated_public_http_url(raw_url)
    if candidate is None:
        return None
    hostname = (urlsplit(candidate).hostname or "").rstrip(".").casefold()
    if not any(
        hostname == domain or hostname.endswith(f".{domain}")
        for domain in allowed_domains
    ):
        return None
    return candidate


def _validated_feed_entry(entry: object, feed_info: dict) -> dict | None:
    raw_title = getattr(entry, "title", None)
    if not isinstance(raw_title, str):
        return None
    title = raw_title.strip()
    if not title or len(title) > NEWS_TITLE_MAX_CHARS:
        return None
    if any(ord(char) < 0x20 or ord(char) == 0x7F for char in title):
        return None
    link = _validated_article_url(
        getattr(entry, "link", None),
        tuple(feed_info["article_domains"]),
    )
    if link is None:
        return None
    return {
        "title": title,
        "url": link,
        "source": feed_info["name"],
        "source_url": feed_info["url"],
        "published_at": _parse_published(entry),
    }


async def fetch_all_feeds() -> list[dict]:
    """Fetch all RSS feeds and return raw parsed items."""
    items: list[dict] = []
    semaphore = asyncio.Semaphore(RSS_MAX_CONCURRENT)

    async with httpx.AsyncClient(timeout=httpx.Timeout(RSS_FETCH_TIMEOUT_SECONDS)) as client:
        async def fetch_one(feed_info: dict) -> list[dict]:
            async with semaphore:
                try:
                    async with client.stream("GET", feed_info["url"]) as response:
                        response.raise_for_status()
                        body = await _read_feed_body(response)
                except httpx.HTTPError:
                    logger.warning("news_feed.fetch_error", feed=feed_info["name"], error_type="network_error")
                    return []
                try:
                    if body is None:
                        logger.warning("news_feed.fetch_error", feed=feed_info["name"], error_type="body_too_large")
                        return []
                    parsed = feedparser.parse(io.BytesIO(body))
                    feed_items = []
                    for entry in _bounded_feed_entries(parsed.entries):
                        item = _validated_feed_entry(entry, feed_info)
                        if item is not None:
                            feed_items.append(item)
                    return feed_items
                except (AttributeError, TypeError, ValueError):
                    logger.warning("news_feed.fetch_error", feed=feed_info["name"], error_type="parse_error")
                    return []

        fetched = await asyncio.gather(*(fetch_one(feed) for feed in RSS_FEEDS))
        for feed_items in fetched:
            remaining = RSS_MAX_ENTRIES_PER_RUN - len(items)
            if remaining <= 0:
                break
            items.extend(feed_items[:remaining])

    return items


def _bounded_plain_text(
    raw_value: object, max_chars: int, *, allow_line_breaks: bool = False
) -> str | None:
    if not isinstance(raw_value, str):
        return None
    if any(
        (ord(char) < 0x20 and not (allow_line_breaks and char in "\t\r\n"))
        or ord(char) == 0x7F
        for char in raw_value
    ):
        return None
    value = " ".join(raw_value.split())
    if not value or len(value) > max_chars:
        return None
    return value


def _parse_webz_published(raw_value: object) -> datetime:
    if isinstance(raw_value, (int, float)) and raw_value >= 0:
        try:
            timestamp = raw_value / 1000 if raw_value > 10_000_000_000 else raw_value
            return datetime.fromtimestamp(timestamp, tz=UTC)
        except (OSError, OverflowError, ValueError):
            return datetime.now(UTC)
    if isinstance(raw_value, str):
        try:
            parsed = datetime.fromisoformat(raw_value.strip().replace("Z", "+00:00"))
            if parsed.tzinfo is not None:
                return parsed.astimezone(UTC)
        except ValueError:
            pass
    return datetime.now(UTC)


def _validated_webz_post(post: object) -> dict | None:
    if not isinstance(post, dict):
        return None
    thread = post.get("thread")
    if not isinstance(thread, dict):
        thread = {}

    title = _bounded_plain_text(
        post.get("title") or thread.get("title"), NEWS_TITLE_MAX_CHARS
    )
    article_url = _validated_public_http_url(post.get("url") or thread.get("url"))
    if title is None or article_url is None:
        return None

    parsed_url = urlsplit(article_url)
    source = _bounded_plain_text(
        thread.get("site") or thread.get("site_full") or parsed_url.hostname,
        NEWS_SOURCE_MAX_CHARS,
    )
    if source is None:
        return None
    source_url = urlunsplit((parsed_url.scheme, parsed_url.netloc, "", "", ""))

    return {
        "title": title,
        "url": article_url,
        "source": source,
        "source_url": source_url,
        "published_at": _parse_webz_published(
            post.get("published") or thread.get("published")
        ),
        "summary": _bounded_plain_text(
            post.get("summary"), NEWS_SUMMARY_MAX_CHARS, allow_line_breaks=True
        ),
    }


async def fetch_webz_items() -> list[dict] | None:
    """Fetch and normalize one bounded Webz.io page.

    None means the provider request failed. An empty list is a valid response
    with no usable records. Neither state exposes provider details or secrets.
    """
    if settings.WEBZ_API_TOKEN is None:
        return None

    params = {
        "token": settings.WEBZ_API_TOKEN.get_secret_value(),
        "format": "json",
        "q": WEBZ_QUERY,
        "sort": "published",
        "order": "desc",
        "size": WEBZ_MAX_ENTRIES_PER_RUN,
        "includeSyndicated": "false",
    }
    try:
        async with httpx.AsyncClient(
            timeout=httpx.Timeout(settings.WEBZ_API_TIMEOUT_SECONDS)
        ) as client:
            async with client.stream(
                "GET", settings.WEBZ_API_URL, params=params
            ) as response:
                response.raise_for_status()
                body = await _read_bounded_body(
                    response, max_bytes=WEBZ_MAX_RESPONSE_BYTES
                )
        if body is None:
            logger.warning("news_feed.webz_unavailable", reason="body_too_large")
            return None
        payload = json.loads(body)
    except httpx.HTTPError:
        logger.warning("news_feed.webz_unavailable", reason="http_error")
        return None
    except (TypeError, ValueError):
        logger.warning("news_feed.webz_unavailable", reason="invalid_response")
        return None

    if not isinstance(payload, dict) or not isinstance(payload.get("posts"), list):
        logger.warning("news_feed.webz_unavailable", reason="invalid_response")
        return None

    posts = payload["posts"][:WEBZ_MAX_ENTRIES_PER_RUN]
    items = [item for post in posts if (item := _validated_webz_post(post)) is not None]
    logger.info(
        "news_feed.webz_fetched",
        fetched=len(posts),
        accepted=len(items),
        rejected=len(posts) - len(items),
    )
    return items


async def fetch_news_items() -> list[dict]:
    """Use licensed Webz.io when configured, with RSS as a safe fallback."""
    if settings.WEBZ_API_TOKEN is not None:
        webz_items = await fetch_webz_items()
        if webz_items:
            return webz_items
        logger.info(
            "news_feed.fallback",
            from_provider="webz",
            reason="empty" if webz_items == [] else "unavailable",
        )
    else:
        logger.info("news_feed.webz_unconfigured")
    return await fetch_all_feeds()


async def _read_bounded_body(
    response: httpx.Response, *, max_bytes: int
) -> bytes | None:
    """Read a decoded response incrementally without crossing a hard cap."""
    chunks: list[bytes] = []
    total = 0
    async for chunk in response.aiter_bytes(chunk_size=RSS_READ_CHUNK_BYTES):
        total += len(chunk)
        if total > max_bytes:
            return None
        chunks.append(chunk)
    return b"".join(chunks)


async def _read_feed_body(response: httpx.Response) -> bytes | None:
    return await _read_bounded_body(response, max_bytes=RSS_MAX_BYTES_PER_FEED)


def _bounded_feed_entries(entries) -> list:
    return list(entries[:RSS_MAX_ENTRIES_PER_FEED])


def _extract_json(text: str) -> Optional[dict]:
    """Try to extract a JSON object from Gemini response text."""
    text = text.strip()
    if text.startswith("```"):
        text = re.sub(r"^```(?:json)?\s*", "", text)
        text = re.sub(r"\s*```$", "", text)
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        match = re.search(r"\{[^}]+\}", text, re.DOTALL)
        if match:
            try:
                return json.loads(match.group())
            except json.JSONDecodeError:
                return None
    return None


def _dedupe_fetched_items(raw_items: list[dict]) -> list[dict]:
    """Collapse duplicate URLs within a single fetch batch before categorization."""
    unique_items: list[dict] = []
    seen_urls: set[str] = set()
    for item in raw_items:
        url = item["url"]
        if url in seen_urls:
            continue
        seen_urls.add(url)
        unique_items.append(item)
        if len(unique_items) >= RSS_MAX_ENTRIES_PER_RUN:
            break
    return unique_items


def _categorize_headline_sync(title: str) -> str:
    model = get_gemini_model(GEMINI_MODEL)
    prompt = (
        "Categorize this shipping/maritime headline into exactly ONE category. "
        "Pick the single best category from this list: shipping, bunkers, regulation, carbon, commodities, markets. "
        "Return ONLY a JSON object with these three fields:\n"
        "- category: one of shipping, bunkers, regulation, carbon, commodities, markets\n"
        "- relevance: integer 1 to 5 where 5 means highly relevant to marine fuel trading\n"
        "- summary: a one-line summary if the title is unclear, otherwise null\n"
        "Do NOT wrap in markdown. Do NOT include explanation.\n"
        f"Headline: {title}"
    )
    response = model.generate_content(
        prompt,
        request_options=request_options(NEWS_PROVIDER_TIMEOUT_SECONDS),
    )
    return response.text


async def categorize_headline(title: str) -> dict:
    """Call Gemini Flash Lite to categorize a shipping headline.

    Returns dict with keys: category, relevance, summary.
    Falls back to defaults on any failure.
    """
    state = _refresh_provider_state.get()
    if not settings.GEMINI_API_KEY:
        if state is not None:
            state.note("configuration", level="info")
        return _local_categorize_headline(title)
    if state is not None and not state.provider_enabled:
        return _local_categorize_headline(title)
    if state is not None:
        if state.provider_calls_remaining <= 0:
            state.note("run_call_cap", level="info")
            return _local_categorize_headline(title)
        state.provider_calls_remaining -= 1
    if not _gemini_breaker.allow():
        return _local_categorize_headline(title)

    try:
        # Do not wrap this in asyncio.wait_for: cancellation would abandon a
        # worker thread while releasing the capacity slot. RequestOptions
        # supplies the provider-native deadline and the slot remains held
        # until the call actually returns.
        raw_text = await run_provider_call(
            _news_provider_capacity,
            _categorize_headline_sync,
            title,
            busy_error=RuntimeError("news provider is busy"),
        )
        _gemini_breaker.success()
        result = _extract_json(raw_text)
        if not result:
            return _local_categorize_headline(title)

        category = result.get("category", "markets")
        if category not in VALID_CATEGORIES:
            category = "markets"

        relevance = result.get("relevance", 3)
        if not isinstance(relevance, int) or relevance < 1 or relevance > 5:
            relevance = 3

        summary = result.get("summary")
        if summary and not isinstance(summary, str):
            summary = None

        return {
            "category": category,
            "relevance": relevance,
            "summary": summary,
        }
    except Exception as exc:
        _gemini_breaker.failure()
        if state is not None:
            if _is_provider_auth_failure(exc):
                state.disable("auth_or_permission")
            else:
                state.note("provider_error")
        else:
            logger.warning("news_feed.provider_unavailable", reason="provider_error")
        return _local_categorize_headline(title)


class NewsRefreshInProgress(RuntimeError):
    """Another database-coordinated refresh already holds the transaction lock."""


async def refresh_news(db: AsyncSession) -> int:
    """Fetch feeds, deduplicate, categorize via Gemini, and insert new items.

    Returns the count of newly inserted items.
    """
    state = _RefreshProviderState(provider_enabled=bool(settings.GEMINI_API_KEY))
    token = _refresh_provider_state.set(state)
    try:
        bind = db.get_bind()
        dialect_name = bind.dialect.name if bind is not None else ""
        if dialect_name == "postgresql":
            acquired = await db.scalar(
                text("SELECT pg_try_advisory_xact_lock(:lock_id)"),
                {"lock_id": NEWS_REFRESH_ADVISORY_LOCK_ID},
            )
            if not acquired:
                raise NewsRefreshInProgress("A news refresh is already running")

        raw_items = await fetch_news_items()
        if not raw_items:
            logger.info("news_feed.no_items_fetched")
            return 0

        raw_items = _dedupe_fetched_items(raw_items)

        # Get existing URLs from DB for deduplication.
        urls = [item["url"] for item in raw_items]
        result = await db.execute(
            select(NewsItem.url).where(NewsItem.url.in_(urls))
        )
        existing_urls = set(result.scalars().all())

        new_items = [item for item in raw_items if item["url"] not in existing_urls]
        if not new_items:
            logger.info("news_feed.all_duplicates", total=len(raw_items))
            return 0

        logger.info(
            "news_feed.categorizing",
            new_count=len(new_items),
            total_fetched=len(raw_items),
            provider_enabled=state.provider_enabled,
        )

        rows_to_insert = []
        for item in new_items:
            cat_result = await categorize_headline(item["title"])
            rows_to_insert.append(
                {
                    "title": item["title"],
                    "url": item["url"],
                    "source": item["source"],
                    "source_url": item["source_url"],
                    "published_at": item["published_at"],
                    "category": cat_result["category"],
                    "relevance": cat_result["relevance"],
                    "summary": cat_result["summary"] or item.get("summary"),
                    "fetched_at": datetime.now(UTC),
                }
            )

        if dialect_name == "postgresql":
            stmt = postgresql_insert(NewsItem).values(rows_to_insert)
            stmt = stmt.on_conflict_do_nothing(index_elements=[NewsItem.url])
        elif dialect_name == "sqlite":
            stmt = sqlite_insert(NewsItem).values(rows_to_insert)
            stmt = stmt.on_conflict_do_nothing(index_elements=[NewsItem.url])
        else:
            for row in rows_to_insert:
                db.add(NewsItem(**row))
            await db.flush()
            inserted = len(rows_to_insert)
            logger.info("news_feed.refresh_complete", inserted=inserted)
            return inserted

        result = await db.execute(stmt)
        inserted = max(result.rowcount or 0, 0)
        logger.info("news_feed.refresh_complete", inserted=inserted)
        return inserted
    finally:
        _refresh_provider_state.reset(token)
