"""Exact abilities ownership through a disposable local Caddy; no live services."""

import json
import os
import shutil
import subprocess
import time
from pathlib import Path

import httpx
import pytest

from tests.rust_contract.test_phase5_routing import free_port, owner, upstream

ROOT = Path(__file__).resolve().parents[2]
PATH = "/api/auth/me/abilities"
METHODS = ("GET", "HEAD", "OPTIONS", "POST", "PUT", "PATCH", "DELETE")
CADDYFILE = """\
{
    admin off
    auto_https off
}
import {$ABILITIES_SNIPPET}
http://:{$CADDY_PORT} {
    bind 127.0.0.1
    route {
        import verdaxis_rust_abilities {$RUST_UPSTREAM} {$PYTHON_UPSTREAM}
        handle {
            reverse_proxy {$PYTHON_UPSTREAM}
        }
    }
}
"""


@pytest.fixture(scope="module", params=("rust", "python", "rust-stopped"))
def site(request, tmp_path_factory):
    caddy = os.environ.get("CADDY_BIN") or shutil.which("caddy")
    assert caddy and Path(caddy).is_file(), "Set CADDY_BIN to the local Caddy executable"
    with upstream("python") as python, upstream("rust") as rust:
        selected = {"rust": rust, "python": python,
                    "rust-stopped": f"127.0.0.1:{free_port()}"}[request.param]
        port = free_port()
        work = tmp_path_factory.mktemp("abilities-routing")
        config = work / "Caddyfile"
        config.write_text(CADDYFILE)
        env = {**os.environ, "ABILITIES_SNIPPET": str(ROOT / "deploy/caddy/rust-abilities.caddy"),
               "CADDY_PORT": str(port), "RUST_UPSTREAM": selected, "PYTHON_UPSTREAM": python}
        subprocess.run([caddy, "adapt", "--config", str(config), "--validate"],
                       env=env, check=True, capture_output=True)
        with (work / "caddy.log").open("w") as log:
            process = subprocess.Popen([caddy, "run", "--config", str(config)],
                                       env=env, stdout=log, stderr=log)
            try:
                with httpx.Client(base_url=f"http://127.0.0.1:{port}", trust_env=False,
                                  follow_redirects=False, timeout=10) as client:
                    for _ in range(100):
                        assert process.poll() is None, (work / "caddy.log").read_text()
                        try:
                            client.get("/health/live")
                            break
                        except httpx.ConnectError:
                            time.sleep(0.05)
                    else:
                        pytest.fail("disposable Caddy did not start")

                    def send(method, path):
                        return client.request(method, path)

                    send.expected_owner = {"rust": "rust", "python": "python",
                                           "rust-stopped": "unavailable"}[request.param]
                    yield send
            finally:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def test_route_inventory():
    inventory = json.loads((ROOT / "tests/rust_contract/abilities/routes.json").read_text())
    assert inventory == {"owner": "rust", "schema_version": 1, "routes": [
        {"method": method, "path": PATH, "auth": "current_user"}
        for method in ("GET", "HEAD", "OPTIONS")
    ]}


@pytest.mark.parametrize("method", METHODS)
def test_exact_methods_and_disabled_equal_arguments(site, method):
    expected = site.expected_owner if method in ("GET", "HEAD", "OPTIONS") else "python"
    assert owner(site, method, PATH) == expected


@pytest.mark.parametrize("path", [
    "/api/auth/me", "/api/auth/me/abilities/", "/api/auth/me/abilities/extra",
    "/api/auth/me/ability", "/api/auth/me/abilitiesx", "/api/auth/Me/abilities",
    "/api/auth/me/Abilities", "/api/auth/me/password",
])
def test_neighbours_stay_on_python(site, path):
    for method in METHODS:
        assert owner(site, method, path) == "python"
