"""Task 8 test-only postcommit response-loss seam around the unchanged app."""
from tests.rust_contract.server import ContractOracleApp

original = ContractOracleApp.__call__


async def with_response_loss(self, scope, receive, send):
    fault = dict(scope.get("headers", [])).get(b"x-notification-fault")
    if fault != b"after-commit":
        return await original(self, scope, receive, send)
    messages = []

    async def capture(message):
        messages.append(message)

    await original(self, scope, receive, capture)
    start = next(message for message in messages if message["type"] == "http.response.start")
    if start["status"] == 200:
        headers = [(name, value) for name, value in start["headers"]
                   if name.lower() not in (b"content-type", b"content-length")]
        headers.append((b"content-type", b"application/json"))
        await send({"type": "http.response.start", "status": 503, "headers": headers})
        await send({"type": "http.response.body", "body": b'{"detail":"Injected response loss"}'})
    else:
        for message in messages:
            await send(message)


if __name__ == "__main__":
    ContractOracleApp.__call__ = with_response_loss
    from scripts.rust_contract.phase2_oracle import main
    raise SystemExit(main())
