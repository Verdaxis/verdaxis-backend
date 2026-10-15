"""Bounded MCP transport capture; no database or provider calls."""

import io
import json

import pytest

from scripts.rust_contract.compare import ContractError
from scripts.rust_contract.mcp_record import read_mcp_response


class Response(io.BytesIO):
    def __init__(self, body=b"", content_type="application/json", status=200):
        super().__init__(body)
        self.headers = {"content-type": content_type}
        self.status = status

    def read1(self, size):
        return self.read(size)


def case(method="POST", request_id=7):
    return {"method": method, "body": json.dumps({"jsonrpc": "2.0", "id": request_id, "method": "tools/list"})}


def test_recorded_mcp_request_keeps_exact_body(monkeypatch):
    from scripts.rust_contract import record

    response = Response(b'{"jsonrpc":"2.0","id":7,"result":{}}')
    monkeypatch.setattr(record._OPENER, "open", lambda *args, **kwargs: response)
    request = {**case(), "id": "mcp-fixture", "path": "/mcp"}
    entry = record.perform("http://127.0.0.1:54321", request)
    assert entry["request"]["body"] == request["body"]


def test_json_response_is_preserved_exactly():
    body = b'{"jsonrpc":"2.0","id":7,"result":{}}'
    assert read_mcp_response(Response(body), case()) == body


def test_stateless_get_records_headers_without_waiting_for_stream_close():
    class OpenStream(Response):
        def read(self, size=-1):
            raise AssertionError("standalone GET has no terminal message to await")

    assert read_mcp_response(OpenStream(content_type="text/event-stream"), case("GET")) == b""


def test_post_sse_stops_at_matching_terminal_message():
    body = b': keepalive\n\nevent: message\ndata: {"jsonrpc":"2.0","id":7,"result":{}}\n\n'
    assert read_mcp_response(Response(body, "text/event-stream"), case()) == body


def test_terminal_capture_does_not_depend_on_network_chunk_size():
    terminal = b'data: {"jsonrpc":"2.0","id":7,"result":{}}\n\n'
    trailing = b': keepalive\n\n'
    assert read_mcp_response(Response(terminal + trailing, "text/event-stream"), case()) == terminal


def test_post_sse_requires_correlated_terminal_message():
    body = b'data: {"jsonrpc":"2.0","id":8,"result":{}}\n\n'
    with pytest.raises(ContractError, match="terminal"):
        read_mcp_response(Response(body, "text/event-stream"), case())


def test_crlf_event_boundary_can_cross_chunks():
    body = b'data: {"jsonrpc":"2.0","id":7,"result":{}}\r\n\r\n'

    class FragmentedResponse(Response):
        def read1(self, size):
            return self.read(1)

    assert read_mcp_response(FragmentedResponse(body, "text/event-stream"), case()) == body


def test_deadline_is_total_not_per_chunk(monkeypatch):
    from scripts.rust_contract import mcp_record

    ticks = iter([0, 0, 31])
    monkeypatch.setattr(mcp_record.time, "monotonic", lambda: next(ticks))

    class TrickleResponse(Response):
        def read1(self, size):
            return b" "

    with pytest.raises(ContractError, match="deadline"):
        read_mcp_response(TrickleResponse(), case())


def test_socket_timeout_is_a_recording_failure():
    class TimedOutResponse(Response):
        def read1(self, size):
            raise TimeoutError()

    with pytest.raises(ContractError, match="timed out"):
        read_mcp_response(TimedOutResponse(), case())


def test_incomplete_sse_is_not_a_success():
    with pytest.raises(ContractError, match="terminal"):
        read_mcp_response(Response(b'data: {', "text/event-stream"), case())


def test_body_size_is_bounded():
    with pytest.raises(ContractError, match="size"):
        read_mcp_response(Response(b"x" * 17), case(), max_bytes=16)


def test_notifications_and_http_errors_keep_their_body():
    assert read_mcp_response(Response(status=202), {"method": "POST", "body": '{"jsonrpc":"2.0","method":"notifications/initialized"}'}) == b""
    assert read_mcp_response(Response(b"Invalid Origin header", "text/plain", 403), case()) == b"Invalid Origin header"
