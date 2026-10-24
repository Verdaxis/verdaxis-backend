"""Bounded oracle recovery must not hide a meaning difference or Rust failure."""
import io
import json
import urllib.error
from types import SimpleNamespace

import pytest

from scripts.rust_contract import compare, record, run_disposable, simulate
from scripts.rust_contract.phase2_meaning import PHASE1_RULES_SHA256

CASE = {"id": "R02-ports", "method": "GET", "path": "/api/ports"}


class Response(io.BytesIO):
    def __init__(self, status=200, body=b"[]", headers=None):
        super().__init__(body)
        self.status = status
        self.headers = headers or {"content-type": "application/json"}


def busy(body=b'{"detail":"temporarily busy"}', retry_after="2"):
    return urllib.error.HTTPError("http://127.0.0.1:9/api/ports", 503, "busy",
        {"content-type": "application/json", **({"Retry-After": retry_after} if retry_after is not None else {})}, io.BytesIO(body))


@pytest.fixture
def harness(monkeypatch, tmp_path):
    instance = simulate.Simulation("small", output_dir=tmp_path, groups=["simple_reads"])
    monkeypatch.setattr(instance, "start_environment", lambda group: {
        "base_env": {"ENVIRONMENT": "test", "RUST_CONTRACT_PROFILE": "phase1-meaning-v1",
                     "MEANING_RULES_SHA256": PHASE1_RULES_SHA256},
        "python_url": "http://127.0.0.1:9", "rust_url": "http://127.0.0.1:10"})
    monkeypatch.setattr(instance, "_seed_workload", lambda env: None)
    monkeypatch.setattr(instance, "stop_environment", lambda: None)
    monkeypatch.setattr(record, "attest", lambda *args: None)
    waits = []
    monkeypatch.setattr(record.time, "sleep", waits.append)
    return instance, waits


def transport(monkeypatch, replies):
    pending = iter(replies)
    calls = []

    def open_response(request, **kwargs):
        calls.append(request.full_url)
        reply = next(pending)
        if isinstance(reply, Exception):
            raise reply
        return reply

    monkeypatch.setattr(record._OPENER, "open", open_response)
    return calls


@pytest.mark.parametrize("body", [b'{"detail":"temporarily busy"}',
    b'{"detail":"Public market data is temporarily busy; retry shortly."}',
    b'{"detail":"Market is temporarily busy; retry shortly."}'])
def test_busy_then_success_compares_and_records_one_retry(harness, monkeypatch, body):
    instance, waits = harness
    calls = transport(monkeypatch, [busy(body), Response(), Response(), Response()])
    outcome = instance._run_group("simple_reads", [CASE])
    assert outcome["outcome"] == "match"
    assert len(calls) == 4
    assert waits == [2]
    assert outcome["oracle_retries"] == [{"group": "simple_reads", "request": CASE["id"],
        "leg": "python-before", "reason": "503 temporarily busy", "attempt": 1, "delay_seconds": 2}]


@pytest.mark.parametrize("failure", [busy(b'{"detail":"other failure"}'),
    busy(retry_after=None),
    urllib.error.HTTPError("http://127.0.0.1:9", 500, "failed", {}, io.BytesIO(b"failed"))])
def test_nontransient_python_answer_is_not_retried(harness, monkeypatch, failure):
    instance, waits = harness
    calls = transport(monkeypatch, [failure, Response(), Response()])
    outcome = instance._run_group("simple_reads", [CASE])
    assert len(calls) == 3
    assert waits == []
    assert outcome["outcome"] != "match"


# A present Retry-After always bounds the wait: a large, non-finite, negative,
# non-numeric or HTTP-date value is clamped to the same stated cap. A missing
# header is not a busy retry instruction and stays non-retried (above).
@pytest.mark.parametrize("value", ["999999", "inf", "-3", "soon",
    "Wed, 21 Oct 2015 07:28:00 GMT"])
def test_retry_after_values_are_clamped_to_the_cap(harness, monkeypatch, value):
    instance, waits = harness
    calls = transport(monkeypatch, [busy(retry_after=value), Response(), Response(), Response()])
    outcome = instance._run_group("simple_reads", [CASE])
    assert outcome["outcome"] == "match"
    assert len(calls) == 4
    assert waits == [record.MAX_RETRY_AFTER_SECONDS]
    assert outcome["oracle_retries"][0]["delay_seconds"] == record.MAX_RETRY_AFTER_SECONDS


def test_rust_busy_is_never_retried(harness, monkeypatch):
    instance, waits = harness
    calls = transport(monkeypatch, [Response(), busy(), Response()])
    outcome = instance._run_group("simple_reads", [CASE])
    assert outcome["outcome"] == "difference"
    assert len(calls) == 3
    assert waits == []


@pytest.mark.parametrize("failure", [TimeoutError("capture timed out"), ConnectionResetError("reset")])
def test_transient_capture_is_retried(harness, monkeypatch, failure):
    instance, _ = harness
    calls = transport(monkeypatch, [failure, Response(), Response(), Response()])
    outcome = instance._run_group("simple_reads", [CASE])
    assert outcome["outcome"] == "match"
    assert len(calls) == 4
    assert len(outcome["oracle_retries"]) == 1
    assert outcome["oracle_retries"][0]["reason"] in {"capture timeout", "connection reset"}


def test_first_complete_python_answer_is_not_replaced(harness, monkeypatch):
    instance, _ = harness
    calls = transport(monkeypatch, [Response(body=b"[1]"), Response(), Response(body=b"[1]")])
    outcome = instance._run_group("simple_reads", [CASE])
    assert outcome["outcome"] == "difference"
    assert len(calls) == 3


def test_exhausted_capture_is_oracle_unavailable(harness, monkeypatch):
    instance, _ = harness
    calls = transport(monkeypatch, [busy(retry_after="0") for _ in range(4)])
    outcome = instance._run_group("simple_reads", [CASE])
    assert outcome["outcome"] == "ORACLE_UNAVAILABLE"
    assert len(calls) == 4
    assert [event["attempt"] for event in outcome["oracle_retries"]] == [1, 2, 3]
    assert "differences" not in outcome


@pytest.fixture
def startup(monkeypatch):
    instance = run_disposable.Runner("simple_reads", seed=False, keep=False, seed_mode="corpus")
    monkeypatch.setattr(instance, "start_database", lambda: 54321)
    monkeypatch.setattr(instance, "migrate_and_seed", lambda env: None)
    ports = iter([54322, 54323])
    monkeypatch.setattr(run_disposable, "ephemeral_port", lambda: next(ports))
    monkeypatch.setattr(run_disposable, "attest", lambda *args: None)
    children = []

    def spawn(args, env):
        child = SimpleNamespace(terminated=False, waited=False)
        child.terminate = lambda: setattr(child, "terminated", True)
        child.wait = lambda **kwargs: setattr(child, "waited", True)
        children.append((args, child))
        instance.children.append(child)

    monkeypatch.setattr(instance, "spawn", spawn)
    return instance, children


def test_startup_retry_then_success(startup, monkeypatch):
    instance, children = startup
    waits = []

    def wait(port, timeout=60):
        waits.append((port, timeout))
        if len(waits) == 1:
            raise run_disposable.RunnerError("nothing accepted a connection")

    monkeypatch.setattr(run_disposable, "wait_for_port", wait)
    try:
        instance.start_environment("simple_reads")
        result = "ready"
    except run_disposable.RunnerError:
        result = "failed"
    assert result == "ready"
    assert waits[0][1] == 180
    python_children = [child for args, child in children if args[0] == instance.python_bin]
    assert len(python_children) == 2
    assert python_children[0].terminated and python_children[0].waited
    assert len(instance.oracle_retries) == 1
    assert instance.oracle_retries[0]["request"] == "startup"


def test_startup_exhaustion_is_oracle_unavailable(startup, monkeypatch):
    instance, children = startup
    monkeypatch.setattr(run_disposable, "wait_for_port", lambda *args, **kwargs: (_ for _ in ()).throw(
        run_disposable.RunnerError("nothing accepted a connection")))
    with pytest.raises(run_disposable.RunnerError) as failure:
        instance.start_environment("simple_reads")
    assert "ORACLE_UNAVAILABLE" in str(failure.value)
    python_children = [child for args, child in children if args[0] == instance.python_bin]
    assert len(python_children) == 3
    assert all(child.terminated and child.waited for child in python_children)
    assert [event["attempt"] for event in instance.oracle_retries] == [1, 2]


def test_report_keeps_every_retry_and_fails_gate(harness, monkeypatch, tmp_path):
    instance, _ = harness
    monkeypatch.setattr(instance, "build_command", lambda: ["true"])
    monkeypatch.setattr(simulate.subprocess, "run", lambda *args, **kwargs: None)
    monkeypatch.setattr(compare, "load_cases", lambda *args: [CASE])
    transport(monkeypatch, [busy(retry_after="0") for _ in range(4)])
    instance.run()
    report = json.loads((tmp_path / "simulation-small.json").read_text())
    assert report["gate"] == "no-go"
    assert report["differences"] == 0
    assert report["oracle_unavailable_groups"] == ["simple_reads"]
    assert report["oracle_retries"] == report["groups"]["simple_reads"]["oracle_retries"]
    assert len(report["oracle_retries"]) == 3


def test_startup_failure_is_reported_and_cleaned(harness, monkeypatch, tmp_path):
    instance, _ = harness
    stopped = []
    monkeypatch.setattr(instance, "stop_environment", lambda: stopped.append(True))

    def failed_start(group):
        instance.oracle_retries.extend([{"group": group, "request": "startup", "attempt": number}
                                        for number in (1, 2)])
        raise run_disposable.OracleStartupUnavailable("ORACLE_UNAVAILABLE: startup")

    monkeypatch.setattr(instance, "start_environment", failed_start)
    monkeypatch.setattr(instance, "build_command", lambda: ["true"])
    monkeypatch.setattr(simulate.subprocess, "run", lambda *args, **kwargs: None)
    monkeypatch.setattr(compare, "load_cases", lambda *args: [CASE])
    instance.run()
    report = json.loads((tmp_path / "simulation-small.json").read_text())
    assert report["gate"] == "no-go"
    assert report["oracle_unavailable_groups"] == ["simple_reads"]
    assert len(report["oracle_retries"]) == 2
    assert stopped == [True]


@pytest.mark.parametrize("value", ["0", "-1", "nan", "inf", "bad"])
def test_invalid_startup_timeout_refuses(monkeypatch, value):
    monkeypatch.setenv("ORACLE_STARTUP_TIMEOUT_SECONDS", value)
    with pytest.raises(run_disposable.RunnerError, match="positive and finite"):
        run_disposable.Runner("health", seed=False, keep=False, seed_mode="corpus")


def test_startup_timeout_is_configurable(startup, monkeypatch):
    instance, _ = startup
    instance.oracle_startup_timeout = 241
    waits = []
    monkeypatch.setattr(run_disposable, "wait_for_port", lambda port, timeout=60: waits.append(timeout))
    instance.start_environment("simple_reads")
    assert waits == [241, 60]


def test_rust_timeout_is_not_retried(harness, monkeypatch):
    instance, waits = harness
    calls = transport(monkeypatch, [Response(), TimeoutError("Rust timeout")])
    outcome = instance._run_group("simple_reads", [CASE])
    assert outcome["outcome"] == "error"
    assert len(calls) == 2
    assert waits == []


def test_body_timeout_is_retried(harness, monkeypatch):
    instance, _ = harness

    class TimedOut(Response):
        def read(self, *args):
            raise TimeoutError("body timed out")

    calls = transport(monkeypatch, [TimedOut(), Response(), Response(), Response()])
    outcome = instance._run_group("simple_reads", [CASE])
    assert outcome["outcome"] == "match"
    assert len(calls) == 4


def test_other_transport_failure_is_not_retried(harness, monkeypatch):
    instance, waits = harness
    calls = transport(monkeypatch, [urllib.error.URLError(ConnectionRefusedError("refused"))])
    assert instance._run_group("simple_reads", [CASE])["outcome"] == "error"
    assert len(calls) == 1
    assert waits == []


def test_error_body_timeout_is_retried(harness, monkeypatch):
    instance, _ = harness

    class TimedOut(Response):
        def read(self, *args):
            raise TimeoutError("error body timeout")

    failure = urllib.error.HTTPError("http://127.0.0.1:9", 503, "busy", {}, TimedOut())
    calls = transport(monkeypatch, [failure, Response(), Response(), Response()])
    assert instance._run_group("simple_reads", [CASE])["outcome"] == "match"
    assert len(calls) == 4


def test_startup_failure_report_preserves_prior_target_results(startup, monkeypatch, tmp_path):
    instance, _ = startup
    instance.rust_target = "all"
    instance.report = tmp_path / "contract.json"
    monkeypatch.setattr(run_disposable.subprocess, "run", lambda *args, **kwargs: None)
    monkeypatch.setattr(instance, "target_groups", lambda: ["health", "echo"])
    monkeypatch.setattr(instance, "stop_environment", lambda: None)

    def start(group):
        if group == "echo":
            instance.oracle_unavailable_groups.append(group)
            raise run_disposable.OracleStartupUnavailable("ORACLE_UNAVAILABLE: echo/startup")
        return {"base_env": {}, "python_url": "python", "rust_url": "rust"}

    monkeypatch.setattr(instance, "start_environment", start)
    monkeypatch.setattr(instance, "run_targets", lambda *args, **kwargs: {"health": False})
    with pytest.raises(run_disposable.OracleStartupUnavailable):
        instance.run()
    report = json.loads(instance.report.read_text())
    assert report["groups"] == {"health": "failed"}
    assert report["outcome"] == "ORACLE_UNAVAILABLE"


def test_contract_child_capture_retries_only_designated_python(harness, monkeypatch, tmp_path):
    instance, _ = harness
    retry_log = tmp_path / "events.jsonl"
    monkeypatch.setenv("RUST_CONTRACT_PROFILE", "phase1-meaning-v1")
    monkeypatch.setenv("ORACLE_CAPTURE_URL", "http://127.0.0.1:9")
    monkeypatch.setenv("ORACLE_CAPTURE_GROUP", "simple_reads")
    monkeypatch.setenv("ORACLE_RETRY_LOG", str(retry_log))
    calls = transport(monkeypatch, [busy(retry_after="0"), Response(), busy(retry_after="0")])
    expected = record.record("http://127.0.0.1:9", instance.token, [CASE])
    actual = record.record("http://127.0.0.1:10", instance.token, [CASE])
    assert expected[0]["exchange"]["status"] == 200
    assert actual[0]["exchange"]["status"] == 503
    assert len(calls) == 3
    assert len(retry_log.read_text().splitlines()) == 1


def test_recording_after_a_retry_keeps_the_persisted_shape(harness, monkeypatch):
    instance, _ = harness
    monkeypatch.setenv("RUST_CONTRACT_PROFILE", "phase1-meaning-v1")
    transport(monkeypatch, [busy(retry_after="0"), Response(), Response(), Response()])
    entries = record.record("http://127.0.0.1:9", instance.token, [CASE], oracle=True,
                            group="simple_reads", leg="python-before")
    # The retry decision needs the headers in memory...
    assert entries[0]["response_headers"]["content-type"] == "application/json"
    # ...but persisted evidence keeps exactly the pre-retry shape.
    assert set(record.stable(entries)[0]) == {"id", "request", "exchange", "raw_body_base64"}
