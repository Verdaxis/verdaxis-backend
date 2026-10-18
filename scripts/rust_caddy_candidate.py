#!/usr/bin/env python3
"""Create a private, one-group Caddy candidate and exact rollback; never reload."""
from __future__ import annotations

import argparse
import os
import re
from pathlib import Path

GROUPS = ("simple", "orderbook", "prices", "curves", "mcp")
IMPORT = re.compile(r"^(?P<prefix>[ \t]*import verdaxis_rust_public[ \t]+)(?P<upstreams>[^\r\n]+)(?P<end>\r?\n|$)", re.MULTILINE)


def candidate(original: str, environment: str, group: str) -> str:
    if environment not in {"production", "staging"} or group not in GROUPS:
        raise ValueError("unknown environment or route group")
    if environment == "production" and group == "mcp":
        raise ValueError("production MCP must remain on Python")
    matches = list(IMPORT.finditer(original))
    if len(matches) != 1:
        raise ValueError("exactly one full-line public import is required")
    match = matches[0]
    # Require the import's immediate enclosing block to be route, not handle
    # or the site itself. Full configuration semantics remain caddy validate's job.
    blocks = []
    for line in original[:match.start()].splitlines():
        code = line.split("#", 1)[0].strip()
        if code.endswith("{"):
            blocks.append(code[:-1].strip())
        elif code == "}" and blocks:
            blocks.pop()
    if not blocks or blocks[-1] != "route":
        raise ValueError("public import must be inside the reviewed route block")
    python, rust = (("8000", "8010") if environment == "production" else ("8001", "8011"))
    upstreams = match["upstreams"].split()
    allowed = {f"127.0.0.1:{python}", f"127.0.0.1:{rust}"}
    if len(upstreams) != 5 or any(value not in allowed for value in upstreams):
        raise ValueError("unexpected public upstream selection")
    if environment == "production" and upstreams[4] != f"127.0.0.1:{python}":
        raise ValueError("production MCP must remain on Python")
    index = GROUPS.index(group)
    if upstreams[index] != f"127.0.0.1:{python}":
        raise ValueError("selected group is not on its Python rollback upstream")
    # Replace the selected token only, preserving spacing and every other byte.
    tokens = list(re.finditer(r"\S+", match["upstreams"]))
    token = tokens[index]
    start = match.start("upstreams") + token.start()
    end = match.start("upstreams") + token.end()
    return original[:start] + f"127.0.0.1:{rust}" + original[end:]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--site", required=True, type=Path)
    parser.add_argument("--environment", required=True, choices=("staging", "production"))
    parser.add_argument("--group", required=True, choices=GROUPS)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    original = args.site.read_bytes()
    updated = candidate(original.decode("utf-8"), args.environment, args.group)
    args.output_dir.mkdir(mode=0o700, parents=False, exist_ok=False)
    for name, content in (("candidate.caddy", updated.encode()), ("rollback.caddy", original)):
        descriptor = os.open(args.output_dir / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(content)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
