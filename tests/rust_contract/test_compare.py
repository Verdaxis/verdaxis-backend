"""Unit tests for the strict public-contract comparator.

The comparator is the single authority for Python/Rust contract equality. It
must never normalize a real difference: decimal scale, null versus absence,
object membership, array order, header presence, and raw body bytes all have
to survive the walk.
"""

import json
from decimal import Decimal

import pytest

from scripts.rust_contract import compare as compare_module
from scripts.rust_contract import record
from scripts.rust_contract.compare import (
    ContractError,
    capture,
    compare,
    parse_json,
    recording_differences,
    strip_headers,
)


def test_decimal_scale_and_null_are_not_normalized():
    base = {"status": 200, "headers": {}, "body": {"price": "1.00", "x": None}}
    assert compare(base, {**base, "body": {"price": "1", "x": None}})
    assert compare(base, {**base, "body": {"price": "1.00"}})


def test_clock_boundary_changes_are_not_normalized():
    base = {
        "status": 200,
        "headers": {},
        "body": {"items": [{"id": "a"}], "staleness_status": "FRESH"},
    }
    assert compare(base, {**base, "body": {"items": [], "staleness_status": "STALE"}})


def test_object_membership_and_array_order():
    assert not compare({"body": {"a": 1, "b": 2}}, {"body": {"b": 2, "a": 1}})
    assert compare({"body": {"a": 1}}, {"body": {"b": 1}})
    assert compare({"body": [1, 2]}, {"body": [2, 1]})


def test_duplicate_json_keys_are_rejected():
    with pytest.raises(ContractError):
        parse_json(b'{"a": 1, "a": 2}')


def test_json_floats_parse_to_decimal_not_binary_float():
    value = parse_json(b'{"price": 1.10}')
    assert isinstance(value["price"], Decimal)
    assert value["price"] == Decimal("1.10")
    assert str(value["price"]) == "1.10"


def test_transport_headers_are_excluded_but_contract_headers_compared():
    stripped = strip_headers(
        {
            "Date": "Thu, 01 Jan 2026 00:00:00 GMT",
            "Content-Length": "3",
            "Transfer-Encoding": "chunked",
            "Content-Type": "application/json",
            "Vary": "Origin",
        }
    )
    assert stripped == {"content-type": "application/json", "vary": "Origin"}


def test_header_absence_is_a_difference():
    expected = {"status": 200, "headers": {"retry-after": "1"}, "body": None}
    actual = {"status": 200, "headers": {}, "body": None}
    assert compare(expected, actual)


def test_csv_bytes_are_compared_exactly():
    lf = capture(200, {"Content-Type": "text/csv"}, b"a,b\n1,2\n")
    crlf = capture(200, {"Content-Type": "text/csv"}, b"a,b\r\n1,2\r\n")
    assert compare(lf, crlf)
    assert lf["body_sha256"] != crlf["body_sha256"]


def test_capture_verifies_content_length_independently():
    with pytest.raises(ContractError):
        capture(200, {"Content-Length": "99"}, b"abc")
    envelope = capture(200, {"Content-Length": "3"}, b"abc")
    assert envelope["status"] == 200


def test_bounded_read_collects_a_finite_body():
    class Finite:
        def __init__(self):
            self.chunks = [b"ab", b"cd", b""]

        def read(self, _size):
            chunk = self.chunks.pop(0)
            return chunk

    assert record._read_bounded(Finite()) == b"abcd"


def test_bounded_read_refuses_an_endless_stream(monkeypatch):
    monkeypatch.setattr(record, "CASE_DEADLINE_SECONDS", 0.05)

    class Endless:
        def read(self, _size):
            return b"x"

    with pytest.raises(ContractError):
        record._read_bounded(Endless())


def _entry(case_id: str) -> dict:
    """One recorded case, shaped like `record.perform` output."""
    return {
        "id": case_id,
        "request": {"method": "GET", "path": "/api/ping", "query": "", "headers": {}},
        "exchange": capture(200, {"content-type": "application/json"}, b'{"ok":true}'),
    }


def test_one_sided_recording_never_zip_truncates():
    # A recording with fewer entries must fail, not silently skip the surplus.
    first = [_entry("A"), _entry("B")]
    assert recording_differences(first, [_entry("A")]) == ["recording-length"]
    assert recording_differences([_entry("A")], first) == ["recording-length"]


def test_recording_case_id_mismatch_is_a_difference():
    first = [_entry("A"), _entry("B")]
    second = [_entry("A"), _entry("C")]
    assert recording_differences(first, second) == ["B:ordering"]


def test_identical_recordings_have_no_differences():
    first = [_entry("A"), _entry("B")]
    assert recording_differences(first, list(first)) == []


def _main_with_fake_endpoints(
    monkeypatch, tmp_path, *, case_ids, python_entries, rust_entries
):
    """Point `record` at two fake endpoints that return the given recordings."""
    cases = tmp_path / "cases.json"
    cases.write_text(json.dumps({"groups": {"g": [{"id": i} for i in case_ids]}}))
    monkeypatch.setenv("PYTHON_CONTRACT_URL", "http://127.0.0.1:1")
    monkeypatch.setenv("RUST_CONTRACT_URL", "http://127.0.0.1:2")
    monkeypatch.setenv("DISPOSABLE_TEST_TOKEN", "token")
    monkeypatch.setattr(compare_module, "attest", lambda url, token: {})

    def fake_record(url, token, cases):
        return list(python_entries if url.endswith(":1") else rust_entries)

    monkeypatch.setattr(record, "record", fake_record)
    return ["--group", "g", "--cases", str(cases)]


def test_main_refuses_a_case_count_mismatch(monkeypatch, tmp_path, capsys):
    argv = _main_with_fake_endpoints(
        monkeypatch,
        tmp_path,
        case_ids=["A"],
        python_entries=[_entry("A")],
        rust_entries=[_entry("A"), _entry("B")],
    )
    assert compare_module.main(argv) == 1
    assert "recorded case counts differ" in capsys.readouterr().err


def test_main_refuses_a_case_id_mismatch(monkeypatch, tmp_path, capsys):
    argv = _main_with_fake_endpoints(
        monkeypatch,
        tmp_path,
        case_ids=["A", "B"],
        python_entries=[_entry("A"), _entry("B")],
        rust_entries=[_entry("A"), _entry("C")],
    )
    assert compare_module.main(argv) == 1
    assert "B:ordering" in capsys.readouterr().err


def test_main_passes_identical_recordings(monkeypatch, tmp_path, capsys):
    argv = _main_with_fake_endpoints(
        monkeypatch,
        tmp_path,
        case_ids=["A"],
        python_entries=[_entry("A")],
        rust_entries=[_entry("A")],
    )
    assert compare_module.main(argv) == 0
    assert "contract group g OK: 1 case(s)" in capsys.readouterr().out
