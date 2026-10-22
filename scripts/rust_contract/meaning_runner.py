"""Explicit successor plumbing; comparison stays in compare_responses."""
import asyncio
import json
import os
from datetime import datetime
from pathlib import Path
from urllib.parse import parse_qs
from uuid import UUID

from scripts.rust_contract.compare import ContractError, compare_responses
from scripts.rust_contract.phase2_meaning import PHASE1_RULES_SHA256

ROOT = Path(__file__).resolve().parents[2]


def route_id(request):
    path = request["path"].split("?", 1)[0].rstrip("/")
    if path == "/mcp":
        try:
            tool = json.loads(request.get("body", "{}"))["params"]["name"]
        except (ValueError, KeyError, TypeError):
            return "P1-mcp-protocol"
        return "P1-tool-" + tool if tool in {"list_markets", "get_market_slice", "plan_voyage_fuel"} else "P1-mcp-protocol"
    inventory = json.loads((ROOT / "tests/rust_contract/phase1_meaning/routes.json").read_text())
    for route in inventory["routes"]:
        if path == route["path"] or (route["id"] == "R02" and path.startswith("/api/ports/")):
            return route["comparator_route_id"]
    return "P1-strict-auxiliary"


def raw_response(recording):
    exchange = recording["exchange"]
    raw = recording.get("raw_body")
    if raw is None:
        try:
            raw = base64.b64decode(recording["raw_body_base64"], validate=True)
        except (KeyError, ValueError) as error:
            raise ContractError("successor recording requires retained wire bytes") from error
    return {"status": exchange["status"], "headers": exchange["headers"], "body": raw}


async def canonical_response(request, response):
    from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

    from scripts.rust_contract.trade_tape_oracle import read_canonical_tape_page

    url = os.environ.get("DATABASE_URL", "")
    identity = os.environ.get("RUST_CORPUS_MANIFEST", "")
    clock = os.environ.get("RUST_CONTRACT_CLOCK", "")
    if not url or not identity or not clock or os.environ.get("ENVIRONMENT") != "test":
        raise ContractError("canonical tape snapshot evidence is unavailable")
    query = parse_qs(request.get("query", ""), keep_blank_values=True)
    filters = {key: values[-1] for key, values in query.items() if key not in {"skip", "limit"}}
    if "delivery_point_id" in filters:
        filters["delivery_point_id"] = UUID(filters["delivery_point_id"])
    filters["demo_only"] = str(filters.get("demo_only", "false")).lower() in {"true", "1", "yes", "on", "t", "y"}
    engine = create_async_engine(url)
    try:
        async with async_sessionmaker(engine)() as session:
            async with session.begin():
                body, proof = await read_canonical_tape_page(session, filters, datetime.fromisoformat(clock.replace("Z", "+00:00")),
                    int(query.get("skip", [0])[-1]), int(query.get("limit", [50])[-1]), snapshot_identity=identity)
        return {**response, "body": json.dumps(body).encode()}, proof
    finally:
        await engine.dispose()


def compare_recorded_case(left, right, profile, digest):
    if profile != "phase1-meaning-v1" or digest != PHASE1_RULES_SHA256:
        raise ContractError("explicit reviewed successor profile and digest required")
    request = left.get("request", {})
    identity = route_id(request)
    expected, actual = raw_response(left), raw_response(right)
    context = None
    if identity.startswith("P1-tool-"):
        params = json.loads(request["body"])["params"]
        context = {"tool": params["name"], "arguments": params.get("arguments") or {}}
    if identity == "P1-R06" and expected["status"] == actual["status"] == 200:
        # Unit callers can supply their own complete-relation canonical proof.
        # Empty pages need no tie selection, but still retain exact totals.
        body = json.loads(expected["body"])
        if body.get("items"):
            expected, _proof = asyncio.run(canonical_response(request, expected))
    problems = compare_responses(expected, actual, profile=profile, route_id=identity,
                                 rules_sha256=digest, diagnostic_context=context)
    evidence = os.environ.get("MEANING_CORPUS_DIR")
    if evidence:
        directory = Path(evidence)
        directory.mkdir(parents=True, exist_ok=True)
        entry = {"id": left["id"], "request": request, "exchange": {"status": expected["status"], "headers": expected["headers"]},
                 "raw_body_base64": base64.b64encode(expected["body"]).decode(),
                 "profile": profile, "rules_sha256": digest, "differences": problems,
                 "kyc_gate": os.environ.get("KYC_EXECUTION_GATE_ENABLED")}
        name = hashlib.sha256(json.dumps(request, sort_keys=True).encode()).hexdigest()
        (directory / (name + ".json")).write_text(json.dumps(entry, indent=2) + "\n")
    return problems
