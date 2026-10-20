"""Third-cell interoperability with the existing Python dispatcher owner."""
import asyncio
import json
import os

from sqlalchemy.ext.asyncio import create_async_engine

from app.services.market_event_dispatch import MarketEventDispatcher


async def check(url):
    if os.environ.get("ENVIRONMENT") != "test":
        raise RuntimeError("disposable dispatcher check only")
    engine = create_async_engine(url.replace("postgresql://", "postgresql+asyncpg://"))
    delivered = []
    class Bus:
        async def publish(self, channel, event_type, payload, **kwargs):
            delivered.append((channel, event_type, kwargs.get("seq")))
    dispatcher = MarketEventDispatcher(engine, Bus(), poll_seconds=0.05)
    try:
        await dispatcher.start()
        for _ in range(200):
            if len(delivered) == 2:
                break
            await asyncio.sleep(.05)
        if len(delivered) != 2 or len({row[2] for row in delivered}) != 2:
            raise RuntimeError("dispatcher did not publish each mixed writer event once")
        return {"delivered": len(delivered), "sequences": [row[2] for row in delivered]}
    finally:
        await dispatcher.stop()
        await engine.dispose()


if __name__ == "__main__":
    print(json.dumps(asyncio.run(check(os.environ["PHASE2_DATABASE_URL"]))))
