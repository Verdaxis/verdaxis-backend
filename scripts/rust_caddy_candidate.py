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
    if environment == "production":
        return production_candidate(original, group)
    return staging_candidate(original, environment, group)


def staging_candidate(original: str, environment: str, group: str) -> str:
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


def blocks(text: str) -> list[dict]:
    """Read line-oriented Caddy blocks, not placeholder braces within directives.

    This intentionally accepts only the reviewed multi-line layout. Caddy itself
    remains the syntax/semantic authority in the required validate/adapt gate.
    """
    roots, stack = [], []
    offset = 0
    for line in text.splitlines(keepends=True):
        code = line.split("#", 1)[0].strip()
        if code.endswith("{"):
            node = {"header": code[:-1].strip(), "start": offset,
                    "body": offset + len(line), "children": [],
                    "indent": line[:len(line) - len(line.lstrip())]}
            (stack[-1]["children"] if stack else roots).append(node)
            stack.append(node)
        elif code == "}":
            if not stack:
                raise ValueError("unbalanced Caddy block")
            node = stack.pop()
            node["end"] = offset + len(line)
        offset += len(line)
    if stack:
        raise ValueError("unclosed Caddy block")
    return roots


def one(nodes: list[dict], header: str) -> dict:
    found = [node for node in nodes if node["header"] == header]
    if len(found) != 1:
        raise ValueError(f"expected exactly one {header} block")
    return found[0]


def production_layout(original: str) -> tuple[dict, dict, dict]:
    if re.search(r"^\s*import\s", original, re.MULTILINE):
        raise ValueError("production requires the reviewed single-file layout")
    site = one(blocks(original), "api.verdaxis.exchange")
    route = one(site["children"], "route")
    children = route["children"]
    fallback = one(children, "handle")
    script = one(children, "handle /script.js")
    analytics = one(children, "handle /api/send")
    if not (script["start"] < analytics["start"] < fallback["start"]
            and children[-1] is fallback):
        raise ValueError("analytics must precede the final Python handle")
    expected = ("reverse_proxy localhost:8000", "header_up X-Forwarded-For {client_ip}",
                "flush_interval -1", "keepalive off")
    body = original[fallback["start"]:fallback["end"]]
    if any(value not in body for value in expected):
        raise ValueError("unexpected production Python proxy options")
    return site, route, fallback


ACCESS_LOG = """\t# BEGIN verdaxis-rust access-log
\tlog {
\t\toutput file /var/log/caddy/api.verdaxis.exchange.log {
\t\t\troll_size 20MiB
\t\t\troll_keep 3
\t\t}
\t\tformat filter {
\t\t\twrap json
\t\t\tfields {
\t\t\t\trequest>headers>Authorization delete
\t\t\t\trequest>headers>Cookie delete
\t\t\t\trequest>headers>X-Monitor-Token delete
\t\t\t\tresp_headers>Set-Cookie delete
\t\t\t}
\t\t}
\t}
\t# END verdaxis-rust access-log
"""


def production_group(group: str) -> str:
    if group not in GROUPS[:-1]:
        raise ValueError("production supports only the four public GET groups")
    snippet = (Path(__file__).resolve().parents[1] / "deploy/caddy/rust-public-staging.caddy").read_text()
    lines = snippet.splitlines(keepends=True)
    start = next(i for i, line in enumerate(lines) if line.strip() == f"@rust_{group} {{")
    stop = next(i for i in range(start, len(lines)) if lines[i].strip() == f"handle @rust_{group} {{") + 3
    body = "".join(lines[start:stop])
    index = GROUPS.index(group)
    body = body.replace(f"reverse_proxy {{args[{index}]}}", """reverse_proxy 127.0.0.1:8010 {
\t\t\theader_up X-Forwarded-For {client_ip}
\t\t\tflush_interval -1
\t\t\ttransport http {
\t\t\t\tkeepalive off
\t\t\t}
\t\t}""")
    body = "".join("\t" + line for line in body.splitlines(keepends=True))
    return f"\t\t# BEGIN verdaxis-rust {group}\n" + body + f"\t\t# END verdaxis-rust {group}\n"


def production_candidate(original: str, group: str) -> str:
    site, route, fallback = production_layout(original)
    if group == "access-log":
        if any(node["header"].split()[0] == "log" for node in site["children"]):
            raise ValueError("API access log already exists; review it instead")
        # Only this stale comment is removed; arbitrary operator comments stay.
        start, end = site["start"], site["end"]
        site_text = original[start:end].replace("\t# Temporary log block removed after diagnosis.\n", "")
        insertion = site_text.index("\n") + 1
        return original[:start] + site_text[:insertion] + ACCESS_LOG + site_text[insertion:] + original[end:]
    if ACCESS_LOG not in original:
        raise ValueError("install and accept the redacted access log before cutover")
    block = production_group(group)
    if f"@rust_{group}" in original or f"# BEGIN verdaxis-rust {group}" in original:
        raise ValueError("group already present")
    return original[:fallback["start"]] + block + original[fallback["start"]:]


def rollback(original: str, environment: str, group: str) -> str:
    if environment != "production":
        raise ValueError("staging uses its exact saved rollback file")
    production_layout(original)
    block = production_group(group)
    if original.count(block) != 1:
        raise ValueError("expected one unchanged generated group block")
    return original.replace(block, "", 1)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--site", required=True, type=Path)
    parser.add_argument("--environment", required=True, choices=("staging", "production"))
    parser.add_argument("--group", required=True, choices=(*GROUPS, "access-log"))
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
