"""Test bridge: compare typed tool payloads through the shared authority."""

import json
import sys

from scripts.rust_contract.compare import compare_responses
from scripts.rust_contract.phase2_meaning import PHASE1_RULES_SHA256


def main():
    pair = json.load(sys.stdin)
    def response(payload):
        return {"status": 200, "headers": {"content-type": "application/json"},
                "body": json.dumps({"jsonrpc": "2.0", "id": 1, "result": {"structuredContent": payload, "isError": False}}).encode()}
    if "validation_tool" in pair:
        from app.mcp_server import build_mcp_server
        metadata = build_mcp_server(read_limiter=None)._tool_manager.get_tool(pair["validation_tool"]).fn_metadata
        expected = metadata.arg_model.model_validate(pair["expected"])
        actual = metadata.arg_model.model_validate(pair["actual"])
        return 0 if expected == actual else 1
    context = pair.get("diagnostic_context")
    if context is not None:
        def diagnostic(text):
            return {"status": 200, "headers": {"content-type": "application/json"},
                    "body": json.dumps({"jsonrpc": "2.0", "id": 1, "result": {"content": [{"type": "text", "text": text}], "isError": True}}).encode()}
        left = diagnostic(f"Error executing tool {context['tool']}: {pair['expected']}")
        right = diagnostic(pair["actual"])
    elif pair.get("raw_envelope"):
        def envelope(value):
            return {"status": 200, "headers": {"content-type": "application/json"}, "body": json.dumps(value).encode()}
        left, right = envelope(pair["expected"]), envelope(pair["actual"])
    else:
        left, right = response(pair["expected"]), response(pair["actual"])
    differences = compare_responses(left, right,
                                    profile="phase1-meaning-v1", route_id=pair["route_id"],
                                    rules_sha256=PHASE1_RULES_SHA256, diagnostic_context=context)
    if differences:
        print("\n".join(differences), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
