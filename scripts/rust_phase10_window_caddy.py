#!/usr/bin/env python3
"""Build and validate a private all-groups site from saved bytes; never activate.

Run with ``python -m scripts.rust_phase10_window_caddy --help``. The output is
one self-contained candidate plus exact rollback bytes. A trusted, absolute,
digest-bound Caddy executable is required. The tool never reloads Caddy.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import tempfile
from pathlib import Path

from scripts import rust_caddy_candidate as public
from scripts import rust_phase5_candidate as phase5

ROOT = Path(__file__).resolve().parents[1]
UPSTREAMS = phase5.UPSTREAMS
SNIPPETS = {
    "verdaxis_rust_auth": ("rust-auth.caddy", 1),
    "verdaxis_rust_abilities": ("rust-abilities.caddy", 2),
    "verdaxis_rust_offboarding": ("rust-offboarding.caddy", 2),
    "verdaxis_rust_crud": ("rust-crud.caddy", 8),
    "verdaxis_rust_phase4": ("rust-phase4.caddy", 1),
    "verdaxis_rust_phase5": ("rust-phase5.caddy", 2),
    "verdaxis_rust_public": ("rust-public-staging.caddy", 6),
    "verdaxis_rust_market": ("rust-market-write.caddy", 2),
    "verdaxis_rust_root_health": ("phase10-root-health.caddy", 1),
    "verdaxis_rust_monitor_canary": ("phase10-monitor-canary.caddy", 1),
}
PROBE_ROUTES = {
    "auth": ("POST", "/api/auth/login"),
    "abilities": ("GET", "/api/auth/me/abilities"),
    "offboarding": ("GET", "/api/auth/admin/users/00000000-0000-4000-8000-000000000001/offboarding-preview"),
    "preauth": ("GET", "/api/admin/feedback"),
    "notifications": ("GET", "/api/notifications"),
    "preferences": ("GET", "/api/users/me/preferences"),
    "feedback": ("POST", "/api/feedback"),
    "watchlists": ("GET", "/api/watchlists"),
    "alerts": ("GET", "/api/alerts"),
    "referrals": ("GET", "/api/referrals/my-referrals"),
    "phase4": ("GET", "/api/auth/me"),
    "phase5": ("GET", "/api/availability"),
    "simple": ("GET", "/api/ports"),
    "orderbook": ("GET", "/api/orderbook"),
    "prices": ("GET", "/api/prices"),
    "curves": ("GET", "/api/curves/forward"),
    "mcp": ("POST", "/mcp"),
    "market-write": ("POST", "/api/orderbook"),
    "root-health": ("GET", "/health/ready"),
    "monitor-canary": ("POST", "/api/monitor/signup-canary-cleanup"),
    "phase7-retired": ("POST", "/api/rfq"),
    "catch-all": ("GET", "/api/not-a-route"),
}


def code(text: str) -> str:
    """Compare reviewed directives without layout or full-line comments."""
    return "\n".join(line.strip() for line in text.splitlines()
                     if line.strip() and not line.lstrip().startswith("#"))


def snippet_body(name: str, arguments: list[str]) -> str:
    source = (ROOT / "deploy/caddy" / name).read_text()
    if name == "phase10-phase7-retire.caddy":
        body = source
    else:
        definition = public.blocks(source)[0]
        body = source[definition["body"]:definition["end"]]
        body = body[:body.rfind("}")]
    body = "\n".join(line for line in body.splitlines() if not line.lstrip().startswith("#")) + "\n"
    for index, argument in enumerate(arguments):
        body = body.replace(f"{{args[{index}]}}", argument)
    if "{args[" in body:
        raise ValueError("unresolved snippet arguments")
    return body


def expand_imports(original: str, environment: str) -> str:
    python, rust = UPSTREAMS[environment]
    def expand(match):
        words = match.group(1).split()
        if not words or words[0] not in SNIPPETS:
            raise ValueError("unreviewed import; supply a saved self-contained site")
        name, count = SNIPPETS[words[0]]
        if len(words) != count + 1 or any(arg not in {python, rust} for arg in words[1:]):
            raise ValueError("unexpected import arguments")
        return snippet_body(name, words[1:])
    return re.sub(r"^[ \t]*import[ \t]+([^\r\n]+)\r?$", expand, original, flags=re.MULTILINE)


def reviewed_children(environment: str) -> set[str]:
    """Each old route child must come from a released snippet, not an unknown rule."""
    python, rust = UPSTREAMS[environment]
    allowed = set()
    for name, count in SNIPPETS.values():
        for owner in (python, rust):
            args = [owner] * count
            if count > 1:
                args[-1] = python
            if name == "rust-crud.caddy":
                args[6] = rust
            body = snippet_body(name, args)
            for node in public.blocks(body):
                allowed.add(code(body[node["start"]:node["end"]]))
    if environment == "production":
        for group in public.READ_GROUPS:
            body = public.production_group(group)
            allowed.update(code(body[n["start"]:n["end"]]) for n in public.blocks(body))
        body = phase5.production_block()
        allowed.update(code(body[n["start"]:n["end"]]) for n in public.blocks(body))
    retired = snippet_body("phase10-phase7-retire.caddy", [])
    allowed.update(code(retired[n["start"]:n["end"]]) for n in public.blocks(retired))
    return allowed


def candidate(original: str, environment: str) -> str:
    if environment not in UPSTREAMS:
        raise ValueError("unknown environment")
    # Normalize line endings for parsing only; rollback always uses saved bytes.
    original = expand_imports(original.replace("\r\n", "\n"), environment)
    python, rust = UPSTREAMS[environment]
    site_name = "api.verdaxis.exchange" if environment == "production" else "api-staging.verdaxis.exchange"
    site = public.one(public.blocks(original), site_name)
    route = public.one(site["children"], "route")
    fallback = public.one(route["children"], "handle")
    if route["children"][-1] is not fallback:
        raise ValueError("expected a final catch-all")
    standalone = {"request_header -X-Verdaxis-Preauth-Counted"}
    for name, count in SNIPPETS.values():
        args = [rust] * count
        if count > 1:
            args[-1] = python
        for line in snippet_body(name, args).splitlines():
            if line.strip().startswith("@") and not line.rstrip().endswith("{"):
                standalone.add(line.strip())
    cursor = route["body"]
    for node in route["children"]:
        if any(line not in standalone for line in code(original[cursor:node["start"]]).splitlines()):
            raise ValueError("unreviewed directive outside route handles")
        cursor = node["end"]
    closing_line = original.rfind("\n", route["body"], route["end"] - 1) + 1
    if code(original[cursor:closing_line]):
        raise ValueError("unreviewed directive after final catch-all")
    fallback_text = original[fallback["start"]:fallback["end"]]
    proxies = public.python_proxies(fallback_text, python.rsplit(":", 1)[1])
    if len(proxies) != 1:
        raise ValueError("expected exactly one saved Python catch-all")
    if environment == "production":
        public.production_layout(original)
        if public.ACCESS_LOG not in original:
            raise ValueError("accept redacted access logging before the window")
    for name in ("rust-abilities.caddy", "rust-offboarding.caddy"):
        if code(snippet_body(name, [rust, python])) not in code(original):
            raise ValueError("saved site must contain complete live abilities and offboarding")
    allowed = reviewed_children(environment)
    keep = []
    live = {"abilities": False, "offboarding": False}
    for node in route["children"][:-1]:
        text = original[node["start"]:node["end"]]
        if node["header"] in {"handle /script.js", "handle /api/send"}:
            keep.append(text)
            continue
        if code(text) not in allowed:
            raise ValueError("unreviewed route child: " + node["header"])
        for group in live:
            if "rust_" + group in node["header"]:
                if python in text and "reverse_proxy " + python in text:
                    raise ValueError("already-live group must select Rust")
                live[group] = True
                keep.append(text)
    if not all(live.values()):
        raise ValueError("saved site must contain live abilities and offboarding")
    # Reuse the existing production composers, then remove their retired dial
    # targets only on reverse_proxy lines. Expressions keep the old owner token
    # as a disabled-state sentinel, not as an active upstream.
    groups = snippet_body("rust-auth.caddy", [rust])
    crud = public.production_crud_block()
    for group in public.CRUD_GROUPS:
        crud = public.crud_group_swap(crud, group, "127.0.0.1:8000", "127.0.0.1:8010")
    groups += crud + snippet_body("rust-phase4.caddy", [rust])
    groups += phase5.production_block()
    groups += "".join(public.production_group(group) for group in public.READ_GROUPS)
    groups += snippet_body("rust-market-write.caddy", [rust, python])
    groups += snippet_body("phase10-root-health.caddy", [rust])
    groups += snippet_body("phase10-monitor-canary.caddy", [rust])
    groups += snippet_body("phase10-phase7-retire.caddy", [])
    if environment == "staging":
        groups = groups.replace("127.0.0.1:8000", python).replace("127.0.0.1:8010", rust)
    # MCP and all unlisted paths reach the Rust catch-all; production MCP keeps
    # the runtime's production-disabled response instead of a Python dial.
    def remove_python(match):
        line = match.group(0)
        return re.sub(r"[ \t]+(?:127\.0\.0\.1|localhost):" + python.rsplit(":", 1)[1] + r"\b", "", line)
    groups = re.sub(r"^[ \t]*reverse_proxy[^\n]*", remove_python, groups, flags=re.MULTILINE)
    proxy = proxies[0]
    fallback_text = fallback_text[:proxy.start("upstreams")] + rust + fallback_text[proxy.end("upstreams"):]
    # Strip client-supplied internal marks before any preserved terminal handle.
    body = "\t\trequest_header -X-Verdaxis-Preauth-Counted\n" + "".join(keep) + groups + fallback_text
    updated = original[:route["body"]] + body + original[closing_line:]
    # No include or active dial may retain the stopped process.
    for line in updated.splitlines():
        if line.strip().startswith("reverse_proxy") and re.search(r"(?:127\.0\.0\.1|localhost):" + python.rsplit(":", 1)[1] + r"\b", line):
            raise ValueError("candidate retains a Python upstream")
    return updated


def probes(environment: str) -> list[dict]:
    return [{"group": group, "method": method, "path": path,
             "expected_owner": "caddy-404" if group == "phase7-retired" else "rust",
             "environment": environment,
             "acceptance": "use disposable authorized fixtures; verify status, identity and response contract"}
            for group, (method, path) in PROBE_ROUTES.items()]


def validate(config: Path, binary: Path, digest: str) -> dict:
    if not binary.is_absolute() or binary.is_symlink() or not binary.is_file():
        raise ValueError("Caddy must be an absolute regular trusted executable")
    metadata = binary.stat()
    if metadata.st_mode & 0o022 or not os.access(binary, os.X_OK):
        raise ValueError("Caddy must not be group/world writable and must be executable")
    if not re.fullmatch(r"[0-9a-f]{64}", digest) or hashlib.sha256(binary.read_bytes()).hexdigest() != digest:
        raise ValueError("Caddy binary digest mismatch")
    config = config.resolve(strict=True)
    result = {"binary": str(binary), "binary_sha256": digest}
    # Isolate default HOME/XDG storage and remap file log writers below.
    # This is not a filesystem sandbox for arbitrary Caddy modules.
    # Validation does not start the server.
    env = {"PATH": "/usr/bin:/bin", "HOME": str(config.parent),
           "XDG_DATA_HOME": str(config.parent / "data"),
           "XDG_CONFIG_HOME": str(config.parent / "config")}
    def run(action: str, checked_config: Path, adapter: bool = False) -> bytes:
        command = [str(binary), action, "--config", str(checked_config)]
        if adapter:
            command += ["--adapter", "caddyfile"]
        completed = subprocess.run(command, cwd=config.parent, env=env,
                                   capture_output=True, timeout=60)
        if completed.returncode:
            raise ValueError("Caddy " + action + " failed: " + completed.stderr.decode(errors="replace"))
        return completed.stdout

    # Adapt the exact candidate first. Operate on typed JSON writer modules,
    # not Caddyfile spelling: blockless, quoted and multiline file writers all
    # become the same module. Only known non-file writers can remain active.
    adapted = json.loads(run("adapt", config, adapter=True))
    if "apps" not in adapted:
        raise ValueError("Caddy adapt returned no applications")
    result["adapt"] = "passed"
    mappings = {}
    for logger in adapted.get("logging", {}).get("logs", {}).values():
        writer = logger.get("writer", {})
        output = writer.get("output", "stderr")
        if output == "file":
            filename = writer.get("filename")
            if not isinstance(filename, str) or not filename:
                raise ValueError("file log writer has no filename")
            destination = config.parent / f"validation-log-{len(mappings)}.jsonl"
            mappings[filename] = str(destination)
            writer["filename"] = str(destination)
        elif output not in {"stdout", "stderr", "discard"}:
            raise ValueError("unsupported validation log writer: " + str(output))
    validation_config = config.parent / "validation.json"
    private_write(validation_config, json.dumps(adapted).encode())
    run("validate", validation_config)
    result["private_log_destinations"] = mappings
    result["validate"] = "passed"
    return result


def private_write(path: Path, data: bytes) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(data)


def create_bundle(site: Path, environment: str, output: Path, binary: Path, digest: str) -> None:
    if site.is_symlink() or not site.is_file():
        raise ValueError("saved site must be a regular file")
    if output.resolve().is_relative_to(ROOT):
        raise ValueError("output must be outside the source tree")
    original = site.read_bytes()
    updated = candidate(original.decode("utf-8"), environment).encode()
    output.mkdir(mode=0o700, parents=False, exist_ok=False)
    private_write(output / "rollback.caddy", original)
    private_write(output / "candidate.caddy", updated)
    private_write(output / "probes.json", json.dumps(probes(environment), indent=2).encode() + b"\n")
    with tempfile.TemporaryDirectory(prefix=".caddy-check-", dir=output) as workspace:
        validation_site = Path(workspace) / "candidate.caddy"
        private_write(validation_site, updated)
        validation = validate(validation_site, binary, digest)
    manifest = {"schema_version": 1, "environment": environment,
                "candidate_sha256": hashlib.sha256(updated).hexdigest(),
                "rollback_sha256": hashlib.sha256(original).hexdigest(),
                "files": ["candidate.caddy", "rollback.caddy", "probes.json"],
                "validation": validation, "activation": "not performed"}
    private_write(output / "manifest.json", json.dumps(manifest, indent=2).encode() + b"\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--site", required=True, type=Path, help="saved complete site, never a live path")
    parser.add_argument("--environment", required=True, choices=tuple(UPSTREAMS))
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--caddy-bin", required=True, type=Path)
    parser.add_argument("--caddy-sha256", required=True)
    args = parser.parse_args()
    create_bundle(args.site, args.environment, args.output_dir, args.caddy_bin, args.caddy_sha256)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
