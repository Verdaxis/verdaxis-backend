"""Disposable Python account writer with an explicit stdin release barrier."""
import asyncio
import os
import sys
from urllib.parse import urlsplit

import asyncpg


async def main():
    url = os.environ["AUTH_INTEROP_ADMIN_URL"]
    if os.environ.get("ENVIRONMENT") != "test" or not urlsplit(url).path.endswith("_test"):
        raise RuntimeError("disposable account writer only")
    connection = await asyncpg.connect(url)
    try:
        async with connection.transaction():
            from uuid import UUID
            if os.environ.get("AUTH_INTEROP_EXPECT_BLOCKED") == "true":
                await connection.execute("SET LOCAL lock_timeout='100ms'")
            await connection.execute("UPDATE users SET authentication_revoked_at='2026-10-19T00:00:00Z' WHERE id=$1", UUID(os.environ["AUTH_INTEROP_SUBJECT"]))
            print("LOCKED", flush=True)
            if await asyncio.to_thread(sys.stdin.readline) != "COMMIT\n":
                raise RuntimeError("missing release barrier")
        print("COMMITTED", flush=True)
    except asyncpg.LockNotAvailableError:
        if os.environ.get("AUTH_INTEROP_EXPECT_BLOCKED") != "true":
            raise
        print("BLOCKED", flush=True)
    finally:
        await connection.close()


if __name__ == "__main__":
    asyncio.run(main())
