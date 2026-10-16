"""Full pure-engine oracle vectors; no database or provider access."""
import json
from datetime import date
from pathlib import Path

from app.schemas.voyage import VoyagePlanRequest
from app.services.voyage_fuel_plan import build_voyage_plan
from tests.unit.test_voyage_fuel_plan import golden_prices, golden_request

ROOT = Path(__file__).resolve().parents[2]


def vectors():
    base = golden_request()
    candidates = [("golden", base, golden_prices(base))]
    for label, changes in [
        ("missing", {"assumptions": {}}),
        ("conventional", {"vessel": {**base.vessel.model_dump(mode="json"), "compatible_fuels": ["VLSFO"]}}),
        ("green-only", {"vessel": {**base.vessel.model_dump(mode="json"), "compatible_fuels": ["BIO_METHANOL"]}}),
        ("missing-intensity", {"vessel": {**base.vessel.model_dump(mode="json"), "compatible_fuels": ["VLSFO", "BIO_ETHANOL"]}}),
        ("no-eu", {"legs": [{"from_port": "Shanghai", "to_port": "Singapore"}]}),
        ("too-old", {"start_date": "2026-09-13"}),
        ("too-far", {"start_date": "2028-10-14"}),
        ("too-long", {"vessel": {**base.vessel.model_dump(mode="json"), "speed_knots": "1"}, "legs": [{"from_port":"Shanghai", "to_port":"Singapore", "distance_nm":"30000"}]}),
        ("invalid-cape", {"legs": [{"from_port":"Shanghai", "to_port":"Singapore", "route":"CAPE_OF_GOOD_HOPE"}]}),
    ]:
        request = VoyagePlanRequest.model_validate({**base.model_dump(mode="json"), **changes})
        candidates.append((label, request, {}))
    for exponent in (-1000027, -2147483648, -1999999999999999997):
        raw = base.model_dump(mode="json")
        raw["vessel"]["sea_consumption_mt_per_day"] = f"1e{exponent}"
        candidates.append((f"underflow-{exponent}", VoyagePlanRequest.model_validate(raw), {}))
    results = []
    for label, request, prices in candidates:
        item = {"name": label, "today": "2026-10-14", "request": request.model_dump(mode="json"),
                "prices": [{"lookup": key.model_dump(mode="json"), "price": value.model_dump(mode="json")} for key, value in prices.items()]}
        try:
            item["result"] = build_voyage_plan(request, prices, today=date(2026, 10, 14)).model_dump(mode="json")
        except ValueError as error:
            item["error"] = str(error)
        results.append(item)
    return results


if __name__ == "__main__":
    (ROOT / "deploy/rust/voyage-vectors.json").write_text(json.dumps(vectors(), indent=2) + "\n")
