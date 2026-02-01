import asyncio
from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession
from sqlalchemy.orm import sessionmaker
from sqlalchemy import text
import sys
import os

sys.path.append(os.getcwd())

DB_USER = "jonathanjie"
DB_PASS = ""
DB_HOST = "localhost"
DB_PORT = "5432"
DB_NAME = "verdaxis"

DATABASE_URL = f"postgresql+asyncpg://{DB_USER}:{DB_PASS}@{DB_HOST}:{DB_PORT}/{DB_NAME}"

engine = create_async_engine(DATABASE_URL, echo=True)
AsyncSessionLocal = sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)

async def reset_password():
    async with AsyncSessionLocal() as session:
        # Get hash from seller_local_1 (known password: password123)
        print("Getting hash from seller_local_1...")
        result = await session.execute(text("SELECT password_hash FROM users WHERE email = 'seller_local_1@test.com'"))
        seller = result.fetchone()
        
        if not seller:
            print("Seller not found! Cannot copy hash.")
            return

        known_hash = seller.password_hash
        print(f"Got hash: {known_hash[:10]}...")
        
        # Update buyer1
        print("Updating buyer1@verdaxis.com password hash...")
        await session.execute(text(f"UPDATE users SET password_hash = '{known_hash}' WHERE email = 'buyer1@verdaxis.com'"))
        await session.commit()
        print("Password reset committed.")

asyncio.run(reset_password())
