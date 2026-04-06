"""
Fleet intelligence endpoint — serves dual-fuel fleet demand data.
Data is scraped from DNV monthly press releases and cached in a JSON file.
"""
import json
import logging
from pathlib import Path
from typing import List

from fastapi import APIRouter
from pydantic import BaseModel

logger = logging.getLogger(__name__)

router = APIRouter(prefix="/fleet-intelligence", tags=["fleet-intelligence"])

DATA_FILE = Path(__file__).parent.parent.parent / "data" / "fleet_demand.json"


class FleetDemandEntry(BaseModel):
    fuel: str
    ordered_vessels: int
    delivered_vessels: int
    avg_consumption_mt: int
    color: str


class FleetDemandResponse(BaseModel):
    entries: List[FleetDemandEntry]
    last_updated: str
    sources: List[str]


FALLBACK_DATA = {
    "entries": [
        {"fuel": "Methanol", "ordered_vessels": 323, "delivered_vessels": 112, "avg_consumption_mt": 9500, "color": "#5DADE2"},
        {"fuel": "Biofuel", "ordered_vessels": 20, "delivered_vessels": 11, "avg_consumption_mt": 6800, "color": "#4CAF50"},
        {"fuel": "Ammonia", "ordered_vessels": 45, "delivered_vessels": 2, "avg_consumption_mt": 12000, "color": "#9C27B0"},
        {"fuel": "LNG", "ordered_vessels": 1010, "delivered_vessels": 632, "avg_consumption_mt": 7500, "color": "#FF9800"},
    ],
    "last_updated": "2026-04-03",
    "sources": [
        "Clarksons Green Technology Tracker (Jan 2026)",
        "DNV AFI (Q1 2026)",
        "IndexBox Maritime Bunkering Report (Q1 2026)",
    ],
}


@router.get("", response_model=FleetDemandResponse)
async def get_fleet_demand():
    """Returns the latest dual-fuel fleet demand data."""
    if DATA_FILE.exists():
        try:
            data = json.loads(DATA_FILE.read_text())
            return FleetDemandResponse(**data)
        except Exception as e:
            logger.warning("Failed to read fleet demand data file: %s", e)
    return FleetDemandResponse(**FALLBACK_DATA)
