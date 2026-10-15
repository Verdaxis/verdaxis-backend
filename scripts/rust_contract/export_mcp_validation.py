"""Export exact tool argument diagnostics from the Python oracle, without I/O."""
import copy
import json
from pathlib import Path

from pydantic import ValidationError

from app.mcp_server import build_mcp_server

ROOT = Path(__file__).resolve().parents[2]


def vectors():
    server = build_mcp_server(read_limiter=None)
    cases = json.loads((ROOT / "tests/rust_contract/cases.json").read_text())["groups"]["mcp"]
    base = json.loads(next(case for case in cases if case["id"] == "M12-voyage-methanol")["body"])["params"]["arguments"]
    inputs = []
    for name, path, value in [
        ("unknown-intensity", ["assumptions", "ghg_intensity_gco2e_per_mj"], {"BOGUS": "35"}),
        ("midnight-date", ["start_date"], "2026-10-15T00:00:00Z"),
        ("padded-decimal", ["vessel", "speed_knots"], " 14 "),
        ("wrong-speed", ["vessel", "speed_knots"], []),
        ("null-speed", ["vessel", "speed_knots"], None),
        ("boolean-speed", ["vessel", "speed_knots"], True),
        ("bad-decimal", ["vessel", "speed_knots"], "fast"),
        ("nonfinite-decimal", ["vessel", "speed_knots"], "NaN"),
        ("superscript-imo", ["vessel", "imo_number"], "²²²²²²²"),
        ("tiny-decimal", ["vessel", "sea_consumption_mt_per_day"], "1e-10001"),
        ("duplicate-fuel", ["vessel", "compatible_fuels"], ["VLSFO", "VLSFO"]),
        ("unknown-fuel", ["vessel", "compatible_fuels"], ["BOGUS"]),
        ("wrong-fuels", ["vessel", "compatible_fuels"], "VLSFO"),
        ("extra-vessel", ["vessel", "extra"], "private"),
        ("extra-request", ["extra"], 1),
        ("extra-assumptions", ["assumptions", "extra"], 1),
        ("year-coercion", ["assumptions", "reporting_year"], "2035"),
        ("unknown-price-port", ["assumptions", "vlsfo_price_usd_per_mt"], {"Hamburg": "600"}),
    ]:
        arguments = copy.deepcopy(base)
        node = arguments["request"]
        for key in path[:-1]:
            node = node[key]
        node[path[-1]] = value
        inputs.append((name, "plan_voyage_fuel", arguments))
    arguments = copy.deepcopy(base)
    del arguments["request"]["vessel"]["speed_knots"]
    inputs.append(("missing-speed", "plan_voyage_fuel", arguments))
    for field in ("market_product", "delivery_point", "availability_window"):
        for label, value in [("null", None), ("number", 42), ("array", [])]:
            arguments = {"market_product": "BIO_METHANOL", "delivery_point": "Singapore", "availability_window": "SPOT"}
            arguments[field] = value
            inputs.append((f"slice-{field}-{label}", "get_market_slice", arguments))
    for label, change in [
        ("same-ports", lambda r: r["legs"][0].update(to_port="Singapore")),
        ("disconnected", lambda r: r["legs"].append({"from_port":"Shanghai", "to_port":"Busan"})),
        ("extra-leg", lambda r: r["legs"][0].update(extra="private")),
        ("bad-price-bound", lambda r: r["assumptions"].update(vlsfo_price_usd_per_mt={"Singapore":"0"})),
        ("bad-intensity-bound", lambda r: r["assumptions"].update(ghg_intensity_gco2e_per_mj={"VLSFO":"201"})),
        ("wrong-price-map", lambda r: r["assumptions"].update(vlsfo_price_usd_per_mt=[])),
        ("aggregate-errors", lambda r: (r["vessel"].update(speed_knots=[], extra=1), r["legs"][0].update(from_port=42, extra=2), r.update(start_date="bad", extra=3))),
    ]:
        arguments = copy.deepcopy(base)
        change(arguments["request"])
        inputs.append((label, "plan_voyage_fuel", arguments))
    for label, value in [("float", 2035.0), ("fraction", 2035.5), ("decimal-string", "2035.0"), ("bool", True), ("bad", "bad"), ("underscores", "2_035"), ("unicode", "２０３５")]:
        arguments = copy.deepcopy(base)
        arguments["request"]["assumptions"]["reporting_year"] = value
        inputs.append(("year-" + label, "plan_voyage_fuel", arguments))
    for label, field, value in [("arabic-imo", "imo_number", "١٢٣٤٥٦٧"), ("wide-imo", "imo_number", "１２３４５６７"), ("arabic-decimal", "speed_knots", "١٤.٠"), ("wide-decimal", "speed_knots", "１４"), ("unicode-exponent", "speed_knots", "١.٤e١")]:
        arguments = copy.deepcopy(base)
        arguments["request"]["vessel"][field] = value
        inputs.append((label, "plan_voyage_fuel", arguments))
    for label, value in [("object", json.dumps(base["request"])), ("null", "null"), ("array", "[]"), ("scalar", "42"), ("invalid", "{bad")]:
        inputs.append(("json-request-" + label, "plan_voyage_fuel", {"request": value}))
    for name, tool, arguments in inputs:
        metadata = server._tool_manager.get_tool(tool).fn_metadata
        vector = {"name": name, "tool": tool, "arguments": arguments}
        try:
            parsed = metadata.arg_model.model_validate(metadata.pre_parse_json(arguments))
            vector["normalized"] = parsed.model_dump(mode="json")
        except ValidationError as error:
            vector["error"] = str(error)
        yield vector


if __name__ == "__main__":
    path = ROOT / "deploy/rust/mcp-validation-vectors.json"
    path.write_text(json.dumps(list(vectors()), ensure_ascii=False, indent=2) + "\n")
