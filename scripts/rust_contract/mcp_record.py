"""Bounded capture for stateless streamable HTTP, without a session store.

A standalone GET records stream establishment (status and headers), then closes
it. It has no terminal JSON-RPC response. POST streams end at the response with
the request's id. Neither path waits for the server to close an idle stream.
"""

from __future__ import annotations

import json
import time

from scripts.rust_contract.compare import ContractError

MAX_RESPONSE_BYTES = 8 * 1024 * 1024
CASE_DEADLINE_SECONDS = 30.0


def read_mcp_response(response, case, *, max_bytes=MAX_RESPONSE_BYTES):
    content_type = response.headers.get("content-type", "").split(";", 1)[0]
    streaming = content_type == "text/event-stream" and response.status == 200
    if streaming and case["method"].upper() == "GET":
        return b""
    request_id = None
    try:
        request_id = json.loads(case.get("body") or "{}").get("id")
    except (ValueError, AttributeError):
        pass
    deadline = time.monotonic() + CASE_DEADLINE_SECONDS
    body = bytearray()
    pending = bytearray()
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise ContractError("MCP response exceeded the case deadline")
        socket = getattr(getattr(getattr(response, "fp", None), "raw", None), "_sock", None)
        if socket is not None:
            socket.settimeout(remaining)
        try:
            chunk = response.read1(min(65536, max_bytes - len(body) + 1))
        except OSError as error:
            raise ContractError("MCP response read timed out") from error
        if not chunk:
            if streaming:
                raise ContractError("MCP stream ended without a terminal response")
            return bytes(body)
        body.extend(chunk)
        if len(body) > max_bytes:
            raise ContractError("MCP response exceeded the size limit")
        if not streaming:
            continue
        pending.extend(chunk)
        # Event boundaries may span network chunks. Preserve original wire bytes.
        # Do not normalize an unfinished CRLF: the next chunk may begin with LF.
        events = []
        while True:
            lf_boundary = pending.find(b"\n\n")
            crlf_boundary = pending.find(b"\r\n\r\n")
            boundaries = [(index, width) for index, width in ((lf_boundary, 2), (crlf_boundary, 4)) if index >= 0]
            if not boundaries:
                break
            index, width = min(boundaries)
            event = bytes(pending[:index]).replace(b"\r\n", b"\n")
            del pending[:index + width]
            events.append((event, len(body) - len(pending)))
        for event, terminal_offset in events:
            data = b"\n".join(line[5:].lstrip(b" ") for line in event.split(b"\n") if line.startswith(b"data:"))
            if not data:
                continue
            try:
                message = json.loads(data)
            except ValueError as error:
                raise ContractError("MCP stream contained invalid JSON") from error
            if (
                isinstance(message, dict)
                and request_id is not None
                and type(message.get("id")) is type(request_id)
                and message.get("id") == request_id
                and ("result" in message or "error" in message)
            ):
                return bytes(body[:terminal_offset])
