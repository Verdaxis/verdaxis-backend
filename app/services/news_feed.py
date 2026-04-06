import feedparser
import google.generativeai as genai
import asyncio
import json
import re
from datetime import datetime, UTC
from time import mktime
from typing import Optional

import structlog
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.config import settings
from app.models.news import NewsItem

logger = structlog.get_logger()

GEMINI_MODEL = "gemini-2.5-flash-lite"

RSS_FEEDS = [
    {"name": "TradeWinds", "url": "https://www.tradewindsnews.com/rss"},
    {"name": "Ship & Bunker", "url": "https://shipandbunker.com/rss"},
    {"name": "Splash247", "url": "https://splash247.com/feed/"},
    {"name": "Maritime Executive", "url": "https://maritime-executive.com/rss"},
    {
        "name": "Hellenic Shipping News",
        "url": "https://www.hellenicshippingnews.com/feed/",
    },
]

VALID_CATEGORIES = {"shipping", "bunkers", "regulation", "carbon", "commodities", "markets"}


def _parse_published(entry: dict) -> datetime:
    """Extract published datetime from a feed entry, falling back to now."""
    if hasattr(entry, "published_parsed") and entry.published_parsed:
        return datetime.fromtimestamp(mktime(entry.published_parsed), tz=UTC)
    if hasattr(entry, "updated_parsed") and entry.updated_parsed:
        return datetime.fromtimestamp(mktime(entry.updated_parsed), tz=UTC)
    return datetime.now(UTC)


async def fetch_all_feeds() -> list[dict]:
    """Fetch all RSS feeds and return raw parsed items."""
    items = []
    loop = asyncio.get_event_loop()

    for feed_info in RSS_FEEDS:
        try:
            parsed = await loop.run_in_executor(
                None, feedparser.parse, feed_info["url"]
            )
            for entry in parsed.entries:
                link = getattr(entry, "link", None)
                title = getattr(entry, "title", None)
                if not link or not title:
                    continue
                items.append(
                    {
                        "title": title.strip(),
                        "url": link.strip(),
                        "source": feed_info["name"],
                        "source_url": feed_info["url"],
                        "published_at": _parse_published(entry),
                    }
                )
        except Exception:
            logger.warning("news_feed.fetch_error", feed=feed_info["name"], exc_info=True)
            continue

    return items


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


async def categorize_headline(title: str) -> dict:
    """Call Gemini Flash Lite to categorize a shipping headline.

    Returns dict with keys: category, relevance, summary.
    Falls back to defaults on any failure.
    """
    defaults = {"category": "markets", "relevance": 3, "summary": None}
    if not settings.GEMINI_API_KEY:
        logger.warning("news_feed.gemini_skipped", reason="GEMINI_API_KEY not configured")
        return defaults

    try:
        genai.configure(api_key=settings.GEMINI_API_KEY)
        model = genai.GenerativeModel(GEMINI_MODEL)
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
        response = await asyncio.get_event_loop().run_in_executor(
            None, model.generate_content, prompt
        )
        result = _extract_json(response.text)
        if not result:
            return defaults

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
    except Exception:
        logger.warning("news_feed.categorize_error", title=title[:80], exc_info=True)
        return defaults


async def refresh_news(db: AsyncSession) -> int:
    """Fetch feeds, deduplicate, categorize via Gemini, and insert new items.

    Returns the count of newly inserted items.
    """
    raw_items = await fetch_all_feeds()
    if not raw_items:
        logger.info("news_feed.no_items_fetched")
        return 0

    # Get existing URLs from DB for deduplication
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
    )

    inserted = 0
    for item in new_items:
        cat_result = await categorize_headline(item["title"])
        news_item = NewsItem(
            title=item["title"],
            url=item["url"],
            source=item["source"],
            source_url=item["source_url"],
            published_at=item["published_at"],
            category=cat_result["category"],
            relevance=cat_result["relevance"],
            summary=cat_result["summary"],
            fetched_at=datetime.now(UTC),
        )
        db.add(news_item)
        inserted += 1

    await db.commit()
    logger.info("news_feed.refresh_complete", inserted=inserted)
    return inserted
