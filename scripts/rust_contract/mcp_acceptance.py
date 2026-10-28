"""MCP privacy and real SQL cancellation checks on the attested disposable runner."""
from __future__ import annotations

import asyncio
import json
import os
import time

import asyncpg
import httpx

from scripts.rust_contract.compare import attest, load_cases
from scripts.rust_contract.record import record
from scripts.rust_contract.run_disposable import ROOT

FORBIDDEN = {
    "organization_id", "owner_id", "user_id", "actor_id", "order_id",
    "inventory_item_id", "certificate_number", "scheme_certificate_number",
    "physical_supplier_name", "certificate_reference", "certificate_holder",
    "batch_reference", "document_references",
}


def privacy(value, private_values):
    if isinstance(value, dict):
        assert not FORBIDDEN.intersection(value), "private field in MCP output"
        for key, child in value.items():
            privacy(key, private_values)
            privacy(child, private_values)
    elif isinstance(value, list):
        for child in value:
            privacy(child, private_values)
    elif isinstance(value, str):
        assert not any(secret in value for secret in private_values), "private value in MCP output"


async def fixture_private_values():
    """Read sentinels only from the runner's attested disposable fixture database."""
    url = os.environ["RUST_DATABASE_URL"]
    assert "127.0.0.1:" in url and url.endswith("/verdaxis_rust_test")
    connection = await asyncpg.connect(url)
    try:
        values = set()
        # Public catalog IDs are deliberately absent. Every ID here names a
        # private participant, order, inventory item or assisted authority.
        for table in ("organizations", "users", "orderbook_orders", "inventory_items",
                      "market_support_authorizations", "staff_capability_assignments"):
            rows = await connection.fetch(f"SELECT id::text AS value FROM {table}")
            values.update(row["value"] for row in rows)
        rows = await connection.fetch("SELECT fame_terms, lng_terms FROM orderbook_orders")
        for row in rows:
            for column in ("fame_terms", "lng_terms"):
                terms = json.loads(row[column]) if isinstance(row[column], str) else row[column]
                if isinstance(terms, dict):
                    def collect_private(node):
                        if isinstance(node, dict):
                            for key, value in node.items():
                                if key in FORBIDDEN or key == "reference":
                                    for text in value if isinstance(value, list) else [value]:
                                        if isinstance(text, str) and text:
                                            values.add(text)
                                collect_private(value)
                        elif isinstance(node, list):
                            for value in node:
                                collect_private(value)
                    collect_private(terms)
        assert values, "privacy proof requires populated fixture identifiers"
        return values
    finally:
        await connection.close()


async def cancellation(base_url):
    url = os.environ["RUST_DATABASE_URL"]
    assert "127.0.0.1:" in url and url.endswith("/verdaxis_rust_test")
    control = await asyncpg.connect(url)
    observer = await asyncpg.connect(url)
    headers = {"accept":"application/json, text/event-stream", "content-type":"application/json"}
    cases = load_cases(ROOT / "tests/rust_contract/cases.json", "mcp")
    requests = [json.loads(next(case for case in cases if case["id"] == name)["body"])
                for name in ("M05-slice-mixed", "M12-voyage-methanol")]
    async with httpx.AsyncClient(base_url=base_url, headers=headers, timeout=15) as client:
        for request in requests:
            transaction = control.transaction()
            await transaction.start()
            try:
                await control.execute("LOCK TABLE orderbook_orders IN ACCESS EXCLUSIVE MODE")
                callers = [asyncio.create_task(client.post("/mcp", json=request)) for _ in range(16)]
                deadline = time.monotonic() + 10
                while True:
                    count = await observer.fetchval("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%eligible_orders%'")
                    if count == 16:
                        break
                    assert time.monotonic() < deadline, "configured MCP SQL reads did not start"
                    await asyncio.sleep(.02)
                for caller in callers:
                    caller.cancel()
                await asyncio.gather(*callers, return_exceptions=True)
                count = await observer.fetchval("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%eligible_orders%'")
                assert count == 16, "HTTP cancellation cancelled admitted SQL"
                busy = (await client.post("/mcp",json=request)).json()
                assert busy["result"]["isError"] is True
                assert "Verdaxis market tools are busy; retry shortly." in busy["result"]["content"][0]["text"]
                catalog = (await client.post("/mcp",json={"jsonrpc":"2.0","id":90,"method":"tools/call","params":{"name":"list_markets","arguments":{}}})).json()
                assert catalog["result"]["isError"] is False
            finally:
                await transaction.rollback()
            deadline = time.monotonic()+10
            while True:
                response = (await client.post("/mcp",json=request)).json()
                if not response["result"]["isError"]:
                    break
                assert time.monotonic()<deadline, "MCP admission did not recover"
                await asyncio.sleep(.02)
            while await observer.fetchval("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND state='active' AND pid<>pg_backend_pid() AND query LIKE '%eligible_orders%'"):
                assert time.monotonic()<deadline, "cancelled SQL did not drain"
                await asyncio.sleep(.02)
            # Successful work returns its connection before releasing the permit.
            # Wait for the last cleanup ping as well before the next configured-call batch.
            await asyncio.sleep(.2)
            print(f"SQL cancellation verified: {request['params']['name']}",flush=True)
    await observer.close()
    await control.close()


def main():
    base_url=os.environ["RUST_CONTRACT_URL"]
    token=os.environ["DISPOSABLE_TEST_TOKEN"]
    attest(base_url,token)
    cases=load_cases(ROOT / "tests/rust_contract/cases.json","mcp")
    entries=record(base_url,token,cases)
    private_values = asyncio.run(fixture_private_values())
    tools=set()
    for entry in entries:
        body=entry["exchange"].get("body")
        if isinstance(body,dict) and not body.get("result",{}).get("isError",True):
            result=body["result"]
            privacy(result.get("structuredContent"), private_values)
            for content in result.get("content",[]):
                if content.get("type")=="text":
                    privacy(content["text"], private_values)
                    privacy(json.loads(content["text"]), private_values)
            request=json.loads(entry["request"]["body"])
            if request["method"]=="tools/call":tools.add(request["params"]["name"])
    assert tools=={"list_markets","get_market_slice","plan_voyage_fuel"}
    print("Recursive privacy verified: all three tools",flush=True)
    asyncio.run(cancellation(base_url))


if __name__ == "__main__":
    main()
