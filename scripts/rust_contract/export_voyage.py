"""Export Task 11's fixed voyage data and MCP tool schemas from this oracle."""
from __future__ import annotations

import asyncio
import json
from dataclasses import asdict
from pathlib import Path

from app.mcp_server import build_mcp_server
from app.schemas.voyage import PlannerFuel
from app.services.sea_distances import _DISTANCES, SEA_DISTANCE_METHOD
from app.services.voyage_fuel_plan import EU_PORTS, FUEL_PROPERTIES

ROOT = Path(__file__).resolve().parents[2]


async def main():
    distances = {
        "method": SEA_DISTANCE_METHOD,
        "routes": [
            {"from_port": first, "to_port": second, "route": route.value,
             "distance_nm": str(value.distance_nm), "via": value.via}
            for (first, second, route), value in _DISTANCES.items()
        ],
    }
    properties = {
        "conventional_fuel": PlannerFuel.VLSFO.value,
        "eu_ports": sorted(EU_PORTS),
        "fuels": {fuel.value: asdict(value) for fuel, value in FUEL_PROPERTIES.items()},
    }
    server = build_mcp_server(read_limiter=None)
    tools = [tool.model_dump(mode="json", by_alias=True, exclude_none=True)
             for tool in await server.list_tools()]
    for filename, value in (("sea-distances.json", distances),
                            ("voyage-properties.json", properties),
                            ("mcp-tools.json", {"tools": tools})):
        (ROOT / "deploy/rust" / filename).write_text(json.dumps(value, indent=2, default=str) + "\n")


if __name__ == "__main__":
    asyncio.run(main())
