"""Staging shadow control-loop checks; never contact deployed services."""
import json
from dataclasses import replace

import pytest

from scripts.rust_contract import shadow
from tests.rust_contract.test_shadow import observation


def config(tmp_path):
    return shadow.ShadowConfig(
        python_url="http://127.0.0.1:8001", rust_url="http://127.0.0.1:8011",
        python_sha="a" * 40, candidate_sha="b" * 40,
        deploy_state_path=str(tmp_path / "staging.state"),
        output_dir=str(tmp_path / "private"), required_observations_per_route=1,
        floor_approved=True,
    )


def case(identifier="R05", path="/api/news"):
    return {"id": identifier, "case_id": identifier + "-public", "kind": "route",
            "path": path, "method": "GET", "query": "", "group": "simple"}


class Clock:
    def __init__(self):
        self.now = 0.0
    def monotonic(self):
        return self.now
    def sleep(self, seconds):
        self.now += seconds


@pytest.mark.parametrize("changes", [
    {"python_url": "http://localhost:8001"},
    {"rust_url": "http://127.0.0.1:8000"},
    {"python_url": "http://127.0.0.1:8001/?x=1"},
    {"sampling_calls_per_ten_seconds": 4},
    {"per_call_deadline_seconds": 2.1}, {"max_response_bytes": 1048577},
    {"pause_seconds": 59}, {"required_observations_per_route": 0},
    {"python_sha": ""}, {"candidate_sha": "head"},
    {"deploy_state_path": ""},
    {"price_minute_limits": {"export_per_minute": 11}},
])
def test_invalid_staging_configuration_is_refused(tmp_path, changes):
    with pytest.raises(shadow.ShadowError):
        shadow.validate_config(replace(config(tmp_path), **changes))


def test_staging_host_required_before_network(tmp_path, monkeypatch):
    monkeypatch.setattr(shadow.socket, "gethostname", lambda: "vmi3623757")
    with pytest.raises(shadow.ShadowError, match="staging host"):
        shadow.run(config(tmp_path), 30, cases=[case()])


def test_private_cases_replace_fixture_defaults(tmp_path):
    rows = [case("R01", "/api/ports"), case("R02", "/api/ports/NLRTM")]
    path = tmp_path / "cases.json"
    path.write_text(json.dumps(rows))
    loaded = shadow.load_staging_cases(path, require_complete=False)
    assert loaded[1]["path"].endswith("NLRTM")
    for unsafe in ["/__contract__/echo", "/api/auth/me", "http://example.com", "/api/ports/T4B_ONE"]:
        path.write_text(json.dumps([case("R02", unsafe)]))
        with pytest.raises(shadow.ShadowError):
            shadow.load_staging_cases(path, require_complete=False)
    path.write_text(json.dumps(rows))
    with pytest.raises(shadow.ShadowError, match="coverage"):
        shadow.load_staging_cases(path)


def test_rolling_global_and_minute_limits_count_both_python_calls():
    clock = Clock()
    pacer = shadow.CallPacer(clock=clock.monotonic, sleep=clock.sleep)
    starts = []
    for _ in range(12):
        pacer.wait("python", "export", 2, deadline=400)
        starts.append(clock.now)
    for start in starts:
        assert sum(start <= t < start + 10 for t in starts) <= 3
        assert sum(start <= t < start + 60 for t in starts) <= 2
    assert starts[2] >= 60


def setup_run(tmp_path, monkeypatch, responses):
    clock = Clock()
    monkeypatch.setattr(shadow.time, "monotonic", clock.monotonic)
    monkeypatch.setattr(shadow.time, "sleep", clock.sleep)
    monkeypatch.setattr(shadow.socket, "gethostname", lambda: "vmi1840561")
    monkeypatch.setattr(shadow, "memory_pressure", lambda: False)
    calls = []
    def fetch(cfg, base, request):
        calls.append((clock.now, base, request["id"]))
        if request["path"] == "/health/live":
            return observation(b'{"status":"ok"}')
        if request["path"] in {"/health/ready", "/health"}:
            sha = cfg.python_sha if base == cfg.python_url else cfg.candidate_sha
            return observation(json.dumps({"status": "ok", "db": "ok", "environment": "staging", "release_sha": sha}).encode())
        return responses.pop(0) if responses else observation(b'[{"n":1}]')
    monkeypatch.setattr(shadow, "_one_call", fetch)
    return config(tmp_path), calls, clock


def test_run_forced_equal_length_content_difference_fails(tmp_path, monkeypatch):
    cfg, calls, _ = setup_run(tmp_path, monkeypatch, [observation(), observation(b'{"n":9}'), observation()])
    result = shadow.run(cfg, 100, cases=[case()])
    assert result["outcome"] == "FAIL"
    assert result["counters"]["difference"] == 1
    assert result["pairs"]["R05"] == 0
    assert shadow.result_exit_code(result) == 1
    assert len([c for c in calls if c[2] == "R05"]) == 3
    text = json.dumps(result)
    assert '"raw"' not in text and 'sha256' not in text and '"n":9' not in text


def test_run_data_change_is_never_coverage(tmp_path, monkeypatch):
    cfg, _, _ = setup_run(tmp_path, monkeypatch, [observation(), observation(), observation(b'{"n":2}')])
    result = shadow.run(cfg, 23, cases=[case()])
    assert result["counters"]["data_changed"] == 1
    assert result["pairs"]["R05"] == 0
    assert shadow.result_exit_code(result) == 2


@pytest.mark.parametrize("status", [429, 503])
def test_busy_call_pauses_sixty_seconds_and_is_in_denominator(tmp_path, monkeypatch, status):
    cfg, calls, _ = setup_run(tmp_path, monkeypatch, [{"status": status, "inconclusive": "busy"}])
    result = shadow.run(cfg, 100, cases=[case()])
    data_calls = [c for c in calls if c[2] == "R05"]
    assert data_calls[1][0] - data_calls[0][0] >= 60
    assert result["counters"]["busy"] == 1
    assert result["counters"]["attempted"] >= 2
    assert result["outcome"] == "INCONCLUSIVE"  # A later match cannot erase the busy attempt.


def test_all_calls_including_readiness_share_one_budget(tmp_path, monkeypatch):
    cfg, calls, _ = setup_run(tmp_path, monkeypatch, [])
    result = shadow.run(cfg, 100, cases=[case()])
    assert result["outcome"] == "PASS"
    for start, _, _ in calls:
        assert sum(start <= t < start + 10 for t, _, _ in calls) <= 3
    assert sum(c[2] == "READY" for c in calls) >= 4


def test_unapproved_floor_and_empty_results_cannot_pass(tmp_path, monkeypatch):
    cfg, _, _ = setup_run(tmp_path, monkeypatch, [observation(b'[]')] * 3)
    result = shadow.run(replace(cfg, floor_approved=False), 11, cases=[case()])
    assert result["outcome"] == "INCONCLUSIVE"
    assert shadow.result_exit_code(result) == 2


def test_deploy_state_during_run_stops_before_next_call(tmp_path, monkeypatch):
    cfg, _, _ = setup_run(tmp_path, monkeypatch, [])
    def fetch(*args):
        from pathlib import Path
        Path(cfg.deploy_state_path).symlink_to("absent")
        return observation()
    monkeypatch.setattr(shadow, "_one_call", fetch)
    with pytest.raises(shadow.ShadowError):
        shadow.run(cfg, 100, cases=[case()])


def test_wrong_readiness_identity_is_a_stop_not_normalization(tmp_path, monkeypatch):
    cfg, _, _ = setup_run(tmp_path, monkeypatch, [])
    monkeypatch.setattr(shadow, "_one_call", lambda *args: observation(b'{"status":"ok","db":"ok","environment":"staging","release_sha":"wrong"}'))
    with pytest.raises(shadow.ShadowError, match="identity"):
        shadow.run(cfg, 100, cases=[case()])


def test_pressure_pauses_without_sending_a_call(tmp_path, monkeypatch):
    cfg, calls, clock = setup_run(tmp_path, monkeypatch, [])
    monkeypatch.setattr(shadow, "memory_pressure", lambda: clock.now < 60)
    shadow.run(cfg, 100, cases=[case()])
    assert calls[0][0] >= 60


def test_transport_deadline_covers_streamed_body_and_refuses_redirects(tmp_path):
    import threading
    import time
    from http.server import BaseHTTPRequestHandler, HTTPServer

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass
        def do_GET(self):
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            try:
                for _ in range(20):
                    self.wfile.write(b" ")
                    self.wfile.flush()
                    time.sleep(.02)
            except (BrokenPipeError, ConnectionResetError):
                pass
    server = HTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever)
    thread.start()
    try:
        cfg = replace(config(tmp_path), per_call_deadline_seconds=.1)
        result = shadow._one_call(cfg, f"http://127.0.0.1:{server.server_port}", case())
        assert result["inconclusive"] == "timeout"
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_transport_body_bound_and_mcp_headers(tmp_path, monkeypatch):
    class Response:
        status = 200
        def read(self, size):
            assert size == 33
            return b"x" * size
        def getheaders(self):
            return [("content-type", "application/json")]
    sent = []
    class Connection:
        def __init__(self, *args, **kwargs):
            pass
        def request(self, method, path, body=None, headers=None):
            sent.append((method, headers))
        def getresponse(self):
            return Response()
        def close(self):
            pass
    monkeypatch.setattr(shadow.http.client, "HTTPConnection", Connection)
    cfg = replace(config(tmp_path), max_response_bytes=32)
    result = shadow._one_call(cfg, cfg.rust_url, {"id": "list_markets", "method": "POST", "path": "/mcp", "body": {}})
    assert result["inconclusive"] == "oversize"
    assert sent[0][1]["host"] == "127.0.0.1"
    assert sent[0][1]["accept"] == "application/json, text/event-stream"
    assert not set(sent[0][1]) & shadow.FORBIDDEN_HEADERS


def test_three_health_contracts_are_checked_with_separate_identities(tmp_path, monkeypatch):
    cfg, calls, _ = setup_run(tmp_path, monkeypatch, [])
    shadow.run(cfg, 100, cases=[case()])
    assert {c[2] for c in calls} >= {"READY", "LIVE", "HEALTH"}


def test_groups_are_round_robin_not_weighted():
    rows = [case("R01", "/api/ports"), case("R02", "/api/ports/NLRTM"),
            dict(case("R12", "/api/prices"), group="prices"),
            dict(case("R07", "/api/curves/forward"), group="curves")]
    assert [c["id"] for c in shadow.round_robin_cases(rows)] == ["R01", "R12", "R07", "R02"]


def test_tool_errors_are_not_economic_coverage():
    row = {"method": "POST"}
    for raw in (b'{"error":{"code":-32602}}', b'{"result":{"isError":true}}'):
        assert not shadow._nonempty(observation(raw), row)


def test_cli_forced_content_difference_has_failure_exit_and_private_output(tmp_path, monkeypatch):
    cfg, _, _ = setup_run(tmp_path, monkeypatch, [observation(), observation(b'{"n":9}'), observation()])
    monkeypatch.setattr(shadow, "load_config", lambda _: cfg)
    monkeypatch.setattr(shadow, "load_staging_cases", lambda _: [case()])
    assert shadow.main(["--config", "private.toml", "--duration-seconds", "100"]) == 1
    path = tmp_path / "private" / "shadow-summary.json"
    assert path.stat().st_mode & 0o777 == 0o600
    assert json.loads(path.read_text())["outcome"] == "FAIL"
    assert "?" not in path.read_text()


def test_mcp_tool_error_does_not_increment_successful_pairs(tmp_path, monkeypatch):
    cfg, _, _ = setup_run(tmp_path, monkeypatch, [observation(b'{"result":{"isError":true}}')] * 3)
    row = dict(case("list_markets", "/mcp"), method="POST", body={})
    result = shadow.run(cfg, 23, cases=[row])
    assert result["pairs"]["list_markets"] == 0
    assert result["counters"]["inconclusive"] == 1


def test_private_output_is_checked_before_network(tmp_path, monkeypatch):
    cfg = replace(config(tmp_path), output_dir="work/public")
    monkeypatch.setattr(shadow, "load_config", lambda _: cfg)
    def forbidden(*args):
        pytest.fail("unsafe evidence path must refuse before run")
    monkeypatch.setattr(shadow, "run", forbidden)
    assert shadow.main(["--config", "private.toml", "--duration-seconds", "100"]) == 1


def test_readiness_failure_pauses_before_stop(tmp_path, monkeypatch):
    cfg, _, clock = setup_run(tmp_path, monkeypatch, [])
    monkeypatch.setattr(shadow, "_one_call", lambda *args: {"status": 503, "inconclusive": "busy"})
    with pytest.raises(shadow.ShadowError):
        shadow.run(cfg, 100, cases=[case()])
    assert clock.now >= 60


def test_busy_status_backoff_is_required_even_for_oversize_body(tmp_path, monkeypatch):
    cfg, calls, _ = setup_run(tmp_path, monkeypatch, [{"status": 503, "inconclusive": "oversize"}])
    shadow.run(cfg, 130, cases=[case()])
    data_calls = [c for c in calls if c[2] == "R05"]
    assert data_calls[1][0] - data_calls[0][0] >= 60


def test_minute_limit_is_enforced_in_run_loop(tmp_path, monkeypatch):
    cfg, calls, _ = setup_run(tmp_path, monkeypatch, [])
    cfg = replace(cfg, required_observations_per_route=2,
                  price_minute_limits={"export_per_minute": 2})
    row = dict(case("R14", "/api/prices/reference/export"), group="prices")
    shadow.run(cfg, 180, cases=[row])
    starts = [t for t, base, identifier in calls if identifier == "R14" and base == cfg.python_url]
    assert len(starts) >= 3
    assert starts[2] - starts[0] >= 60


def test_response_headers_remain_part_of_content_contract():
    same = observation()
    other = observation(headers={"content-type": "application/json", "x-other": "different"})
    assert shadow.classify_sandwich(same, other, same, request_path="/api/news") == "difference"


def test_final_readiness_must_complete_before_pass(tmp_path, monkeypatch):
    cfg, _, _ = setup_run(tmp_path, monkeypatch, [])
    result = shadow.run(cfg, 23, cases=[case()])
    assert result["outcome"] == "INCONCLUSIVE"


@pytest.mark.parametrize("record", [
    {"case_id": "private-query=value"}, {"route_id": "private-value"},
    {"status_pair": "secret"}, {"counters": {"private-value": 1}},
    {"byte_counts": ["private-value"]}, {"field_path": "/body/private_company"},
])
def test_diagnostics_accept_only_typed_schema_metadata(record):
    with pytest.raises(shadow.ShadowError):
        shadow.sanitize_diagnostic(record)
