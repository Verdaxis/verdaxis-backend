"""Real Python alert producer and dispatcher against the runner's mixed cell."""
import asyncio
import json
import os
from decimal import Decimal
from uuid import UUID

from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.services.activity import check_price_alerts
from app.services.market_event_dispatch import MarketEventDispatcher
from app.services.market_events import commit_market_events
from scripts.rust_contract.phase2_alerts import POINT, PRODUCT


async def main():
    assert os.environ["ENVIRONMENT"] == "test"
    engine = create_async_engine(os.environ["PHASE2_DATABASE_URL"].replace("postgresql://", "postgresql+asyncpg://"))
    delivered = []
    class Bus:
        async def publish(self, channel, event_type, payload, **kwargs):
            assert event_type == "price_alert_triggered"
            assert payload["direction"] == "above" and payload["triggered_price"] == "100"
            delivered.append((channel, payload, kwargs.get("seq")))
    dispatcher = MarketEventDispatcher(engine, Bus(), poll_seconds=.05)
    try:
        async with async_sessionmaker(engine, expire_on_commit=False)() as session:
            events = await check_price_alerts(session, UUID(PRODUCT), UUID(POINT), Decimal(100))
            assert len(events) == 5
            assert any(event.payload["alert_id"].endswith("000000000041") and event.payload["threshold_usd"] == "12.35" for event in events)
            await commit_market_events(session, events)
        await dispatcher.start()
        for _ in range(200):
            if len(delivered) == 5:
                break
            await asyncio.sleep(.05)
        assert len(delivered) == 5 and len({entry[2] for entry in delivered}) == 5
        return {"delivered": len(delivered), "sequences": [entry[2] for entry in delivered]}
    finally:
        await dispatcher.stop()
        await engine.dispose()


if __name__ == "__main__":
    print(json.dumps(asyncio.run(main())))
