"""False-pass and safety tests for the shadow harness.

None of these tests contacts a host. They exercise the pure safety rules that
decide what the harness may send and what it may persist.
"""

import json
from pathlib import Path

import pytest

from scripts.rust_contract import shadow

PORT = "1379d36c-1ca9-55b7-9c0d-5235a0ba1f36"
HASH = "a" * 64


def _manifest():
    from scripts.rust_contract import workload

    return workload.load()


# ------------------------------------------------------------- target safety


@pytest.mark.parametrize(
    "url",
    [
        "https://api.verdaxis.exchange",
        "https://app.verdaxis.exchange/api",
        "http://169.58.37.164:8000",
        "http://verdaxis.exchange",
    ],
)
def test_production_urls_are_refused(url):
    assert shadow.is_production_url(url)
    with pytest.raises(shadow.ShadowError):
        shadow.assert_not_production(url)


@pytest.mark.parametrize("url", ["http://127.0.0.1:8001", "http://localhost:8011"])
def test_loopback_targets_are_allowed(url):
    assert not shadow.is_production_url(url)
    shadow.assert_not_production(url)


def test_config_requires_explicit_urls(tmp_path):
    config = tmp_path / "staging.toml"
    config.write_text('sampling_calls_per_ten_seconds = 3\n')
    with pytest.raises(shadow.ShadowError, match="python_url"):
        shadow.load_config(config)


def test_config_refuses_a_production_target(tmp_path):
    config = tmp_path / "staging.toml"
    config.write_text(
        'python_url = "http://127.0.0.1:8001"\n'
        'rust_url = "https://api.verdaxis.exchange"\n'
    )
    with pytest.raises(shadow.ShadowError, match="production"):
        shadow.load_config(config)


def test_config_reads_the_reviewed_template(tmp_path):
    template = Path("scripts/rust_contract/staging.example.toml")
    loaded = shadow.load_config(template)
    assert loaded.sampling_calls_per_ten_seconds == 3
    assert loaded.per_call_deadline_seconds == 2.0
    assert loaded.max_response_bytes == 1024 * 1024
    assert loaded.price_minute_limits == {
        "prices_per_minute": 60,
        "reference_per_minute": 30,
        "export_per_minute": 10,
    }


# ------------------------------------------------------------- header safety


@pytest.mark.parametrize("name", ["Authorization", "Cookie", "X-Api-Key", "Proxy-Authorization"])
def test_credential_headers_are_refused(name):
    with pytest.raises(shadow.ShadowError):
        shadow.assert_safe_headers({name: "value"})


def test_plain_headers_are_allowed():
    shadow.assert_safe_headers({"x-request-id": "shadow-r16", "host": "127.0.0.1"})


def test_no_case_carries_a_credential():
    for case in shadow.safe_cases(_manifest()):
        # The case list is corpus-derived; no case supplies a credential header.
        assert "authorization" not in {k.lower() for k in (case.get("headers") or {})}


# ------------------------------------------------------------ request safety


def test_only_read_only_methods_are_sent():
    for case in shadow.safe_cases(_manifest()):
        if case["kind"] == "route":
            assert case["method"] == "GET"
        else:
            assert case["method"] == "POST"
            assert case["path"] == "/mcp"
            # A read-only MCP tool call, never initialize or a mutation.
            body = json.dumps(case["body"])
            assert "tools/call" in body


def test_timeout_is_inconclusive_never_a_pass():
    config = shadow.ShadowConfig(
        python_url="http://127.0.0.1:1", rust_url="http://127.0.0.1:1", per_call_deadline_seconds=0.2
    )
    case = {"id": "R16", "method": "GET", "path": "/api/orderbook/bids", "query": ""}
    result = shadow._one_call(config, "http://127.0.0.1:1", case)
    assert result.get("inconclusive") in {"timeout", "unreachable"}
    assert "status" not in result or result["status"] is None


def test_busy_status_is_not_economic_coverage():
    assert not shadow.status_pair_ok(429)
    assert not shadow.status_pair_ok(503)
    assert shadow.status_pair_ok(200)


# ------------------------------------------------------- sandwich classification


def observation(raw=b'{"n":1}', status=200, headers=None):
    return {"status": status, "headers": headers or {"content-type": "application/json"},
            "raw": raw, "bytes": len(raw), "duration_ms": 1.0,
            "started_utc": "2026-10-18T00:00:00Z", "ended_utc": "2026-10-18T01:00:00Z"}


def test_data_change_is_inconclusive_not_a_match():
    first = observation()
    third = observation(b'{"n":2}')
    assert shadow.classify_sandwich(first, first, third, request_path="/api/news") == "data_changed"


def test_rust_mismatch_is_a_difference():
    first = observation()
    rust = observation(b'{"n":9}')
    assert len(first["raw"]) == len(rust["raw"])
    assert shadow.classify_sandwich(first, rust, first, request_path="/api/news") == "difference"


def test_matching_sandwich_is_a_match():
    same = observation()
    assert shadow.classify_sandwich(same, dict(same), dict(same), request_path="/api/news") == "match"


def test_sandwich_uses_the_authoritative_byte_comparator(monkeypatch):
    from scripts.rust_contract import compare

    calls = []
    def authoritative(left, right, *, request_path, profile, live):
        calls.append((request_path, profile, live))
        return ["/body:value"] if len(calls) == 2 else []
    monkeypatch.setattr(compare, "compare_responses", authoritative, raising=False)
    same = observation()
    assert shadow.classify_sandwich(same, same, same, request_path="/api/news") == "difference"
    assert calls == [("/api/news", "byte", True)] * 2


@pytest.mark.parametrize("left,right", [
    (b'{"n":1.0}', b'{"n":1.00}'),
    (b'{"n":null}', b'{}'),
    (b'[1,2]', b'[2,1]'),
    (b'{"a":1,"b":2}', b'{"b":2,"a":1}'),
    (b'a,b\\n1,2\\n', b'a,b\\r\\n1,2\\r\\n'),
])
def test_byte_profile_preserves_wire_content(left, right):
    from scripts.rust_contract import compare

    headers = {"content-type": "text/csv"} if left.startswith(b'a,b') else None
    assert compare.compare_responses(observation(left, headers=headers), observation(right, headers=headers),
                                     request_path="/api/orderbook", profile="byte", live=True)


def test_decision_85_permits_only_exact_items_inside_confirmed_time_ties():
    from scripts.rust_contract import compare

    a = b'{"confirmed_at":"2026-10-18T00:00:00Z","price":"1.00"}'
    b = b'{"confirmed_at":"2026-10-18T00:00:00Z","price":"2.00"}'
    first = observation(b'{"items":[' + a + b',' + b + b'],"total":2}')
    swapped = observation(b'{"items":[' + b + b',' + a + b'],"total":2}')
    assert not compare.compare_responses(first, swapped, request_path="/api/trade-tape", profile="byte", live=True)
    changed = observation(swapped["raw"].replace(b'2.00', b'2.01'))
    assert compare.compare_responses(first, changed, request_path="/api/trade-tape", profile="byte", live=True)
    assert compare.compare_responses(first, swapped, request_path="/api/orderbook", profile="byte", live=True)


def test_only_reviewed_generation_time_is_ignored():
    from scripts.rust_contract import compare

    first = observation(b'{"generated_at":"2026-10-18T00:00:00Z","observed_at":"2026-10-17T00:00:00Z"}')
    later = observation(first["raw"].replace(b'18T00', b'18T01'))
    assert not compare.compare_responses(first, later, request_path="/api/prices", profile="byte", live=True)
    stored = observation(first["raw"].replace(b'17T00', b'17T01'))
    assert compare.compare_responses(first, stored, request_path="/api/prices", profile="byte", live=True)
    assert compare.compare_responses(first, later, request_path="/api/news", profile="byte", live=True)


def test_mcp_text_escaping_is_not_silently_normalized():
    from scripts.rust_contract import compare

    inner = json.dumps({"observed_at": "2026-10-18T00:00:00Z", "x": "a"})
    raw = json.dumps({"result": {"content": [{"type": "text", "text": inner}]}}).encode()
    raw = raw.replace(b'Z', b'\\u005a')
    # This encoded JSON must either remain a difference or fail closed; it must
    # not gain equality by re-encoding an unrelated escaped value.
    item = observation(raw)
    with pytest.raises(compare.ContractError):
        compare.compare_responses(item, item, request_path="/mcp#get_market_slice", live=True)


def test_generation_clock_must_be_utc_and_inside_call_interval():
    from scripts.rust_contract import compare

    for value in ("not-a-time", "2026-10-17T00:00:00Z", "2026-10-18T08:00:00+08:00"):
        item = observation(json.dumps({"generated_at": value}).encode())
        with pytest.raises(compare.ContractError):
            compare.compare_responses(item, item, request_path="/api/prices", profile="byte", live=True)


def test_mcp_slice_clock_normalizes_both_sdk_representations_only():
    from scripts.rust_contract import compare

    def response(hour):
        data = {"observed_at": f"2026-10-18T{hour}:00:00Z", "bid_order_count": 1}
        result = {"content": [{"type": "text", "text": json.dumps(data)}], "structuredContent": data, "isError": False}
        return observation(json.dumps({"jsonrpc": "2.0", "id": 8, "result": result}).encode())
    first, second = response("00"), response("01")
    assert not compare.compare_responses(first, second, request_path="/mcp#get_market_slice", profile="byte", live=True)
    assert compare.compare_responses(first, second, request_path="/mcp#list_markets", profile="byte", live=True)
    changed = observation(second["raw"].replace(b'count\\": 1', b'count\\": 2'))
    assert compare.compare_responses(first, changed, request_path="/mcp#get_market_slice", profile="byte", live=True)


# ------------------------------------------------------------------- coverage


def test_missing_coverage_is_inconclusive_with_the_number_needed():
    result = shadow.coverage({"R16": 400, "R17": 1000}, 1000)
    assert result["routes"]["R16"] == {"outcome": "inconclusive", "pairs": 400, "needed": 600}
    assert result["routes"]["R17"] == {"outcome": "ok", "pairs": 1000}
    assert result["inconclusive"] == ["R16"]


def test_zero_traffic_is_not_zero_differences():
    result = shadow.coverage({"R16": 0}, 1000)
    assert result["routes"]["R16"]["outcome"] == "inconclusive"
    assert result["routes"]["R16"]["needed"] == 1000


def test_a_route_with_only_empty_observations_is_inconclusive():
    result = shadow.coverage({"R16": 2000}, 1000, nonempty={"R16": False})
    assert result["routes"]["R16"]["outcome"] == "inconclusive"
    assert result["routes"]["R16"]["reason"] == "no nonempty observation"


def test_a_skipped_route_is_never_a_pass():
    # A route absent from the pairs map simply is not in the outcome set; a
    # caller that reports it as covered would be claiming an unrun case.
    result = shadow.coverage({"R16": 1000}, 1000)
    assert "R99" not in result["routes"]
    assert result["inconclusive"] == []


# ------------------------------------------------------------------ persistence


def test_sanitize_keeps_only_allowed_fields():
    record = {
        "candidate_sha": "b" * 40,
        "route_id": "R16",
        "case_id": "R16-bids",
        "status_pair": "200/200/200",
        "field_path": "/body/items/0/price_per_mt_usd",
        "difference_category": "value",
        "byte_counts": [100, 100],
        "durations_ms": [12.5, 11.0, 12.1],
        "counters": {"match": 1},
        "notes": "dropped",
    }
    sanitized = shadow.sanitize_diagnostic(record)
    assert "notes" not in sanitized
    assert sanitized["route_id"] == "R16"


def test_sanitize_refuses_a_row_id():
    with pytest.raises(shadow.ShadowError):
        shadow.sanitize_diagnostic({"route_id": "R16", "status_pair": PORT})


def test_sanitize_refuses_a_difference_value():
    with pytest.raises(shadow.ShadowError, match="must not carry"):
        shadow.sanitize_diagnostic({"route_id": "R16", "difference_value": "600.00 vs 599.00"})
    with pytest.raises(shadow.ShadowError, match="must not carry"):
        shadow.sanitize_diagnostic({"route_id": "R16", "body": "{\"price\":\"600.00\"}"})


def test_sanitize_refuses_a_payload_hash():
    with pytest.raises(shadow.ShadowError, match="hash"):
        shadow.sanitize_diagnostic({"route_id": HASH})
    # policy_hash and candidate_sha are declared identities, not payload hashes.
    shadow.sanitize_diagnostic({"policy_hash": HASH})


def test_sanitize_refuses_a_raw_url_with_a_query():
    with pytest.raises(shadow.ShadowError, match="URL"):
        shadow.sanitize_diagnostic({"case_id": "http://127.0.0.1:8001/api/news?category=bunkers"})


def test_sanitize_refuses_a_credential():
    with pytest.raises(shadow.ShadowError, match="credential"):
        shadow.sanitize_diagnostic({"status_pair": "Bearer abcdefghijklmnop"})


def test_sanitize_refuses_a_non_schema_field_path():
    with pytest.raises(shadow.ShadowError, match="schema path"):
        shadow.sanitize_diagnostic({"field_path": "body.items.0.price"})
    with pytest.raises(shadow.ShadowError):
        shadow.sanitize_diagnostic({"field_path": "/body/" + PORT})


def test_sanitize_refuses_an_unknown_difference_category():
    with pytest.raises(shadow.ShadowError, match="category"):
        shadow.sanitize_diagnostic({"difference_category": "secret"})


# ------------------------------------------------------------- deploy state


def test_deploy_state_presence_blocks_the_harness(tmp_path):
    state = tmp_path / "staging.state"
    state.write_text("blocked")
    assert shadow.deploy_state_present(str(state))
    config = shadow.ShadowConfig(
        python_url="http://127.0.0.1:8001",
        rust_url="http://127.0.0.1:8011",
        deploy_state_path=str(state),
    )
    with pytest.raises(shadow.ShadowPaused):
        shadow._death_line(config)


def test_a_dangling_symlink_still_blocks_the_harness(tmp_path):
    state = tmp_path / "staging.state"
    state.symlink_to(tmp_path / "gone")
    assert shadow.deploy_state_present(str(state))


def test_absent_deploy_state_does_not_block(tmp_path):
    assert not shadow.deploy_state_present(str(tmp_path / "absent.state"))
    assert not shadow.deploy_state_present("")
