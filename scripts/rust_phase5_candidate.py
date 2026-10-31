#!/usr/bin/env python3
"""Create a private Caddy candidate and exact rollback for the Phase 5 read routes.

Run from the repository root: ``python -m scripts.rust_phase5_candidate``.

The Phase 5 routes move in one switch (see ``deploy/caddy/rust-phase5.caddy``).
Staging imports the snippet, so the candidate changes only its first argument.
Production keeps one site file without imports, so the candidate inserts the
snippet's body with the production upstreams between generated markers, and the
rollback removes exactly that block. The tool reads the saved site file it is
given, never a live path, and never reloads Caddy; the rollback file it writes
is the original, byte for byte.
"""
from __future__ import annotations

import argparse
import os
import re
from pathlib import Path

from scripts.rust_caddy_candidate import (
    ACCESS_LOG,
    blocks,
    enclosing,
    production_layout,
)

ROOT = Path(__file__).resolve().parents[1]
SNIPPET = ROOT / "deploy/caddy/rust-phase5.caddy"
SNIPPET_HEADER = "(verdaxis_rust_phase5)"
# Loopback upstream pair of each environment: (Python, Rust).
UPSTREAMS = {
    "staging": ("127.0.0.1:8001", "127.0.0.1:8011"),
    "production": ("127.0.0.1:8000", "127.0.0.1:8010"),
}
IMPORT = re.compile(
    r"^(?P<prefix>[ \t]*import verdaxis_rust_phase5[ \t]+)(?P<upstreams>[^\r\n]+)(?P<end>\r?\n|$)",
    re.MULTILINE,
)
BEGIN = "\t\t# BEGIN verdaxis-rust phase5\n"
END = "\t\t# END verdaxis-rust phase5\n"


def candidate(original: str, environment: str, target: str) -> str:
    """Return the site text with the Phase 5 routes on ``target`` (rust or python)."""
    if environment not in UPSTREAMS:
        raise ValueError("unknown environment")
    if target not in {"rust", "python"}:
        raise ValueError("one switch: target is rust or python")
    if environment == "production":
        return production_candidate(original, target)
    return staging_candidate(original, target)


def staging_candidate(original: str, target: str) -> str:
    python, rust = UPSTREAMS["staging"]
    matches = list(IMPORT.finditer(original))
    if len(matches) != 1:
        raise ValueError("exactly one full-line Phase 5 import is required")
    match = matches[0]
    route = enclosing(blocks(original), match.start())
    if route is None or route["header"] != "route":
        raise ValueError("Phase 5 import must be inside the reviewed route block")
    tokens = list(re.finditer(r"\S+", match["upstreams"]))
    # Two arguments only: the Phase 5 upstream and the shared Python fallback.
    if len(tokens) != 2 or tokens[1].group() != python:
        raise ValueError("the Phase 5 import takes one Rust upstream and the Python fallback")
    current = tokens[0].group()
    if current not in {python, rust}:
        raise ValueError("unexpected Phase 5 upstream selection")
    wanted = rust if target == "rust" else python
    if current == wanted:
        raise ValueError(f"the Phase 5 routes are already on {target}")
    start = match.start("upstreams") + tokens[0].start()
    end = match.start("upstreams") + tokens[0].end()
    return original[:start] + wanted + original[end:]


def production_block() -> str:
    """Inline the reviewed snippet with the production upstreams."""
    python, rust = UPSTREAMS["production"]
    snippet = SNIPPET.read_text()
    definition = blocks(snippet)[0]
    if definition["header"] != SNIPPET_HEADER:
        raise ValueError("unexpected Phase 5 snippet layout")
    lines = snippet[definition["body"]:definition["end"]].splitlines(keepends=True)
    if not lines or lines[-1].strip() != "}":
        raise ValueError("unexpected Phase 5 snippet end")
    body = "".join(lines[:-1]).replace("{args[0]}", rust).replace("{args[1]}", python)
    if "{args[" in body:
        raise ValueError("unexpected Phase 5 snippet arguments")
    return BEGIN + "".join("\t" + line for line in body.splitlines(keepends=True)) + END


def production_candidate(original: str, target: str) -> str:
    _, _, fallback = production_layout(original)
    block = production_block()
    present = original.count(BEGIN) + original.count(END)
    if target == "rust":
        if ACCESS_LOG not in original:
            raise ValueError("install and accept the redacted access log before cutover")
        if present:
            raise ValueError("the Phase 5 block is already present")
        return original[:fallback["start"]] + block + original[fallback["start"]:]
    if original.count(block) != 1 or present != 2:
        raise ValueError("expected one unchanged generated Phase 5 block")
    return original.replace(block, "", 1)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--site", required=True, type=Path, help="saved site file, never a live path")
    parser.add_argument("--environment", required=True, choices=tuple(UPSTREAMS))
    parser.add_argument("--target", required=True, choices=("rust", "python"))
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    original = args.site.read_bytes()
    updated = candidate(original.decode("utf-8"), args.environment, args.target)
    args.output_dir.mkdir(mode=0o700, parents=False, exist_ok=False)
    for name, content in (("candidate.caddy", updated.encode()), ("rollback.caddy", original)):
        descriptor = os.open(args.output_dir / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(content)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
