"""Strict public-contract comparator and external comparison runner.

This module is the single authority for Python/Rust contract equality. The
recorded Python corpus and the Rust replay are compared with `compare`, which
reports a difference for every schema path where the two disagree. Nothing is
normalized to make a difference disappear: decimal scale, null versus absence,
object membership, array order, header presence, and raw body bytes all
survive the walk.

The module also holds the comparison CLI. It attests both endpoints, records
the Python corpus, records the Rust replay, and compares them per case. It
prints schema paths and categories only, never a body value.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import urllib.error
import urllib.request
from collections.abc import Mapping, Sequence
from decimal import Decimal
from pathlib import Path
from typing import Any

SCHEMA_REVISION = 1
IDENTITY_PATH = "/.well-known/verdaxis-disposable-test"
ROOT = Path(__file__).resolve().parents[2]
CASES_PATH = ROOT / "tests" / "rust_contract" / "cases.json"
# The reviewed news tie rule. Exactly one rule is approved (decision 18).
NEWS_TIE_RULES_PATH = ROOT / "tests" / "rust_contract" / "news-tie-paths.json"
APPROVED_NEWS_TIE_RULES = [
    {
        "rule": "news-published-at-tie-multiset",
        "request_path": "/api/news",
        "json_path": "/body",
        "sort_key": "published_at",
    }
]

# Transport-generated headers carry no contract meaning; every other header is
# compared, including its absence. Content-Length is verified separately.
TRANSPORT_HEADERS = frozenset(
    {"date", "server", "connection", "transfer-encoding", "content-length"}
)

# Fields whose value is the request's own clock. They are validated and
# replaced by one marker only for paired live observations; a fixed-clock
# golden run compares them exactly. Never add a stored timestamp here.
VOLATILE_FIELDS = frozenset({"generated_at", "observed_at"})


class ContractError(RuntimeError):
    """A recording, attestation, or comparison precondition failed."""


def _object_without_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ContractError(f"duplicate JSON key {key!r}")
        result[key] = value
    return result


def parse_json(raw: bytes) -> Any:
    """Parse JSON, rejecting duplicate keys and keeping decimals exact."""
    try:
        return json.loads(
            raw.decode("utf-8"),
            parse_float=Decimal,
            parse_int=int,
            object_pairs_hook=_object_without_duplicates,
        )
    except (UnicodeDecodeError, ValueError) as error:
        raise ContractError("response body is not valid JSON") from error


def strip_headers(headers: Mapping[str, str]) -> dict[str, str]:
    """Drop transport-generated headers; keep every other header."""
    return {
        str(name).lower(): str(value)
        for name, value in headers.items()
        if str(name).lower() not in TRANSPORT_HEADERS
    }


def compare(expected: dict[str, Any], actual: dict[str, Any]) -> list[str]:
    """Return the schema paths that differ. An empty list means equal."""
    differences: list[str] = []

    def walk(left: Any, right: Any, path: str) -> None:
        if type(left) is not type(right):
            differences.append(path + ":type")
        elif isinstance(left, dict):
            if left.keys() != right.keys():
                differences.append(path + ":keys")
            for key in sorted(left.keys() & right.keys()):
                walk(left[key], right[key], path + "/" + str(key))
        elif isinstance(left, list):
            if len(left) != len(right):
                differences.append(path + ":length")
            for index, (left_item, right_item) in enumerate(zip(left, right)):
                walk(left_item, right_item, path + "/" + str(index))
        elif left != right:
            differences.append(path + ":value")

    walk(expected, actual, "")
    return differences


def compare_responses(
    expected, actual, *, profile=None, route_id=None, rules_sha256=None,
    rules_path=None, request_id=None, diagnostic_context=None,
    request_path="", live=False,
):
    """Compare raw status/header/body pairs under an explicit profile.

    No profile keeps the Phase 1 byte gate. Phase 2 callers must supply a
    reviewed route ID and ledger digest; no rule is inferred from body fields.
    All golden, simulation, stability and staging callers use this boundary.
    """
    if profile == "byte" or (profile is None and "raw" in expected):
        return _compare_phase1_responses(expected, actual, request_path=request_path, profile="byte", live=live)
    if profile is None:
        return compare(
            capture(expected["status"], expected["headers"], expected["body"]),
            capture(actual["status"], actual["headers"], actual["body"]),
        )
    if profile not in {"phase2-meaning-v1", "phase1-meaning-v1"}:
        raise ContractError("unknown comparator profile")
    from scripts.rust_contract.phase2_meaning import compare_meaning

    return compare_meaning(
        expected, actual, route_id=route_id, rules_sha256=rules_sha256,
        rules_path=rules_path, request_id=request_id, profile=profile,
        diagnostic_context=diagnostic_context,
    )


def _news_item_bytes(raw: bytes) -> list[str] | None:
    """Slice original JSON tokens, retaining each item's spelling and key order."""
    text = raw.decode("utf-8")
    decoder = json.JSONDecoder()
    index = 0

    def whitespace(position):
        while position < len(text) and text[position].isspace():
            position += 1
        return position

    index = whitespace(index)
    if index >= len(text) or text[index] != "[":
        return None
    index = whitespace(index + 1)
    items = []
    while index < len(text) and text[index] != "]":
        start = index
        _, index = decoder.raw_decode(text, index)
        items.append(text[start:index])
        index = whitespace(index)
        if index < len(text) and text[index] == ",":
            index = whitespace(index + 1)
        else:
            break
    return items


def _approved_news_tie_rule(rules_path: Path) -> dict[str, Any]:
    """Load the tie rule file and refuse anything but the one reviewed rule.

    A missing, unreadable, malformed, or widened rule file stops the run. The
    relaxation is only ever the reviewed `/api/news` rule, so an edited file
    cannot quietly loosen another path.
    """
    try:
        rules = json.loads(rules_path.read_text())
    except (OSError, ValueError) as error:
        raise ContractError("news tie rule configuration is unreadable") from error
    if rules != APPROVED_NEWS_TIE_RULES:
        raise ContractError("unreviewed news tie rule configuration")
    return APPROVED_NEWS_TIE_RULES[0]


def compare_news_tie_groups(expected, actual, *, request_path, rules_path=None):
    """Decision 18: only news root arrays may permute exact tokens within ties."""
    from collections import Counter
    from itertools import groupby

    approved = _approved_news_tie_rule(rules_path or NEWS_TIE_RULES_PATH)
    if request_path != approved["request_path"] or not all(
        isinstance(item.get("body"), list) and item.get("status") == 200
        for item in (expected, actual)
    ):
        return compare(expected, actual)
    left, right = expected["body"], actual["body"]
    if not all(
        isinstance(item, dict) and "published_at" in item for item in left + right
    ):
        return compare(expected, actual)
    left_keys = [item["published_at"] for item in left]
    right_keys = [item["published_at"] for item in right]
    if left_keys != right_keys:
        return ["/body/published_at:sequence"]
    if not all(isinstance(key, str) for key in left_keys):
        return ["/body/published_at:type"]
    if left_keys != sorted(left_keys, reverse=True):
        # Decision 18: Rust must retain descending published-time order. Equal
        # key sequences are not enough: a matching ascending pair is a
        # difference, not a reviewed relaxation.
        return ["/body/published_at:order"]
    if "news_exact_items" not in expected or "news_exact_items" not in actual:
        return compare(expected, actual)

    def groups(envelope, keys):
        return [
            (key, Counter(token for _, token in values))
            for key, values in groupby(
                zip(keys, envelope["news_exact_items"]), key=lambda pair: pair[0]
            )
        ]

    if groups(expected, left_keys) != groups(actual, right_keys):
        return ["/body:tie-item-bytes"]
    ignored = {"body", "body_sha256", "news_exact_items"}
    return compare(
        {k: v for k, v in expected.items() if k not in ignored},
        {k: v for k, v in actual.items() if k not in ignored},
    )


PRICE_SUMMARY_RULES_PATH = ROOT / "tests/rust_contract/price-summary-paths.json"
APPROVED_PRICE_SUMMARY_RULES = [
    {
        "rule": "price-summary-exact-item-multiset",
        "request_path": "/api/prices",
        "json_path": "/body/summaries",
    }
]


def _object_array_tokens(raw: bytes, wanted_key: str):
    """Keep exact item tokens of one top-level array and the bytes around them.

    The full JSON is already validated, so this only slices the raw text: each
    item keeps its exact spelling and key order, and the frame keeps every byte
    outside the item positions.
    """
    text = raw.decode("utf-8")
    decoder = json.JSONDecoder()

    def skip(index):
        while index < len(text) and text[index].isspace():
            index += 1
        return index

    index = skip(0) + 1
    while text[skip(index)] != "}":
        index = skip(index)
        key, index = decoder.raw_decode(text, index)
        index = skip(index) + 1  # colon (the full JSON was already validated)
        start = skip(index)
        _, end = decoder.raw_decode(text, start)
        if key == wanted_key:
            spans = []
            cursor = skip(start + 1)
            while cursor < end - 1:
                _, stop = decoder.raw_decode(text, cursor)
                spans.append((cursor, stop))
                cursor = skip(stop)
                if text[cursor] == ",":
                    cursor = skip(cursor + 1)
                else:
                    break
            pieces = []
            cursor = 0
            for first, last in spans:
                pieces.append(text[cursor:first])
                cursor = last
            pieces.append(text[cursor:])
            return [text[first:last] for first, last in spans], pieces
        index = skip(end)
        if text[index] == ",":
            index += 1
    raise ContractError(f"the {wanted_key!r} array was not found")


def _summary_tokens(raw: bytes):
    """Keep exact summary tokens and all bytes outside their positions."""
    return _object_array_tokens(raw, "summaries")


def compare_price_summary_multiset(expected, actual, *, request_path, rules_path=None):
    from collections import Counter

    try:
        rules = json.loads((rules_path or PRICE_SUMMARY_RULES_PATH).read_text())
    except (OSError, ValueError) as error:
        raise ContractError("price summary rule configuration is unreadable") from error
    if rules != APPROVED_PRICE_SUMMARY_RULES:
        raise ContractError("unreviewed price summary rule configuration")
    if request_path != "/api/prices" or not all(
        item.get("status") == 200 and "price_summary_tokens" in item
        for item in (expected, actual)
    ):
        return compare_trade_tape_tie_groups(expected, actual, request_path=request_path)
    left_items, left_frame = expected["price_summary_tokens"]
    right_items, right_frame = actual["price_summary_tokens"]
    if Counter(left_items) != Counter(right_items):
        return ["/body/summaries:item-bytes"]
    if left_frame != right_frame:
        return ["/body:surrounding-bytes"]
    ignored = {"body", "body_sha256", "price_summary_tokens"}
    return compare(
        {k: v for k, v in expected.items() if k not in ignored},
        {k: v for k, v in actual.items() if k not in ignored},
    )


TRADE_TAPE_TIE_RULES_PATH = ROOT / "tests" / "rust_contract" / "trade-tape-tie-paths.json"
APPROVED_TRADE_TAPE_TIE_RULES = [
    {
        "rule": "trade-tape-confirmed-at-tie-multiset",
        "request_path": "/api/trade-tape",
        "json_path": "/body/items",
        "sort_key": "confirmed_at",
    }
]


def compare_trade_tape_tie_groups(expected, actual, *, request_path, rules_path=None):
    """PM decision (17 Oct): `/api/trade-tape` ties compare as a multiset.

    The two implementations copy the same tie-unstable `ORDER BY confirmed_at
    DESC`, so the order of items inside one `confirmed_at` tie group is source
    nondeterminism. Only the item bytes within a tie group may permute: the
    sequence of `confirmed_at` keys must match exactly, the keys must stay
    descending, the item **sets** must be identical, and every other byte
    (the frame, `total`, `market_hours`, headers) stays exact. A real ordering
    difference across tie groups still fails.
    """
    from collections import Counter
    from itertools import groupby

    try:
        rules = json.loads((rules_path or TRADE_TAPE_TIE_RULES_PATH).read_text())
    except (OSError, ValueError) as error:
        raise ContractError("trade-tape tie rule configuration is unreadable") from error
    if rules != APPROVED_TRADE_TAPE_TIE_RULES:
        raise ContractError("unreviewed trade-tape tie rule configuration")
    if request_path != "/api/trade-tape" or not all(
        item.get("status") == 200
        and isinstance(item.get("body"), dict)
        and "trade_tape_tokens" in item
        for item in (expected, actual)
    ):
        return compare_news_tie_groups(expected, actual, request_path=request_path)
    left_items, left_frame = expected["trade_tape_tokens"]
    right_items, right_frame = actual["trade_tape_tokens"]
    left_keys = [item["confirmed_at"] for item in expected["body"]["items"]]
    right_keys = [item["confirmed_at"] for item in actual["body"]["items"]]
    # The sequence of tie keys and their descending order are strict: only the
    # order *inside* a tie group may differ.
    if left_keys != right_keys:
        return ["/body/items/confirmed_at:sequence"]
    if left_keys != sorted(left_keys, reverse=True):
        return ["/body/items/confirmed_at:order"]

    def groups(tokens, keys):
        return [
            (key, Counter(token for _, token in values))
            for key, values in groupby(zip(keys, tokens), key=lambda pair: pair[0])
        ]

    if groups(left_items, left_keys) != groups(right_items, right_keys):
        return ["/body/items:tie-item-bytes"]
    if left_frame != right_frame:
        return ["/body/items:surrounding-bytes"]
    ignored = {"body", "body_sha256", "trade_tape_tokens"}
    return compare(
        {k: v for k, v in expected.items() if k not in ignored},
        {k: v for k, v in actual.items() if k not in ignored},
    )


# Live clock exceptions are schema paths, never recursive timestamp deletion.
LIVE_ROOT_CLOCK_ROUTES = frozenset({
    "/api/prices", "/api/prices/reference", "/api/orderbook/snapshot",
    "/api/curves/forward", "/api/curves/forward/table",
    "/api/curves/forward/slice", "/api/curves/forward/board",
})


def _live_clock_path(request_path: str, path: tuple[str, ...]) -> bool:
    if request_path == "/mcp#get_market_slice" and path == ("result", "structuredContent", "observed_at"):
        return True
    if request_path == "/mcp-slice-text" and path == ("observed_at",):
        return True
    if request_path in LIVE_ROOT_CLOCK_ROUTES and path == ("generated_at",):
        return True
    if request_path == "/api/curves/forward/slice":
        return path in {
            ("cell", "generated_at"),
            ("cell", "fair_price_band_provenance", "generated_at"),
            ("cell", "indication_summary", "provenance", "generated_at"),
            ("cell", "physical_stem_summary", "provenance", "generated_at"),
        } or (len(path) == 4 and path[0] in {"indications", "physical_stems"}
              and path[1].isdigit() and path[2:] == ("provenance", "generated_at"))
    if request_path == "/api/curves/forward/board":
        focus_curve_clock = (
            len(path) >= 5 and path[:2] == ("focus", "curve") and path[2].isdigit()
            and path[3:] in {("fair_price_band_provenance", "generated_at"),
                             ("fair_price_band", "provenance", "generated_at"),
                             ("indication_summary", "provenance", "generated_at"),
                             ("physical_stem_summary", "provenance", "generated_at")}
        )
        return focus_curve_clock or path in {("focus", "fair_price_band_provenance", "generated_at")} or (
            len(path) == 5 and path[:2] in {("focus", "indications"), ("focus", "physical_stems")}
            and path[2].isdigit() and path[3:] == ("provenance", "generated_at")
        ) or (len(path) >= 5 and path[0] == "ports" and path[1].isdigit()
              and path[2] == "cells" and path[3].isdigit()
              and path[4:] in {("fair_price_band_provenance", "generated_at"),
                              ("indication_summary", "provenance", "generated_at"),
                              ("physical_stem_summary", "provenance", "generated_at")})
    return False


def _live_bytes(response: dict[str, Any], request_path: str) -> bytes:
    """Replace only validated request-clock string tokens; keep all other bytes."""
    from datetime import datetime, timedelta

    raw = response["raw"]
    if not _is_json(strip_headers(response["headers"]).get("content-type", "")):
        return raw
    parse_json(raw)  # Reject ambiguous keys before walking raw tokens.
    text = raw.decode("utf-8")
    decoder = json.JSONDecoder()
    replacements = []

    def utc(value):
        if not isinstance(value, str) or not value.endswith(("Z", "+00:00")):
            raise ContractError("invalid live request clock")
        try:
            parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
        except ValueError as error:
            raise ContractError("invalid live request clock") from error
        if parsed.utcoffset() != timedelta(0):
            raise ContractError("invalid live request clock")
        return parsed

    def skip(index):
        while index < len(text) and text[index].isspace():
            index += 1
        return index

    def walk(index, path):
        index = skip(index)
        start = index
        value, end = decoder.raw_decode(text, index)
        if (request_path == "/mcp#get_market_slice" and len(path) == 4
                and path[:2] == ("result", "content") and path[2].isdigit() and path[3] == "text"):
            if not isinstance(value, str):
                raise ContractError("invalid SDK text content")
            if text[start:end] != json.dumps(value, ensure_ascii=False):
                raise ContractError("SDK text has noncanonical string escaping")
            nested = dict(response, raw=value.encode(), headers={"content-type": "application/json"})
            normalized = _live_bytes(nested, "/mcp-slice-text").decode()
            replacements.append((start, end, json.dumps(normalized, ensure_ascii=False)))
        elif _live_clock_path(request_path, path):
            clock = utc(value)
            if not utc(response.get("started_utc")) <= clock <= utc(response.get("ended_utc")):
                raise ContractError("live request clock outside call interval")
            replacements.append((start, end, '"<request-clock>"'))
        elif isinstance(value, dict):
            cursor = skip(index + 1)
            while text[cursor] != "}":
                key, cursor = decoder.raw_decode(text, cursor)
                cursor = walk(skip(cursor) + 1, path + (key,))
                cursor = skip(cursor)
                if text[cursor] == ",":
                    cursor = skip(cursor + 1)
        elif isinstance(value, list):
            cursor = skip(index + 1)
            for number in range(len(value)):
                cursor = skip(walk(cursor, path + (str(number),)))
                if text[cursor] == ",":
                    cursor = skip(cursor + 1)
        return end

    walk(0, ())
    for start, end, replacement in reversed(replacements):
        text = text[:start] + replacement + text[end:]
    return text.encode("utf-8")


def _compare_phase1_responses(expected, actual, *, request_path, profile="byte", live=False):
    """Authoritative Phase 1 wire comparison, with only reviewed exceptions.

    Raw response data and internal capture digests must remain in memory. Callers
    may persist only separately validated diagnostic paths and categories.
    """
    if profile != "byte":
        raise ContractError("Phase 1 requires the byte comparison profile")
    envelopes = []
    for response in (expected, actual):
        # Validate the original Content-Length before clock token replacement.
        capture(response["status"], response["headers"], response["raw"])
        raw = _live_bytes(response, request_path) if live else response["raw"]
        headers = {k: v for k, v in response["headers"].items() if k.lower() != "content-length"}
        envelopes.append(capture(response["status"], headers, raw))
    return compare_price_summary_multiset(*envelopes, request_path=request_path)


def recording_differences(
    expected: Sequence[dict[str, Any]], actual: Sequence[dict[str, Any]], *,
    profile=None, rules_sha256=None,
) -> list[str]:
    """Compare two recordings of one group. Empty means equal.

    Case counts and case ids are part of the contract: a shorter or reordered
    recording is a difference. Nothing is zipped silently.
    """
    if len(expected) != len(actual):
        return ["recording-length"]
    problems: list[str] = []
    for left, right in zip(expected, actual):
        if left["id"] != right["id"]:
            problems.append(f"{left['id']}:ordering")
            continue
        if profile is not None:
            from scripts.rust_contract.meaning_runner import compare_recorded_case
            problems.extend(f"{left['id']}:{path}" for path in compare_recorded_case(left, right, profile, rules_sha256))
            continue
        problems += [
            f"{left['id']}:{path}"
            for path in compare_price_summary_multiset(
                left["exchange"],
                right["exchange"],
                request_path=left.get("request", {}).get("path", ""),
            )
        ]
    return problems


def _declared_length(headers: Mapping[str, str]) -> int | None:
    for name, value in headers.items():
        if str(name).lower() == "content-length":
            try:
                return int(value)
            except ValueError as error:
                raise ContractError("Content-Length is not an integer") from error
    return None


def _is_json(content_type: str) -> bool:
    media_type = content_type.split(";", 1)[0].strip().lower()
    return media_type == "application/json" or media_type.endswith("+json")


def capture(
    status: int, headers: Mapping[str, str], raw: bytes, *, expect_body: bool = True
) -> dict[str, Any]:
    """Build the compared envelope for one response.

    Transport headers are dropped, Content-Length is checked against the body
    length independently, JSON bodies are parsed without losing decimal scale,
    and every body keeps its exact byte digest. `expect_body` is false for a
    HEAD response, which carries no body to measure.
    """
    if expect_body:
        declared = _declared_length(headers)
        if declared is not None and declared != len(raw):
            raise ContractError("declared Content-Length does not match the body")
    kept = strip_headers(headers)
    if not raw:
        # An empty body is not JSON, whatever the declared content type.
        body: Any = None
    elif _is_json(kept.get("content-type", "")):
        body = parse_json(raw)
    else:
        # Non-JSON bodies (for example CSV) are kept verbatim. The digest in
        # the envelope is the byte-exact comparison; the text aids diagnosis.
        body = {"raw": raw.decode("utf-8", "surrogateescape")}
    envelope = {
        "status": int(status),
        "headers": kept,
        "body": body,
        "body_sha256": hashlib.sha256(raw).hexdigest(),
    }
    if isinstance(body, list) and all(
        isinstance(item, dict) and "published_at" in item for item in body
    ):
        envelope["news_exact_items"] = _news_item_bytes(raw)
    if isinstance(body, dict) and isinstance(body.get("summaries"), list):
        envelope["price_summary_tokens"] = _summary_tokens(raw)
    if (
        isinstance(body, dict)
        and isinstance(body.get("items"), list)
        and body["items"]
        and all(isinstance(item, dict) and "confirmed_at" in item for item in body["items"])
    ):
        envelope["trade_tape_tokens"] = _object_array_tokens(raw, "items")
    return envelope


def attest(base_url: str, token: str) -> dict[str, Any]:
    """Prove the endpoint is the attested disposable target before requesting."""
    request = urllib.request.Request(base_url.rstrip("/") + IDENTITY_PATH, method="GET")
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            raw = response.read()
    except (urllib.error.URLError, OSError) as error:
        raise ContractError("endpoint handshake failed") from error
    try:
        body = parse_json(raw)
    except (ContractError, ValueError) as error:
        raise ContractError("endpoint identity is not JSON") from error
    if not isinstance(body, dict) or body.get("disposable") is not True:
        raise ContractError("endpoint did not attest a disposable target")
    if body.get("token") != token:
        raise ContractError("endpoint token mismatch")
    return body


def load_cases(path: Path, group: str) -> list[dict[str, Any]]:
    """Load one group's cases. Unknown groups and empty groups fail."""
    try:
        document = json.loads(path.read_text())
    except (OSError, ValueError) as error:
        raise ContractError("corpus cases are unreadable") from error
    groups = document.get("groups") if isinstance(document, dict) else None
    if not isinstance(groups, dict) or group not in groups:
        raise ContractError(f"unknown contract group {group!r}")
    cases = groups[group]
    if not isinstance(cases, list) or not cases:
        raise ContractError(f"contract group {group!r} has no cases")
    return cases


def _require_environment(name: str) -> str:
    value = os.environ.get(name, "")
    if not value:
        raise ContractError(f"{name} is required")
    return value


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--group", required=True)
    parser.add_argument("--cases", type=Path, default=CASES_PATH)
    parser.add_argument("--profile", default=os.environ.get("RUST_CONTRACT_PROFILE"))
    parser.add_argument("--rules-sha256", default=os.environ.get("MEANING_RULES_SHA256"))
    args = parser.parse_args(argv)

    from scripts.rust_contract.record import record

    try:
        python_url = _require_environment("PYTHON_CONTRACT_URL")
        rust_url = _require_environment("RUST_CONTRACT_URL")
        token = _require_environment("DISPOSABLE_TEST_TOKEN")
        attest(python_url, token)
        attest(rust_url, token)
        cases = load_cases(args.cases, args.group)
        expected = record(python_url, token, cases)
        actual = record(rust_url, token, cases)
    except ContractError as error:
        print(f"contract setup failed: {error}", file=sys.stderr)
        return 2

    if len(expected) != len(actual):
        print(
            f"contract group {args.group} failed: recorded case counts differ "
            f"({len(expected)} expected, {len(actual)} actual)",
            file=sys.stderr,
        )
        return 1
    problems = recording_differences(expected, actual, profile=args.profile, rules_sha256=args.rules_sha256)
    for problem in problems:
        # Schema path and category only. Never a body value.
        print(problem, file=sys.stderr)
    if problems:
        print(
            f"contract group {args.group} failed: {len(problems)} difference(s)",
            file=sys.stderr,
        )
        return 1
    print(f"contract group {args.group} OK: {len(cases)} case(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
