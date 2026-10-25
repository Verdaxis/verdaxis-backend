#!/usr/bin/env python3
"""Create a private one-switch Caddy candidate for the Phase 5 read groups.

Never reloads Caddy and never reads a live site file: the caller supplies the
saved site text. Method B keeps one import for every Phase 5 group, so the
candidate either moves the whole area to the Rust upstream or leaves the site
unchanged; there is no per-route or per-group candidate to rehearse. The
rollback of a deployed candidate is the site's previously saved file.
"""
from __future__ import annotations

import argparse
import re
from pathlib import Path

SNIPPET_NAME = "rust-phase5.caddy"
IMPORT = re.compile(
    r"^(?P<prefix>[ \t]*import verdaxis_rust_phase5[ \t]+)(?P<upstreams>[^\r\n]+)(?P<end>\r?\n|$)",
    re.MULTILINE,
)


def candidate(original: str, environment: str, target: str) -> str:
    if environment not in {"production", "staging"}:
        raise ValueError("unknown environment")
    if target not in {"rust", "python"}:
        raise ValueError("one switch: target is rust or python")
    python, rust = (("8000", "8010") if environment == "production" else ("8001", "8011"))
    matches = list(IMPORT.finditer(original))
    if len(matches) != 1:
        raise ValueError("exactly one Phase 5 import line is required")
    match = matches[0]
    upstreams = match["upstreams"].split()
    # Two arguments only: the Phase 5 upstream and the shared Python fallback.
    if len(upstreams) != 2 or upstreams[1] != f"127.0.0.1:{python}":
        raise ValueError("the Phase 5 import takes one Rust upstream and the Python fallback")
    wanted = f"127.0.0.1:{rust if target == 'rust' else python}"
    if upstreams[0] != wanted:
        tokens = list(re.finditer(r"\S+", match["upstreams"]))
        first = tokens[0]
        start = match.start("upstreams") + first.start()
        end = match.start("upstreams") + first.end()
        return original[:start] + wanted + original[end:]
    return original


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--site", required=True, type=Path, help="saved site file, never a live path")
    parser.add_argument("--environment", required=True, choices=("staging", "production"))
    parser.add_argument("--target", required=True, choices=("rust", "python"))
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    original = args.site.read_text(encoding="utf-8")
    args.output.write_text(candidate(original, args.environment, args.target), encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
