"""Export declared DTO scalar paths; run only on the isolated test host."""

import hashlib
import json
import types
from datetime import datetime
from decimal import Decimal
from pathlib import Path
from typing import Annotated, Union, get_args, get_origin

from pydantic import BaseModel

from app.schemas import catalog, curves, orderbook, port, trade_tape, voyage
from app.schemas.pagination import PaginatedResponse

ROOT = Path(__file__).resolve().parents[3]
DESTINATION = ROOT / "tests/rust_contract/phase1_meaning/normalization.json"


def scalar_paths(annotation, path="", found=None):
    if found is None:
        found = {"decimal_paths": [], "datetime_paths": [], "map_paths": []}
    origin = get_origin(annotation)
    args = get_args(annotation)
    if annotation is Decimal:
        found["decimal_paths"].append(path)
    elif annotation is datetime:
        found["datetime_paths"].append(path)
    elif origin in (Union, types.UnionType):
        for child in args:
            scalar_paths(child, path, found)
    elif origin is Annotated:
        scalar_paths(args[0], path, found)
    elif origin in (list, tuple):
        scalar_paths(args[0], path + "/*", found)
    elif origin is dict:
        # Only these typed maps introduce wildcard object traversal.
        found["map_paths"].append(path)
        scalar_paths(args[1], path + "/*", found)
    elif isinstance(annotation, type) and issubclass(annotation, BaseModel):
        for name, field in annotation.model_fields.items():
            if not field.exclude:
                scalar_paths(field.annotation, path + "/" + name, found)
    return {key: sorted(set(values)) for key, values in found.items()}


def export():
    models = {
        "R01": list[port.PortResponse], "R02": port.PortResponse,
        "R03": list[catalog.ProductResponse], "R04": list[catalog.DeliveryPointResponse],
        "R06": trade_tape.TradeTapeResponse, "R07": curves.ForwardCurveResponse,
        "R09": curves.ForwardCurveTableResponse, "R10": curves.ForwardCurveSliceResponse,
        "R11": curves.ForwardCurveBoardResponse, "R12": orderbook.PriceDiscoveryResponse,
        "R13": orderbook.ReferencePriceResponse, "R15": orderbook.OrderBookSnapshotResponse,
        "R16": PaginatedResponse[orderbook.OrderResponse], "R17": PaginatedResponse[orderbook.OrderResponse],
        "R18": list[orderbook.OrderResponseWithCI], "R19": list[orderbook.AggregatedOrderbookResponse],
        "R20": orderbook.ProductCountResponse, "R21": orderbook.MapSummaryResponse,
        "R22": orderbook.MapCompactSummaryResponse, "R23": list[str], "R24": list[str],
        "R25": list[str], "R26": list[orderbook.OrderResponse],
    }
    routes = {}
    for number in range(1, 28):
        identity = f"R{number:02}"
        rules = scalar_paths(models.get(identity, str))
        rules.update(codec="bytes" if identity in {"R08", "R14"} else "json", ties=[])
        routes["P1-" + identity] = rules
    routes["P1-R05"].update(datetime_paths=["/*/fetched_at", "/*/published_at"], ties=[{"path": "", "key": "published_at"}])
    routes["P1-R12"]["ties"] = [{"path": "/summaries", "key": "unordered"}]
    routes["P1-R06"]["page_oracle"] = "decision85-v1:complete-python-relation-before-pagination"
    # The compatibility listing is an explicit dict projection, not a DTO.
    routes["P1-R27"].update(
        decimal_paths=["/*/carbon_intensity_gco2_mj", "/*/price_per_mt_usd", "/*/quantity_mt"],
        datetime_paths=["/*/expires_at", "/*/observed_at"],
    )
    for name, model in (("list_markets", voyage.MarketCatalog), ("get_market_slice", voyage.MarketSliceSnapshot), ("plan_voyage_fuel", voyage.VoyagePlanResult)):
        rules = scalar_paths(model)
        # Paths refer to the tool payload; the codec applies them to both copies.
        rules.update(codec="mcp-json", ties=[], diagnostic_presentation="decision84-v1")
        routes["P1-tool-" + name] = rules
    for identity in ("health", "echo", "live-benchmarks", "mcp-protocol", "strict-auxiliary"):
        routes["P1-" + identity] = {**scalar_paths(str), "codec": "json-or-bytes", "ties": []}
    sources = [
        "app/schemas/port.py", "app/schemas/catalog.py", "app/schemas/trade_tape.py",
        "app/schemas/orderbook.py", "app/schemas/curves.py", "app/schemas/voyage.py",
        "app/schemas/pagination.py", "app/routers/news.py", "app/routers/inventory.py",
        "app/schemas/fame_order.py", "app/schemas/lng_order.py",
    ]
    return {"profile": "phase1-meaning-v1", "version": 1, "decisions": [84, 85],
            "sources": {path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest() for path in sources},
            "routes": routes}


if __name__ == "__main__":
    DESTINATION.write_text(json.dumps(export(), indent=2) + "\n")
