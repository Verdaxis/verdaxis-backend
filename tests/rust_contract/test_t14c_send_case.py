"""The measurement diagnostic must never GET an MCP tool path.

A GET /mcp opens the server's event stream, which keeps sending data forever:
the read timeout never fires and the caller blocks while holding the host lock
(this blocked five other lanes for 46 minutes on 18 October). The warm-up and
the sample loop both go through `send_case`, so this test pins the one helper.
"""

from unittest.mock import MagicMock

import pytest

from scripts.rust_contract.t14c_shapes import PATHS, TOOL_BODIES, send_case


@pytest.mark.parametrize("name", sorted(TOOL_BODIES))
def test_an_mcp_tool_never_sends_get(name):
    client = MagicMock()
    send_case(client, "http://127.0.0.1:1", name, PATHS[name])
    client.get.assert_not_called()
    client.post.assert_called_once()
    _, kwargs = client.post.call_args
    assert kwargs["json"] == TOOL_BODIES[name]
    assert kwargs["headers"]["accept"] == "application/json, text/event-stream"


@pytest.mark.parametrize(
    "name", ["R06", "R20", "R27", "get_market_slice", "list_markets", "plan_voyage_fuel"]
)
def test_every_measured_id_is_dispatched(name):
    client = MagicMock()
    send_case(client, "http://127.0.0.1:1", name, PATHS[name])
    assert client.get.called or client.post.called


def test_a_plain_route_still_sends_get():
    client = MagicMock()
    send_case(client, "http://127.0.0.1:1", "R24", PATHS["R24"])
    client.post.assert_not_called()
    client.get.assert_called_once_with("http://127.0.0.1:1" + PATHS["R24"])


def test_every_mcp_id_has_a_tool_body_and_vice_versa():
    mcp_ids = {name for name, path in PATHS.items() if path == "/mcp"}
    assert mcp_ids == set(TOOL_BODIES), "an MCP id without a body would GET the stream"
