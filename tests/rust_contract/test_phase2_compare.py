"""Adversarial same-meaning vectors: formatting is not permission to lose data."""

import hashlib
import json
from pathlib import Path

import pytest

from scripts.rust_contract import compare

RULES = Path(__file__).parent / "phase2/normalization.json"
STAMP = "2026-10-18T10:00:00.123456Z"


def response(body, *, status=200, headers=None):
    kept = {"content-type": "application/json", "x-request-id": "00000000-0000-4000-8000-000000000001"}
    kept.update({key.lower(): value for key, value in (headers or {}).items()})
    return {"status": status, "headers": kept, "body": body if isinstance(body, bytes) else json.dumps(body).encode()}


def differences(left, right, route="P01", **options):
    assert hasattr(compare, "compare_responses"), "missing explicit Phase 2 profile entry point"
    assert RULES.is_file(), "missing reviewed normalization ledger"
    return compare.compare_responses(
        left, right, profile="phase2-meaning-v1", route_id=route,
        rules_sha256=hashlib.sha256(RULES.read_bytes()).hexdigest(), **options,
    )


@pytest.mark.parametrize("left,right", [
    (b'{"z":1,"a":[null,true]}', b' { "a": [null, true], "z": 1.00 } '),
    (b'1', b'1e0'), (b'123456789012345678901234567890.1', b'123456789012345678901234567890.10'),
])
def test_formatting_and_exact_json_numbers_match(left, right):
    assert differences(response(left), response(right)) == []


@pytest.mark.parametrize("left,right", [
    (True, 1), ({}, {"a": None}), ({"s": "1.00"}, {"s": "1"}),
    ({"s": "a "}, {"s": "a"}), ({"s": "é"}, {"s": "é"}),
    (b'100000000000000000000000000000.1', b'100000000000000000000000000000.2'),
])
def test_real_json_differences_survive(left, right):
    assert differences(response(left), response(right))


def test_only_declared_alert_decimal_string_paths_normalize():
    assert differences(response({"threshold_usd": "750.00"}), response({"threshold_usd": "750"}), "A01") == []
    assert differences(response([{"threshold_usd": "750.00", "created_at": STAMP}]), response([{"threshold_usd": 750, "created_at": STAMP}]), "A02")
    assert differences(response({"threshold_usd": "750.00"}), response({"threshold_usd": "750.01"}), "A01")
    assert differences(response({"threshold_usd": "750.00"}), response({"threshold_usd": "750"}), "P01")


def test_typed_timestamp_exact_instant_and_no_tolerance():
    assert differences(response({"updated_at": STAMP}), response({"updated_at": "2026-10-18T12:00:00.123456+02:00"}), "P02") == []
    assert differences(response({"updated_at": STAMP}), response({"updated_at": "2026-10-18T10:00:00.123457+00:00"}), "P02")
    assert differences(response({"updated_at": STAMP}), response({"updated_at": None}), "P02")
    assert differences(response({"value": {"created_at": STAMP}}), response({"value": {"created_at": STAMP.replace("Z", "+00:00")}}), "P02")


@pytest.mark.parametrize("raw", [b'{"a":1,"a":1}', b'{"x":{"a":1,"a":2}}', b'"\xff"', b'NaN', b'Infinity', b'-Infinity', b''])
def test_invalid_json_is_invalid_evidence_on_either_side(raw):
    with pytest.raises(compare.ContractError):
        differences(response(raw), response({}))
    with pytest.raises(compare.ContractError):
        differences(response({}), response(raw))


@pytest.mark.parametrize("route,body", [
    ("A01", {"threshold_usd": "NaN"}), ("A01", {"threshold_usd": "Infinity"}),
    ("A01", {"threshold_usd": " 750 "}), ("P02", {"updated_at": "2026-10-18T10:00:00"}),
    ("P02", {"updated_at": "2026-10-18T10:00:00.1234567Z"}),
    ("P02", {"updated_at": "2026-02-30T10:00:00Z"}),
])
def test_invalid_typed_wire_values_are_invalid_evidence(route, body):
    with pytest.raises(compare.ContractError):
        differences(response(body), response(body), route)


def notice(identity, stamp=STAMP):
    return {"id": identity, "created_at": stamp, "data": None}


def test_approved_contiguous_ties_keep_items_multiplicity_and_page_membership():
    a, b, c = notice("a"), notice("b"), notice("c", "2026-10-17T10:00:00Z")
    assert differences(response([a, b, c]), response([b, a, c]), "N01") == []
    for changed in ([a, c], [a, a, c], [a, {**b, "data": {}}, c], [a, c, b], [a, notice("different-page-member"), c]):
        assert differences(response([a, b, c]), response(changed), "N01")
    assert differences(response([c, a]), response([c, a]), "N01")


def test_hidden_notification_fields_are_never_discarded():
    a = notice("a")
    assert differences(response([a]), response([{**a, "data": {"_email_secret": "private"}}]), "N01")
    assert differences(response([a]), response([{**a, "_email_secret": "private"}]), "N01")


def test_watchlist_entries_have_only_the_reviewed_multiset_exception():
    a = {"id": "w", "created_at": STAMP, "entries": [notice("a"), notice("b")]}
    b = {**a, "entries": list(reversed(a["entries"]))}
    assert differences(response([a]), response([b]), "W01") == []
    assert differences(response([a]), response([{**b, "entries": [notice("a"), notice("a")]}]), "W01")


def test_nested_slice_and_pin_ties_keep_all_fields():
    pins = [notice("p1") | {"latest_event_at": None}, notice("p2") | {"latest_event_at": None}]
    slices = [notice("s1") | {"unread_event_count": 1, "latest_event_at": None, "pins": pins}, notice("s2") | {"unread_event_count": 2, "latest_event_at": None, "pins": []}]
    changed = [slices[1], {**slices[0], "pins": list(reversed(pins))}]
    assert differences(response({"slices": slices}), response({"slices": changed}), "W04") == []
    assert differences(response({"slices": slices}), response({"slices": [{**slices[0], "unread_event_count": 3}, slices[1]]}), "W04")


def test_leaderboard_retains_rank_and_tenth_place_membership():
    left = [{"user_name": "a", "rank": 1, "referral_count": 5}, {"user_name": "b", "rank": 2, "referral_count": 5}]
    assert differences(response(left), response(list(reversed(left))), "R03") == []
    assert differences(response(left), response([{**left[1], "rank": 1}, {**left[0], "rank": 2}]), "R03")
    assert differences(response(left), response([left[0], {**left[1], "user_name": "different"}]), "R03")


def test_feed_payloads_and_opaque_cursors_stay_exact():
    a, b = notice("a"), notice("b")
    assert differences(response({"items": [a, b]}), response({"items": [b, a]}), "W07")
    assert differences(response({"event_payload": {"array": [1, 2]}}), response({"event_payload": {"array": [2, 1]}}), "W08")
    assert differences(response({"next_cursor": STAMP + "|uuid"}), response({"next_cursor": STAMP.replace("Z", "+00:00") + "|uuid"}), "W07")


@pytest.mark.parametrize("header", ["www-authenticate", "retry-after", "access-control-allow-origin", "location", "allow"])
def test_contract_header_values_and_absence_are_exact(header):
    left = response({}, headers={header: "a"})
    assert differences(left, response({}, headers={header: "b"}))
    assert differences(left, response({}))


def test_content_type_is_compared_and_missing_or_non_json_is_invalid():
    assert differences(response({}), response({}, headers={"Content-Type": "application/json; charset=utf-8"}))
    for value in ("", "text/plain"):
        with pytest.raises(compare.ContractError):
            differences(response({}), response({}, headers={"Content-Type": value}))


def test_status_empty_204_and_error_envelopes():
    assert differences(response({}, status=401), response({}, status=403))
    assert differences(response({"detail": "a"}, status=422), response({"detail": "b"}, status=422))
    assert differences(response(b'', status=204), response(b'', status=204), "W06") == []
    with pytest.raises(compare.ContractError):
        differences(response(b'null', status=204), response(b'null', status=204), "W06")


def test_request_ids_are_validated_and_only_generated_ids_substituted():
    left = response({})
    right = response({}, headers={"X-Request-ID": "00000000-0000-4000-8000-000000000002"})
    assert differences(left, right) == []
    assert differences(left, right, request_id="00000000-0000-4000-8000-000000000001")
    with pytest.raises(compare.ContractError):
        differences(left, response({}, headers={"X-Request-ID": "not-a-generated-uuid"}))


@pytest.mark.parametrize("route,field,container", [
    ("F02", "created_at", "items"), ("A02", "created_at", None),
    ("R02", "signed_up_at", "referrals"),
])
def test_remaining_approved_tie_paths(route, field, container):
    a, b = {"id": "a", field: STAMP}, {"id": "b", field: STAMP}
    left, right = [a, b], [b, a]
    if container:
        left, right = {container: left}, {container: right}
    assert differences(response(left), response(right), route) == []


def test_error_validation_locations_values_and_typed_looking_strings_stay_exact():
    left = {"detail": [{"loc": ["body", "threshold_usd"], "msg": "invalid", "input": "750.00"}]}
    right = {"detail": [{"loc": ["body", "threshold_usd"], "msg": "invalid", "input": "750"}]}
    assert differences(response(left, status=422), response(right, status=422), "A01")
    assert differences(response({"updated_at": STAMP}, status=422), response({"updated_at": STAMP.replace("Z", "+00:00")}, status=422), "P02")


def test_arbitrary_payload_fields_keep_string_meaning_and_array_order():
    for route, field in (("N01", "data"), ("W08", "event_payload")):
        left = {field: {"threshold_usd": "750.00", "created_at": STAMP}}
        right = {field: {"threshold_usd": "750", "created_at": STAMP.replace("Z", "+00:00")}}
        if route == "N01":
            left, right = [notice("a") | left], [notice("a") | right]
        assert differences(response(left), response(right), route)


def test_header_name_case_transport_headers_and_length_validation():
    left = response({})
    right = {**left, "headers": {key.upper(): value for key, value in left["headers"].items()}}
    right["headers"]["DATE"] = "ignored transport value"
    assert differences(left, right) == []
    with pytest.raises(compare.ContractError):
        differences(left, response({}, headers={"content-length": "999"}))
    duplicate = {**left, "headers": {**left["headers"], "Content-Type": "application/json"}}
    with pytest.raises(compare.ContractError):
        differences(left, duplicate)


def test_equal_but_wrong_supplied_request_id_never_passes():
    assert differences(response({}), response({}), request_id="harness-request")
    assert differences(response({}, headers={"x-request-id": "harness-request"}), response({}, headers={"x-request-id": "harness-request"}), request_id="harness-request") == []


def test_unknown_profile_route_digest_and_widened_rule_are_fatal(tmp_path):
    assert hasattr(compare, "compare_responses"), "missing explicit Phase 2 profile entry point"
    for options in ({"profile": "unknown"}, {"profile": "phase2-meaning-v1", "route_id": "UNKNOWN"}, {"profile": "phase2-meaning-v1", "route_id": "P01", "rules_sha256": "0" * 64}):
        with pytest.raises(compare.ContractError):
            compare.compare_responses(response({}), response({}), **options)
    ledger = json.loads(RULES.read_text())
    ledger["routes"]["P01"]["datetime_paths"] = ["/*/created_at"]
    path = tmp_path / "rules.json"
    path.write_text(json.dumps(ledger))
    with pytest.raises(compare.ContractError):
        compare.compare_responses(response({}), response({}), profile="phase2-meaning-v1", route_id="P01", rules_path=path, rules_sha256=hashlib.sha256(path.read_bytes()).hexdigest())
