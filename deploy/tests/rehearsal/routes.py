"""Observe every reviewed route group through Caddy and the actual Rust socket."""

import hashlib
import json
import os
import subprocess
import uuid


def request(url, method, private, label, *, ca=None, domain=None):
    """Keep response bodies private; curl cannot use ambient proxy settings."""
    body = private / f"{label}.body"
    arguments = [
        "curl",
        "--disable",
        "--silent",
        "--show-error",
        "--noproxy",
        "*",
        "--connect-timeout",
        "5",
        "--max-time",
        "20",
        "--max-filesize",
        "1048576",
        "--request",
        method,
        "--output",
        str(body),
        "--write-out",
        "%{http_code}",
    ]
    if ca is not None:
        arguments.extend(("--cacert", str(ca), "--proto", "=https"))
    else:
        arguments.extend(("--proto", "=http"))
    if domain is not None:
        arguments.extend(("--header", "Host: " + domain))
    # No credentials, cookies or payload are sent. Writes must refuse before
    # mutation; authenticated success cases are the separate session probe.
    arguments.extend(("--header", "Origin: https://" + domain, url))
    result = subprocess.run(
        arguments,
        capture_output=True,
        timeout=25,
        check=False,
        env={"PATH": "/usr/local/bin:/usr/bin:/bin", "LC_ALL": "C"},
    )
    if result.returncode != 0 or not body.is_file():
        raise RuntimeError("local route ownership request failed")
    status = int(result.stdout)
    if not 200 <= status < 500:
        raise RuntimeError("route ownership request returned a server error")
    data = body.read_bytes()
    try:
        normalized = json.dumps(
            json.loads(data), sort_keys=True, separators=(",", ":")
        ).encode()
    except (ValueError, UnicodeDecodeError):
        normalized = data
    return {"status": status, "response_sha256": hashlib.sha256(normalized).hexdigest()}


def verify(s):
    if os.geteuid() != 0 or os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError("route probes require the disposable hosted runner")
    import sys

    sys.path.insert(0, str(s.ROOT))
    from scripts.rust_phase10_window_caddy import probes

    directory = s.PRIVATE / ("route-probes-" + uuid.uuid4().hex)
    directory.mkdir(mode=0o700)
    process = s.process_identity(
        s.SHA, s.state()["artifacts"]["current"]["binary_sha256"]
    )
    rows = []
    for index, route in enumerate(probes(s.ENV)):
        proxied = request(
            f"https://{s.DOMAIN}{route['path']}",
            route["method"],
            directory,
            f"{index}-public",
            ca=s.PRIVATE / "caddy-ca.pem",
            domain=s.DOMAIN,
        )
        if route["expected_owner"] == "caddy-404":
            if proxied["status"] != 404:
                raise RuntimeError("retired route did not terminate at Caddy")
            direct = None
        else:
            direct = request(
                f"http://127.0.0.1:{s.PORT}{route['path']}",
                route["method"],
                directory,
                f"{index}-direct",
                domain=s.DOMAIN,
            )
            if proxied != direct:
                raise RuntimeError(
                    f"proxy and actual Rust response differ: {route['group']}"
                )
        rows.append(
            {
                "group": route["group"],
                "method": route["method"],
                "path": route["path"],
                "expected_owner": route["expected_owner"],
                "proxied": proxied,
                "direct": direct,
            }
        )
    result = {
        "scope": "disposable-candidate",
        "environment": s.ENV,
        "release_sha": s.SHA,
        "process": process,
        "routes": rows,
        "authenticated_writes": "separate session and CRUD checks",
    }
    s.write(s.PUBLIC / "routes.json", result)
    return result
