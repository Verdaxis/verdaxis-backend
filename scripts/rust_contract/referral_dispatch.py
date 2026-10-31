"""Referral has no outbox effect: the real dispatcher must remain idle."""
import asyncio
import json
import os

from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

from app.services.market_event_dispatch import MarketEventDispatcher


async def main():
    if os.environ.get('ENVIRONMENT')!='test': raise RuntimeError('test only')
    engine=create_async_engine(os.environ['PHASE2_DATABASE_URL'].replace('postgresql://','postgresql+asyncpg://'))
    delivered=[]
    class Bus:
        async def publish(self,*args,**kwargs): delivered.append(args)
    dispatcher=MarketEventDispatcher(engine,Bus(),poll_seconds=.05)
    try:
        await dispatcher.start()
        async with engine.connect() as connection:
            count=await connection.scalar(text('SELECT count(*) FROM market_event_outbox'))
        assert count==0 and not delivered
        return {'outbox':count,'delivered':len(delivered)}
    finally:
        await dispatcher.stop()
        await engine.dispose()


if __name__=='__main__': print(json.dumps(asyncio.run(main())))
