"""Private implementation of the explicitly selected Phase 2 comparison profile.

Call compare.compare_responses, not this module. The digest is a review boundary:
changing a rule requires changing the committed ledger, this pin and its tests.
"""

import hashlib
import json
import re
from collections import Counter
from datetime import UTC, datetime
from decimal import Decimal, InvalidOperation
from itertools import groupby
from pathlib import Path
from uuid import UUID

from scripts.rust_contract.compare import (
    ContractError,
    _declared_length,
    _is_json,
    _object_without_duplicates,
    strip_headers,
)

RULES_PATH = Path(__file__).resolve().parents[2] / "tests/rust_contract/phase2/normalization.json"
APPROVED_RULES_SHA256 = "08e9165617b804704a35fa46685f90ca310bd3f0985c61797ddad20c706dfee4"
PHASE1_RULES_PATH = RULES_PATH.parents[1] / "phase1_meaning/normalization.json"
PHASE1_RULES_SHA256 = "92578257e18b80120fbb87d27d3a4f3ae6e83bc84342ba33bd07aed20d38b80d"
PROFILES = {
    "phase2-meaning-v1": (RULES_PATH, APPROVED_RULES_SHA256),
    "phase1-meaning-v1": (PHASE1_RULES_PATH, PHASE1_RULES_SHA256),
}
TIMESTAMP = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]{1,6})?(?:Z|[+-][0-9]{2}:[0-9]{2})")
DECIMAL = re.compile(r"[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?")
MINIMUM_TIME = datetime.min.replace(tzinfo=UTC)


def _invalid_constant(_value):
    raise ContractError("nonfinite JSON number")


def _parse(raw):
    try:
        return json.loads(raw.decode("utf-8"), parse_int=Decimal, parse_float=Decimal,
                          parse_constant=_invalid_constant, object_pairs_hook=_object_without_duplicates)
    except (UnicodeError, ValueError, InvalidOperation, RecursionError) as error:
        raise ContractError("response body is not valid finite UTF-8 JSON") from error


def _load_rules(path, digest, route_id, profile):
    default_path, approved_digest = PROFILES[profile]
    try:
        raw = (path or default_path).read_bytes()
    except OSError as error:
        raise ContractError("normalization ledger is unreadable") from error
    if digest != approved_digest or hashlib.sha256(raw).hexdigest() != digest:
        raise ContractError("unreviewed or mismatched normalization ledger")
    rules = _parse(raw)
    if rules["profile"] != profile or route_id not in rules["routes"]:
        raise ContractError("unknown comparator route ID or profile ledger")
    return rules["routes"][route_id]


def _instant(value):
    if not isinstance(value, str) or not TIMESTAMP.fullmatch(value):
        raise ContractError("invalid aware microsecond timestamp")
    # datetime accepts invalid offset minute components by carrying them, so
    # check the wire grammar's hour/minute bounds before parsing.
    if value[-1] != "Z" and (int(value[-5:-3]) > 23 or int(value[-2:]) > 59):
        raise ContractError("invalid timestamp offset")
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone(UTC)
    except (ValueError, OverflowError) as error:
        raise ContractError("invalid aware microsecond timestamp") from error


def _decimal_string(value):
    if not isinstance(value, str) or not DECIMAL.fullmatch(value):
        raise ContractError("invalid finite decimal string")
    try:
        number = Decimal(value)
    except InvalidOperation as error:
        raise ContractError("invalid finite decimal string") from error
    if not number.is_finite():
        raise ContractError("invalid finite decimal string")
    return number


def _tie_key(item, kind):
    if not isinstance(item, dict):
        raise ContractError("tie array item is not an object")
    if kind == "unordered":
        return ()
    if kind in {"created_at", "signed_up_at", "published_at"}:
        return (_instant(item.get(kind)),)
    if kind == "referral_count":
        number = item.get(kind)
        if not isinstance(number, Decimal) or number < 0 or number != number.to_integral_value():
            raise ContractError("invalid referral count tie key")
        return (number,)
    if kind in {"slice", "pin"}:
        latest = item.get("latest_event_at")
        key = (_instant(latest) if latest is not None else MINIMUM_TIME, _instant(item.get("created_at")))
        if kind == "slice":
            count = item.get("unread_event_count")
            if not isinstance(count, Decimal) or count < 0 or count != count.to_integral_value():
                raise ContractError("invalid unread count tie key")
            key = (count > 0,) + key
        return key
    raise ContractError("unknown tie key rule")


def _diagnostic_contract(context):
    """Use the pinned Python validator as input authority, never parse its repr."""
    from pydantic import ValidationError

    from app.mcp_server import build_mcp_server

    if not isinstance(context, dict) or set(context) != {"tool", "arguments"}:
        raise ContractError("MCP diagnostic comparison needs exact request context")
    tool = context["tool"]
    metadata = build_mcp_server(read_limiter=None)._tool_manager.get_tool(tool)
    if metadata is None:
        raise ContractError("unknown MCP diagnostic tool")
    metadata = metadata.fn_metadata
    try:
        metadata.arg_model.model_validate(metadata.pre_parse_json(context["arguments"]))
    except ValidationError as error:
        errors = _parse(error.json(include_url=False, include_context=False).encode())
        text = f"Error executing tool {tool}: {error}"
        return text, errors
    raise ContractError("diagnostic request is not rejected by the pinned validator")


def _mcp_payload(value, rules, context):
    """Declare the two tool-payload copies to the existing JSON walker."""
    result = value.get("result", {}) if isinstance(value, dict) else {}
    outer = {"decimal_paths": [], "datetime_paths": [], "ties": []}
    if isinstance(result, dict) and result.get("isError") is not True:
        for field in ("decimal_paths", "datetime_paths", "map_paths"):
            outer[field] = ["/result/structuredContent" + path for path in rules.get(field, [])]
        outer["embedded_json"] = {"/result/content/*/text": rules}
    elif isinstance(result, dict) and result.get("isError") is True and context is not None:
        # Only rejected argument-validation requests admit decision 84. Ordinary
        # tool failures keep exact text, even when the caller supplies context.
        try:
            outer["diagnostic"] = _diagnostic_contract(context)
        except ContractError:
            pass
    return _canonical(value, "", outer)


def _canonical(value, path, rules):
    """Tag every JSON kind; wildcards are introduced only by array traversal."""
    if value is None:
        return ("null",)
    if type(value) is bool:
        return ("boolean", value)
    if isinstance(value, Decimal):
        return ("number", value)
    if isinstance(value, str):
        if path in rules.get("embedded_json", {}):
            return ("embedded-json", _canonical(_parse(value.encode()), "", rules["embedded_json"][path]))
        if path == "/result/content/*/text" and "diagnostic" in rules:
            expected_text, errors = rules["diagnostic"]
            if value == expected_text:
                return ("validation-diagnostic", _canonical(errors, "", {"decimal_paths": [], "datetime_paths": [], "ties": []}))
            if value.startswith("Validation failed: "):
                parsed = _parse(value.removeprefix("Validation failed: ").encode())
                exact = {"decimal_paths": [], "datetime_paths": [], "ties": []}
                actual_errors = _canonical(parsed, "", exact)
                expected_errors = _canonical(errors, "", exact)
                if actual_errors != expected_errors:
                    return ("invalid-diagnostic", actual_errors)
                return ("validation-diagnostic", actual_errors)
        if path in rules["decimal_paths"]:
            return ("decimal-string", _decimal_string(value))
        if path in rules["datetime_paths"]:
            return ("datetime-string", _instant(value))
        return ("string", value)
    if isinstance(value, dict):
        return ("object", tuple((key, _canonical(child, path + "/" + ("*" if path in rules.get("map_paths", []) else key.replace("~", "~0").replace("/", "~1")), rules)) for key, child in sorted(value.items())))
    if isinstance(value, list):
        children = [_canonical(child, path + "/*", rules) for child in value]
        tie = next((entry for entry in rules["ties"] if entry["path"] == path), None)
        if tie is None:
            return ("array", tuple(children))
        keys = [_tie_key(item, tie["key"]) for item in value]
        if keys != sorted(keys, reverse=True):
            return ("invalid-tie-order", tuple(children))
        groups = tuple((key, frozenset(Counter(child for _, child in pairs).items()))
                       for key, pairs in groupby(zip(keys, children), key=lambda pair: pair[0]))
        return ("tie-array", tuple(keys), groups)
    raise ContractError("unsupported parsed JSON kind")


def _response(response, rules, request_id, diagnostic_context=None):
    if not isinstance(response, dict) or set(response) != {"status", "headers", "body"}:
        raise ContractError("raw response requires exactly status, headers and body")
    status, raw = response["status"], response["body"]
    if type(status) is not int or not 100 <= status <= 599 or not isinstance(raw, bytes):
        raise ContractError("invalid raw response status/body")
    headers = response["headers"]
    if not isinstance(headers, dict) or not all(isinstance(k, str) and isinstance(v, str) for k, v in headers.items()):
        raise ContractError("invalid raw response headers")
    if len({key.lower() for key in headers}) != len(headers):
        raise ContractError("duplicate case-insensitive response header")
    declared = _declared_length(headers)
    if declared is not None and declared != len(raw):
        raise ContractError("declared Content-Length does not match the body")
    kept = strip_headers(headers)
    if "x-request-id" in kept:
        rid = kept["x-request-id"]
        if request_id is None:
            try:
                parsed = UUID(rid)
            except ValueError as error:
                raise ContractError("invalid generated request ID") from error
            if str(parsed) != rid or parsed.version != 4:
                raise ContractError("invalid generated request ID")
            kept["x-request-id"] = "<generated-request-id>"
        elif not re.fullmatch(r"[A-Za-z0-9._:-]{1,36}", rid):
            raise ContractError("invalid supplied response request ID")
    if status == 204:
        if raw:
            raise ContractError("204 response has a nonempty body")
        body = ("empty",)
    else:
        codec = rules.get("codec", "json")
        is_json = _is_json(kept.get("content-type", ""))
        if codec == "bytes" and 200 <= status < 300 or not is_json and codec == "json-or-bytes":
            body = ("bytes", raw)
        else:
            if not is_json:
                raise ContractError("route requires a JSON response")
            parsed = _parse(raw)
            # Errors retain exact fields and strings. MCP validation uses only
            # its declared codec, request context, and structured error values.
            active_rules = rules if 200 <= status < 300 else {"decimal_paths": [], "datetime_paths": [], "ties": []}
            body = (_mcp_payload(parsed, active_rules, diagnostic_context)
                    if codec == "mcp-json" and 200 <= status < 300 else
                    _canonical(parsed, "", active_rules))
    return {"status": status, "headers": kept, "body": body}


def compare_meaning(expected, actual, *, route_id, rules_sha256, rules_path=None, request_id=None,
                    profile="phase2-meaning-v1", diagnostic_context=None):
    from scripts.rust_contract.compare import compare

    rules = _load_rules(rules_path, rules_sha256, route_id, profile)
    if request_id is not None and not re.fullmatch(r"[A-Za-z0-9._:-]{1,36}", request_id):
        raise ContractError("invalid harness-supplied request ID")
    left = _response(expected, rules, request_id, diagnostic_context)
    right = _response(actual, rules, request_id, diagnostic_context)
    differences = compare(left, right)

    def has_invalid_order(value):
        return isinstance(value, tuple) and (
            (bool(value) and value[0] in ("invalid-tie-order", "invalid-diagnostic"))
            or any(has_invalid_order(child) for child in value)
        )

    if any(has_invalid_order(item["body"]) for item in (left, right)):
        differences.append("/body:tie-key-order")
    if request_id is not None and any(r["headers"].get("x-request-id") != request_id for r in (left, right)):
        differences.append("/headers/x-request-id:round-trip")
    return differences
