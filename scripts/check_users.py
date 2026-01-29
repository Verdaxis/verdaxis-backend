import asyncio
from app.database import AsyncSessionLocal
from app.models.user import User
from sqlalchemy import select

async def main():
    async with AsyncSessionLocal() as db:
        result = await db.execute(select(User))
        users = result.scalars().all()
        print(f"Found {len(users)} users:")
        for u in users:
            print(f" - {u.email} (Role: {u.role})")

if __name__ == "__main__":
    asyncio.run(main())
