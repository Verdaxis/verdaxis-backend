"""Cross-stack harness for the responsible-person reassignment tests.

``offboarding_reassignment_postgres.rs`` starts this script to send trade
settlement requests through the unchanged Python application, in process,
against the same disposable database. It refuses to run unless
``ENVIRONMENT=test``.

Usage: ``python offboarding_reassignment_harness.py settle``. The command
reads one JSON object on stdin, ``{"requests": [{"name", "method", "path",
"token", "body"}]}``, sends the requests in order and prints one line
``REASSIGNMENT_RESULT <json>`` with each request's status and detail.
"""

import asyncio
import json
import os
import sys

RESULT_PREFIX = "REASSIGNMENT_RESULT "


def _require_disposable() -> None:
    if os.environ.get("ENVIRONMENT") != "test":
        raise SystemExit("the reassignment harness runs only with ENVIRONMENT=test")


async def settle(request: dict) -> dict:
    import httpx
    from app.main import app

    results = {}
    # The ASGI transport serves the application without its lifespan, which
    # loads deployed keys and starts background dispatchers this check does
    # not need.
    transport = httpx.ASGITransport(app=app, client=("127.0.0.1", 50000))
    async with httpx.AsyncClient(transport=transport, base_url="http://localhost") as client:
        for item in request["requests"]:
            response = await client.request(
                item["method"],
                item["path"],
                headers={"Authorization": f"Bearer {item['token']}"},
                json=item.get("body"),
            )
            try:
                detail = response.json()
            except ValueError:
                detail = response.text
            results[item["name"]] = {"status": response.status_code, "body": detail}
    return {"results": results}


def main() -> None:
    _require_disposable()
    if sys.argv[1:] != ["settle"]:
        raise SystemExit("usage: offboarding_reassignment_harness.py settle")
    request = json.loads(sys.stdin.read())
    result = asyncio.run(settle(request))
    sys.stdout.write(RESULT_PREFIX + json.dumps(result, sort_keys=True, default=str) + "\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
