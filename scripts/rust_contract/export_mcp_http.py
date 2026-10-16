"""Record bounded SDK header precedence and modern success with no database."""
import asyncio
import json
from pathlib import Path

import anyio
import httpx

from app.mcp_server import build_mcp_http_app, build_mcp_server

ROOT = Path(__file__).resolve().parents[2]


async def vectors():
    server = build_mcp_server(read_limiter=anyio.CapacityLimiter(2))
    app = build_mcp_http_app(server)
    async with server.session_manager.run():
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url="http://testserver") as client:
            body = {"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_markets","arguments":{}}}
            modern = {**body,"params":{**body["params"],"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}
            for name, headers, raw in [
                ("origin-first", {"origin":"","host":"bad","content-type":"text/plain","accept":"text/plain"}, "{"),
                ("content-before-host", {"host":"bad","content-type":"text/plain","accept":"text/plain"}, "{"),
                ("accept-before-json", {"content-type":"application/json","accept":"text/plain"}, "{"),
                ("inner-content-type", {"content-type":"APPLICATION/JSON","accept":"application/json"}, "{"),
                ("json-only", {"content-type":"application/json","accept":"application/json"}, json.dumps(body)),
                ("wildcard", {"content-type":"application/json","accept":"*/*"}, json.dumps(body)),
                ("modern-catalog", {"content-type":"application/json","accept":"application/json","mcp-protocol-version":"2026-07-28","mcp-method":"tools/call","mcp-name":"list_markets"}, json.dumps(modern)),
            ]:
                response = await client.post("/mcp", content=raw, headers=headers)
                yield {"name":name,"headers":headers,"body":raw,"status":response.status_code,"response":response.text}


async def main():
    data = [item async for item in vectors()]
    (ROOT / "deploy/rust/mcp-http-vectors.json").write_text(json.dumps(data,indent=2)+"\n")


if __name__ == "__main__":
    asyncio.run(main())
