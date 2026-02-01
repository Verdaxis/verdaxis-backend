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

async def check_buyers():
    async with AsyncSessionLocal() as session:
        print("Checking for ANY buyer account...")
        result = await session.execute(text("SELECT email, status, role FROM users WHERE role = 'BUYER' LIMIT 1"))
        user = result.fetchone()
        
        if user:
            print(f"Found buyer: {user.email}, Status: {user.status}")
            if user.status != 'APPROVED':
                print(f"Updating status for {user.email} to APPROVED...")
                await session.execute(text(f"UPDATE users SET status = 'APPROVED' WHERE email = '{user.email}'"))
                await session.commit()
                print("Update committed.")
        else:
            print("No buyer account found!")

asyncio.run(check_buyers())
