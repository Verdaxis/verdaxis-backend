from fastapi import APIRouter, Depends, Query
from sqlalchemy import select, desc
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.news import NewsItem
from app.services.news_feed import refresh_news

router = APIRouter(prefix="/news", tags=["news"])


@router.get("")
async def list_news(
    limit: int = Query(default=20, ge=1, le=100),
    category: str | None = Query(default=None),
    min_relevance: int = Query(default=1, ge=1, le=5),
    db: AsyncSession = Depends(get_db),
):
    """Public endpoint - returns latest news items sorted by published_at desc."""
    stmt = select(NewsItem).where(NewsItem.relevance >= min_relevance)
    if category:
        stmt = stmt.where(NewsItem.category == category)
    stmt = stmt.order_by(desc(NewsItem.published_at)).limit(limit)

    result = await db.execute(stmt)
    items = result.scalars().all()

    return [
        {
            "id": str(item.id),
            "title": item.title,
            "summary": item.summary,
            "source": item.source,
            "source_url": item.source_url,
            "url": item.url,
            "category": item.category,
            "relevance": item.relevance,
            "published_at": item.published_at.isoformat() if item.published_at else None,
            "fetched_at": item.fetched_at.isoformat() if item.fetched_at else None,
        }
        for item in items
    ]


@router.post("/refresh")
async def trigger_refresh(
    db: AsyncSession = Depends(get_db),
):
    """Manually trigger a news feed refresh. Returns count of new items inserted."""
    count = await refresh_news(db)
    return {"inserted": count}
