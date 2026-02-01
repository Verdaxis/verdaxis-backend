import asyncio
from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession
from sqlalchemy.orm import sessionmaker
from sqlalchemy import text
import sys
import os

# Add local path to sys.path to ensure we can import from app
sys.path.append(os.getcwd())

# Configuration from .env (or hardcoded backup based on observations)
DB_USER = "jonathanjie"
DB_PASS = "" # Empty in .env
DB_HOST = "localhost"
DB_PORT = "5432"
DB_NAME = "verdaxis"

DATABASE_URL = f"postgresql+asyncpg://{DB_USER}:{DB_PASS}@{DB_HOST}:{DB_PORT}/{DB_NAME}"
print(f"Connecting to {DATABASE_URL}")

engine = create_async_engine(DATABASE_URL, echo=True)
AsyncSessionLocal = sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)

async def fix_user():
    async with AsyncSessionLocal() as session:
        print("Checking for seller_local_1@test.com...")
        result = await session.execute(text("SELECT email, status, role FROM users WHERE email = 'seller_local_1@test.com'"))
        user = result.fetchone()
        
        if user:
            print(f"Found user: {user.email}, Status: {user.status}, Role: {user.role}")
            if user.status != 'APPROVED':
                print("Updating status to APPROVED...")
                await session.execute(text("UPDATE users SET status = 'APPROVED' WHERE email = 'seller_local_1@test.com'"))
                await session.commit()
                print("Update committed.")
            else:
                print("User is already APPROVED.")
        else:
            print("User not found!")

asyncio.run(fix_user())
