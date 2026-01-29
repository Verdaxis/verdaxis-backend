import asyncio
from sqlalchemy import text
from app.database import AsyncSessionLocal

async def migrate():
    async with AsyncSessionLocal() as db:
        try:
            await db.execute(text("ALTER TABLE vessels ADD COLUMN IF NOT EXISTS previous_location geography(POINT, 4326)"))
            await db.commit()
            print("Migration successful: Added previous_location column.")
        except Exception as e:
            print(f"Migration failed (it might already exist): {e}")

if __name__ == "__main__":
    asyncio.run(migrate())
