#!/usr/bin/env python3
"""Create a private, one-group Caddy candidate and exact rollback; never reload."""
from __future__ import annotations

import argparse
import os
import re
from pathlib import Path

GROUPS = ("simple", "orderbook", "prices", "curves", "mcp")
READ_GROUPS = GROUPS[:-1]
IMPORT = re.compile(r"^(?P<prefix>[ \t]*import verdaxis_rust_public[ \t]+)(?P<upstreams>[^\r\n]+)(?P<end>\r?\n|$)", re.MULTILINE)
PROXY = re.compile(r"^(?P<indent>[ \t]*)reverse_proxy[ \t]+(?P<upstreams>[^{]+?)[ \t]*\{[ \t]*$", re.MULTILINE)


def candidate(original: str, environment: str, group: str) -> str:
    if environment not in {"production", "staging"}:
        raise ValueError("unknown environment")
    if group == "fallback":
        return (production_fallback_candidate(original) if environment == "production"
                else staging_fallback_candidate(original, environment))
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
    # The sixth argument is the shared Python fallback for the four read groups.
    if len(upstreams) != 6 or any(value not in allowed for value in upstreams[:5]):
        raise ValueError("unexpected public upstream selection")
    if upstreams[5] != f"127.0.0.1:{python}":
        raise ValueError("read groups require the Python fallback upstream")
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


def enclosing(nodes: list[dict], offset: int) -> dict | None:
    """Return the deepest block whose body contains offset."""
    for node in nodes:
        if node["start"] < offset < node["end"]:
            return enclosing(node["children"], offset) or node
    return None


def python_proxies(text: str, python: str) -> list[re.Match]:
    """Find proxies whose sole upstream is the environment's Python process."""
    allowed = {f"127.0.0.1:{python}", f"localhost:{python}"}
    return [match for match in PROXY.finditer(text)
            if len(match["upstreams"].split()) == 1
            and match["upstreams"].split()[0] in allowed]


def with_python_retry(text: str, start: int, end: int, python: str) -> str:
    """Give the catch-all Python proxy a bounded wait across a restart.

    A dial failure is retried for lb_try_duration. No passive health check is
    added here: with one upstream, a remembered failure would make Caddy stop
    selecting it, and the request would fail before the restart completed.
    """
    window = text[start:end]
    if "lb_try_duration" in window or "lb_try_interval" in window:
        raise ValueError("Python retry window already present")
    matches = python_proxies(window, python)
    if len(matches) != 1:
        raise ValueError("expected exactly one Python catch-all proxy")
    match = matches[0]
    line_end = window.find("\n", match.end())
    if line_end < 0:
        raise ValueError("Python catch-all proxy line is incomplete")
    eol = "\r\n" if window[:line_end].endswith("\r") else "\n"
    insertion = (f"{match['indent']}\tlb_try_duration 15s{eol}"
                 f"{match['indent']}\tlb_try_interval 250ms{eol}")
    at = start + line_end + 1
    return text[:at] + insertion + text[at:]


def production_fallback_candidate(original: str) -> str:
    _, _, fallback = production_layout(original)
    return with_python_retry(original, fallback["start"], fallback["end"], "8000")


def staging_fallback_candidate(original: str, environment: str) -> str:
    if environment != "staging":
        raise ValueError("staging fallback candidate requires the staging environment")
    matches = list(IMPORT.finditer(original))
    if len(matches) != 1:
        raise ValueError("exactly one full-line public import is required")
    match = matches[0]
    python, rust = "8001", "8011"
    upstreams = match["upstreams"].split()
    if (len(upstreams) != 6 or upstreams[5] != f"127.0.0.1:{python}"
            or any(value not in {f"127.0.0.1:{python}", f"127.0.0.1:{rust}"} for value in upstreams[:5])):
        raise ValueError("unexpected public upstream selection")
    route = enclosing(blocks(original), match.start())
    if route is None or route["header"] != "route":
        raise ValueError("public import must be inside the reviewed route block")
    handles = [node for node in route["children"] if node["header"] == "handle"]
    catchalls = [node for node in handles
                 if python_proxies(original[node["start"]:node["end"]], python)]
    # The bare handle must be the final route child; anything after it is
    # unreachable and means the reviewed layout is not the expected one.
    if len(catchalls) != 1 or route["children"][-1] is not catchalls[0]:
        raise ValueError("expected exactly one final Python catch-all")
    return with_python_retry(original, catchalls[0]["start"], catchalls[0]["end"], python)


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
\t\t\t\t# The activity stream sends a short-lived token in its query string.
\t\t\t\trequest>uri query {
\t\t\t\t\tdelete stream_token
\t\t\t\t}
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
    definition = blocks(snippet)[0]
    handle = one(definition["children"], f"handle @rust_{group}")
    body = snippet[sum(len(line) for line in lines[:start]):handle["end"]]
    index = GROUPS.index(group)
    placeholder = f"reverse_proxy {{args[{index}]}} {{args[5]}} {{"
    if body.count(placeholder) != 1:
        raise ValueError("unexpected group proxy layout")
    # Rust first, the unchanged Python process second. lb_policy first keeps
    # that reviewed order; the snippet carries the retry and passive-health
    # options, which apply to whichever upstream is selected.
    body = body.replace(placeholder, "reverse_proxy 127.0.0.1:8010 127.0.0.1:8000 {\n\t\t\theader_up X-Forwarded-For {client_ip}")
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
    parser.add_argument("--group", required=True, choices=(*GROUPS, "access-log", "fallback"))
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
