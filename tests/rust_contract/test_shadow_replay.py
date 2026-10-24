"""Opt-in disposable replay; never targets staging or production services."""
import json
import os
import subprocess
import urllib.request

import pytest

from scripts.rust_contract import compare, run_disposable, workload


@pytest.mark.skipif(os.environ.get("RUN_SHADOW_DISPOSABLE_REPLAY") != "1", reason="explicit disposable replay only")
def test_board_and_stateless_tools_replay_on_disposable_database():
    runner = run_disposable.Runner("all", seed=True, keep=False, seed_mode="corpus", gate=False)
    subprocess.run(runner.build_command(), env={**os.environ, "RELEASE_SHA": runner.sha}, check=True)
    try:
        environment = runner.start_environment("all")
        manifest = workload.load()
        rows = [{"id": "R11", "method": "GET", "path": "/api/curves/forward/board"}]
        rows += [{"id": t["id"], "method": "POST", "path": "/mcp", "body": t["body"]} for t in manifest["tools"]]
        for row in rows:
            observations = []
            for key in ("python_url", "rust_url", "python_url"):
                body = json.dumps(row["body"]).encode() if "body" in row else None
                headers = {"x-request-id": "shadow-" + row["id"].lower()}
                if body:
                    headers.update({"content-type":"application/json", "accept":"application/json, text/event-stream"})
                request = urllib.request.Request(environment[key] + row["path"], data=body, headers=headers)
                with urllib.request.urlopen(request, timeout=15) as response:
                    raw = response.read(1048577)
                    assert len(raw) <= 1048576
                    assert response.status == 200
                    observations.append({"status":200,"headers":dict(response.headers),"raw":raw,
                        "started_utc":"2026-10-15T12:00:00Z", "ended_utc":"2026-10-15T12:00:01Z"})
                if body:
                    assert not json.loads(raw).get("result", {}).get("isError", False)
            path = row["path"] + ("#" + row["id"] if body else "")
            assert not compare.compare_responses(observations[0], observations[2], request_path=path, live=True)
            assert not compare.compare_responses(observations[0], observations[1], request_path=path, live=True)
            print(row["id"] + ": disposable status=200/200/200 content=match")
    finally:
        runner.stop_environment()


@pytest.mark.asyncio
async def test_real_staging_mcp_host_policy_accepts_direct_stateless_call(monkeypatch):
    import anyio
    import httpx
    from app import mcp_server

    monkeypatch.setattr(mcp_server.settings, "ENVIRONMENT", "staging")
    server = mcp_server.build_mcp_server(read_limiter=anyio.CapacityLimiter(1))
    app = mcp_server.build_mcp_http_app(server)
    async with server.session_manager.run():
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url="http://127.0.0.1:8011") as client:
            body = {"jsonrpc":"2.0","id":7,"method":"tools/call", "params":{"name":"list_markets","arguments":{}}}
            headers = {"accept":"application/json, text/event-stream", "host":"127.0.0.1"}
            refused = await client.post("/mcp", json=body, headers=headers)
            assert refused.status_code == 421
            headers["host"] = "api-staging.verdaxis.exchange"
            accepted = await client.post("/mcp", json=body, headers=headers)
            assert accepted.status_code == 200
            assert "error" not in accepted.json()
            assert not accepted.json()["result"].get("isError", False)
