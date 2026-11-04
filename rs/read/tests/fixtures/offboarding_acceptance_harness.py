"""Cross-stack harness for the integrated offboarding acceptance tests.

``offboarding_acceptance_postgres.rs`` starts this script to send requests
through the unchanged Python application, in process, against the same
disposable database. It refuses to run unless ``ENVIRONMENT=test``.

Usage: ``python offboarding_acceptance_harness.py requests``. The command
reads one line of JSON on stdin, ``{"requests": [...]}``, loads the
application, prints ``ACCEPTANCE_READY`` and waits for one more line on stdin.
Then it sends the requests in order and prints one line
``ACCEPTANCE_RESULT <json>``. The pause lets a test start the slow interpreter
before a race and release the requests at an exact moment. Each request has a
``name``, ``method`` and ``path`` and may have:

- ``token``: a bearer access token;
- ``json``: a JSON body, or ``form``: a URL-encoded form body;
- ``cookies``: cookies to present, by name;
- ``headers``: extra request headers.

Each result has the status, the decoded body and the cookies the response
set, by name.
"""

import asyncio
import json
import os
import sys
from http.cookies import SimpleCookie

RESULT_PREFIX = "ACCEPTANCE_RESULT "
READY_LINE = "ACCEPTANCE_READY"


def _require_disposable() -> None:
    if os.environ.get("ENVIRONMENT") != "test":
        raise SystemExit("the acceptance harness runs only with ENVIRONMENT=test")


def _set_cookies(response) -> dict:
    cookies = {}
    for header in response.headers.get_list("set-cookie"):
        parsed = SimpleCookie()
        parsed.load(header)
        for name, morsel in parsed.items():
            cookies[name] = morsel.value
    return cookies


async def send_requests(app, request: dict) -> dict:
    import httpx

    results = {}
    # The ASGI transport serves the application without its lifespan, which
    # loads deployed keys and starts background dispatchers these checks do
    # not need.
    transport = httpx.ASGITransport(app=app, client=("127.0.0.1", 50000))
    async with httpx.AsyncClient(transport=transport, base_url="http://localhost") as client:
        for item in request["requests"]:
            headers = dict(item.get("headers") or {})
            if item.get("token"):
                headers["Authorization"] = f"Bearer {item['token']}"
            cookies = item.get("cookies") or {}
            if cookies:
                headers["Cookie"] = "; ".join(f"{name}={value}" for name, value in cookies.items())
            response = await client.request(
                item["method"],
                item["path"],
                headers=headers,
                json=item.get("json"),
                data=item.get("form"),
            )
            try:
                body = response.json()
            except ValueError:
                body = response.text
            results[item["name"]] = {
                "status": response.status_code,
                "body": body,
                "cookies": _set_cookies(response),
            }
    return {"results": results}


def main() -> None:
    _require_disposable()
    if sys.argv[1:] != ["requests"]:
        raise SystemExit("usage: offboarding_acceptance_harness.py requests")
    request = json.loads(sys.stdin.readline())
    from app.main import app

    sys.stdout.write(READY_LINE + "\n")
    sys.stdout.flush()
    sys.stdin.readline()
    result = asyncio.run(send_requests(app, request))
    sys.stdout.write(RESULT_PREFIX + json.dumps(result, sort_keys=True, default=str) + "\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
