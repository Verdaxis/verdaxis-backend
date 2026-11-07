#!/usr/bin/env python3
"""Install and activate the actual disposable saved site and complete candidate."""

import json
import shutil
from pathlib import Path


def saved_site(root, environment):
    import sys

    sys.path.insert(0, str(root))
    from scripts import rust_caddy_candidate as public
    from scripts import rust_phase10_window_caddy as composer

    domain = (
        "api.verdaxis.exchange"
        if environment == "production"
        else "api-staging.verdaxis.exchange"
    )
    legacy, rust = composer.UPSTREAMS[environment]
    live = "".join(
        composer.snippet_body(name, [rust, legacy])
        for name in ("rust-abilities.caddy", "rust-offboarding.caddy")
    )
    analytics = ""
    access_log = ""
    if environment == "production":
        access_log = public.ACCESS_LOG
        analytics = (
            "\t\thandle /script.js {\n\t\t\treverse_proxy localhost:8700\n\t\t}\n"
            "\t\thandle /api/send {\n\t\t\t@allowed_origin expression `{header.Origin} in ['https://app.verdaxis.exchange', 'https://verdaxis.exchange', 'https://canary.verdaxis.exchange']`\n"
            "\t\t\thandle @allowed_origin {\n\t\t\t\trequest_body {\n\t\t\t\t\tmax_size 64KB\n\t\t\t\t}\n\t\t\t\treverse_proxy localhost:8700\n\t\t\t}\n"
            "\t\t\thandle {\n\t\t\t\trespond 403\n\t\t\t}\n\t\t}\n"
        )
    return (
        f"{domain} {{\n\ttls internal\n"
        + access_log
        + "\troute {\n"
        + analytics
        + live
        + f"\t\thandle {{\n\t\t\treverse_proxy localhost:{8000 if environment == 'production' else 8001} {{\n"
        "\t\t\t\theader_up X-Forwarded-For {client_ip}\n\t\t\t\tflush_interval -1\n"
        "\t\t\t\ttransport http {\n\t\t\t\t\tkeepalive off\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n}\n"
    )


def prepare(s):
    directory = Path("/etc/caddy")
    directory.mkdir(mode=0o755, exist_ok=True)
    run_dir = Path("/var/lib/caddy")
    run_dir.mkdir(mode=0o700, exist_ok=True)
    s.run("chown", "-R", "caddy:caddy", run_dir)
    # The saved site is constructed for this disposable stack, not represented
    # as a captured live host site. Its dials terminate in actual old releases.
    saved = saved_site(s.ROOT, s.ENV)
    # The companion API exists for the old two-environment monitor. It dials
    # its actual Rust service directly and is not part of this window switch.
    companion_domain = (
        "api-staging.verdaxis.exchange"
        if s.ENV == "production"
        else "api.verdaxis.exchange"
    )
    companion_port = 8011 if s.ENV == "production" else 8010
    saved += f"\n{companion_domain} {{\n\ttls internal\n\treverse_proxy 127.0.0.1:{companion_port}\n}}\n"
    # Only the API implementation is under rehearsal. These explicit local
    # static sites model the other fixed monitor origins without any host
    # access or claim that the deployed frontend was exercised.
    for domain, api in (
        ("verdaxis.exchange", "api.verdaxis.exchange"),
        ("app.verdaxis.exchange", "api.verdaxis.exchange"),
        ("staging.verdaxis.exchange", "api-staging.verdaxis.exchange"),
        ("deck.verdaxis.exchange", "api.verdaxis.exchange"),
    ):
        saved += f'\n{domain} {{\n\ttls internal\n\trespond "Sign In https://{api}/api disposable frontend" 200\n}}\n'
    Path("/var/log/caddy").mkdir(mode=0o750, exist_ok=True)
    s.run("chown", "caddy:caddy", "/var/log/caddy")
    # Both literal API names resolve locally, including trusted pre-M curl
    # calls that intentionally do not accept a URL override.
    with Path("/etc/hosts").open("a") as stream:
        stream.write(
            "\n127.0.0.1 verdaxis.exchange app.verdaxis.exchange staging.verdaxis.exchange api.verdaxis.exchange api-staging.verdaxis.exchange deck.verdaxis.exchange\n"
        )
    s.write(directory / "Caddyfile", saved, 0o644)
    s.write(
        "/etc/systemd/system/caddy.service",
        """[Unit]
Description=Disposable real Caddy API
After=network.target
[Service]
User=caddy
Group=caddy
Environment=HOME=/var/lib/caddy
Environment=XDG_DATA_HOME=/var/lib/caddy/data
Environment=XDG_CONFIG_HOME=/var/lib/caddy/config
ExecStart=/usr/local/bin/caddy run --config /etc/caddy/Caddyfile --adapter caddyfile
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
Restart=on-failure
""",
        0o644,
    )
    s.run("systemctl", "daemon-reload")
    s.run("systemctl", "start", "caddy.service")
    ca = run_dir / "data/caddy/pki/authorities/local/root.crt"
    import time

    for _ in range(30):
        if ca.is_file():
            break
        time.sleep(1)
    if not ca.is_file():
        raise RuntimeError("real Caddy internal certificate authority missing")
    shutil.copyfile(ca, s.PRIVATE / "caddy-ca.pem")
    shutil.copyfile(ca, "/usr/local/share/ca-certificates/verdaxis-rehearsal.crt")
    s.run("update-ca-certificates")
    from scripts.rust_phase10_window_caddy import create_bundle

    create_bundle(
        directory / "Caddyfile",
        s.ENV,
        s.PRIVATE / "caddy-candidate",
        Path("/usr/local/bin/caddy"),
        s.digest("/usr/local/bin/caddy"),
    )
    return {
        "saved_sha256": s.digest(directory / "Caddyfile"),
        "candidate_sha256": s.digest(s.PRIVATE / "caddy-candidate/candidate.caddy"),
    }


def activate(s):
    source = s.PRIVATE / "caddy-candidate/candidate.caddy"
    shutil.copyfile(source, "/etc/caddy/Caddyfile")
    Path("/etc/caddy/Caddyfile").chmod(0o644)
    s.run(
        "runuser",
        "-u",
        "caddy",
        "--",
        "/usr/local/bin/caddy",
        "validate",
        "--config",
        "/etc/caddy/Caddyfile",
        "--adapter",
        "caddyfile",
    )
    s.run(
        "runuser",
        "-u",
        "caddy",
        "--",
        "/usr/local/bin/caddy",
        "reload",
        "--config",
        "/etc/caddy/Caddyfile",
        "--adapter",
        "caddyfile",
    )
    adapted = json.loads(
        s.run(
            "/usr/local/bin/caddy",
            "adapt",
            "--config",
            "/etc/caddy/Caddyfile",
            "--adapter",
            "caddyfile",
        )
    )
    dials = []

    def walk(node):
        if isinstance(node, dict):
            if "dial" in node:
                dials.append(node["dial"])
            for child in node.values():
                walk(child)
        elif isinstance(node, list):
            for child in node:
                walk(child)

    walk(adapted)
    if any(dial.rsplit(":", 1)[-1] in ("8000", "8001") for dial in dials):
        raise RuntimeError("active candidate retains a legacy upstream dial")
    s.write(
        s.PUBLIC / "caddy.json",
        {
            "scope": "disposable-candidate",
            "candidate_sha256": s.digest(source),
            "active_sha256": s.digest("/etc/caddy/Caddyfile"),
            "dials": dials,
            "adapted_sha256": __import__("hashlib")
            .sha256(json.dumps(adapted, sort_keys=True).encode())
            .hexdigest(),
        },
    )
    from routes import verify

    verify(s)
