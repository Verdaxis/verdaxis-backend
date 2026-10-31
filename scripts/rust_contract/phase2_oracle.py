"""Test-only Python entry point: fail closed on non-loopback provider traffic."""
import os
from urllib.parse import urlsplit


def main():
    target = os.environ.get("PHASE2_MAIL_URL", "")
    parsed = urlsplit(target)
    if (os.environ.get("ENVIRONMENT") != "test" or not os.environ.get("DISPOSABLE_TEST_TOKEN")
            or parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or not parsed.port):
        raise SystemExit("disposable loopback mail provider is required")
    import httpx

    from app.services import email
    from tests.rust_contract.server import main as serve

    email.RESEND_API_URL = target
    original = httpx.AsyncClient.send

    async def loopback_only(client, request, *args, **kwargs):
        if request.url.host != "127.0.0.1":
            raise RuntimeError("external HTTP is disabled in route scenarios")
        return await original(client, request, *args, **kwargs)

    httpx.AsyncClient.send = loopback_only
    return serve()


if __name__ == "__main__":
    raise SystemExit(main())
