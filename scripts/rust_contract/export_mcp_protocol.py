"""Export protocol rejection vectors from the pinned Python SDK."""
import json
from pathlib import Path

from mcp.shared.inbound import classify_inbound_request
from mcp_types.jsonrpc import jsonrpc_message_adapter
from mcp_types.version import MODERN_PROTOCOL_VERSIONS
from pydantic import ValidationError

ROOT = Path(__file__).resolve().parents[2]


def vectors():
    base = {"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"get_market_slice", "arguments":{}}}
    for label, body in [("wrong-version", {**base,"jsonrpc":"1.0"}), ("missing-version", {k:v for k,v in base.items() if k != "jsonrpc"}), ("array-envelope", []), ("wrong-method", {**base,"method":42}), ("wrong-params", {**base,"params":[]})]:
        try:
            jsonrpc_message_adapter.validate_python(body, by_name=False)
        except ValidationError as error:
            yield {"name":label,"body":body,"headers":{},"error":{"code":-32602,"message":"Validation error: "+str(error)},"id":None}
    version_key = "io.modelcontextprotocol/protocolVersion"
    capability_key = "io.modelcontextprotocol/clientCapabilities"
    for label, meta, headers in [
        ("absent-meta", None, {}),
        ("empty-meta", {}, {}),
        ("partial-meta", {version_key:"2099-01-01"}, {}),
        ("version-mismatch", {version_key:"2026-07-28",capability_key:{}}, {}),
        ("method-mismatch", {version_key:"2099-01-01",capability_key:{}}, {}),
        ("unsupported-modern", {version_key:"2099-01-01",capability_key:{}}, {"mcp-method":"tools/list"}),
    ]:
        body={"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}
        if meta is not None: body["params"]["_meta"]=meta
        headers={"mcp-protocol-version":"2099-01-01",**headers}
        result=classify_inbound_request(body,headers=headers)
        error={"code":result.code,"message":result.message}
        if result.data is not None: error["data"]=result.data
        yield {"name":label,"body":body,"headers":headers,"error":error,"id":1}


if __name__ == "__main__":
    (ROOT/"deploy/rust/mcp-protocol-schema.json").write_text(json.dumps({"schema":jsonrpc_message_adapter.json_schema(),"modern_versions":list(MODERN_PROTOCOL_VERSIONS)},indent=2)+"\n")
    (ROOT/"deploy/rust/mcp-protocol-vectors.json").write_text(json.dumps(list(vectors()),indent=2)+"\n")
