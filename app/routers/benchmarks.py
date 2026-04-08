from datetime import UTC, datetime
from typing import Optional
from uuid import UUID

from fastapi import APIRouter, Depends, Query
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.schemas.benchmark import BenchmarkQuoteResponse
from app.services.benchmarks import get_benchmark_quote

router = APIRouter(prefix="/benchmarks", tags=["benchmarks"])


@router.get("", response_model=BenchmarkQuoteResponse)
async def list_benchmarks(
    market_product: Optional[str] = Query(None),
    delivery_point_id: Optional[UUID] = Query(None),
    availability_window: str = Query("SPOT"),
    db: AsyncSession = Depends(get_db),
):
    items = []
    if market_product and delivery_point_id:
        quote = await get_benchmark_quote(
            db,
            market_product=market_product,
            delivery_point_id=delivery_point_id,
            availability_window=availability_window,
        )
        if quote is not None:
            items.append(quote)

    return BenchmarkQuoteResponse(
        items=items,
        generated_at=datetime.now(UTC),
    )
