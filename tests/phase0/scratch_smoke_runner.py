"""Run the scratch business smoke against a disposable container backend.

This mirrors the Task 9 HTTP harness (same pinned image, same server entry point,
same ACL setup) but calls the new script as an external HTTP client instead of
running the pytest integration suite. It does not change the harness gate.
"""
import os
import subprocess
import sys
import time
import uuid
from pathlib import Path

import httpx

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tests.phase0.runner import IMAGE

# The app's test environment requires a disposable *_test name. The rehearsal
# uses the exact production-mode name in a separate scratch cluster.
DATABASE = "p0_scratch_smoke_test"
PASSWORD = uuid.uuid4().hex
ITEST_PASSWORD = "Synthetic-Scratch-Password-42!"
name = "verdaxis-p0smoke-" + uuid.uuid4().hex[:10]
run = ROOT / "work/scratch-smoke"
run.mkdir(parents=True, exist_ok=True)
server = None
log = None


def cmd(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, **kwargs)


def main() -> int:
    global server, log
    cmd("docker", "run", "-d", "--platform", "linux/amd64", "--name", name,
        "-e", f"POSTGRES_PASSWORD={PASSWORD}", "-p", "127.0.0.1::5432", IMAGE)
    for _ in range(240):
        if subprocess.run(["docker", "exec", name, "pg_isready", "-h", "127.0.0.1", "-U", "postgres"],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
            break
        time.sleep(1)
    else:
        raise RuntimeError("database not ready")
    port = cmd("docker", "port", name, "5432/tcp", capture_output=True).stdout.strip().split(":")[-1]
    cmd("docker", "exec", name, "psql", "-U", "postgres", "-v", "ON_ERROR_STOP=1", "-c",
        f"CREATE ROLE phase0_owner LOGIN NOINHERIT PASSWORD '{PASSWORD}'")
    cmd("docker", "exec", name, "createdb", "-U", "postgres", "-O", "phase0_owner", DATABASE)
    cmd("docker", "exec", name, "psql", "-U", "postgres", "-d", DATABASE, "-c", "CREATE EXTENSION postgis")
    owner_url = f"postgresql+asyncpg://phase0_owner:{PASSWORD}@127.0.0.1:{port}/{DATABASE}"
    python = str(ROOT / "venv/bin/python")
    guard = run / "guard"
    guard.mkdir(exist_ok=True)
    (guard / "sitecustomize.py").write_bytes((ROOT / "tests/phase0/network_guard.py").read_bytes())
    env = {
        "PATH": os.environ["PATH"], "HOME": str(run), "ENVIRONMENT": "test",
        "RELEASE_SHA": cmd("git", "rev-parse", "HEAD", capture_output=True).stdout.strip(),
        "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars",
        "JWT_ISSUER": "verdaxis-scratch-smoke-api",
        "JWT_AUDIENCE": "verdaxis-scratch-smoke-web",
        "KYC_EXECUTION_GATE_ENABLED": "false", "DATABASE_URL": owner_url,
        "MIGRATOR_DATABASE_URL": owner_url, "PHASE0_PORT": port,
        "PYTHONPATH": f"{guard}:{ROOT}", "DISPOSABLE_TEST_TOKEN": uuid.uuid4().hex,
        "DISPOSABLE_ITEST_PASSWORD": ITEST_PASSWORD,
    }
    cmd(python, "-m", "alembic", "-c", str(ROOT / "alembic.ini"), "upgrade", "head", env=env, cwd=ROOT)
    # The fixture seed needs the administrator role: the REAL-provenance marker
    # requires the replication-role switch, exactly as the disposable helper does.
    seed_env = dict(env, DATABASE_URL=owner_url.replace("phase0_owner:", "postgres:"))
    cmd(python, str(ROOT / "tests/phase0/scratch_smoke_seed.py"), env=seed_env, cwd=run)
    policy = "/tmp/scratch-smoke-policy"
    cmd("docker", "exec", name, "mkdir", "-p", policy)
    for filename in ("bootstrap_roles.sql", "validate_roles.sql", "app_acl_policy.sql",
                     "converge_runtime_object_acls.sql"):
        cmd("docker", "cp", str(ROOT / "deploy/postgres" / filename), f"{name}:{policy}/{filename}")
    variables = ["-v", f"database_name={DATABASE}", "-v", "app_role=phase0_app_test",
                 "-v", "migrator_role=phase0_owner", "-v", "backup_role=phase0_backup_test"]
    for filename in ("bootstrap_roles.sql", "validate_roles.sql"):
        cmd("docker", "exec", name, "psql", "-X", "-U", "postgres", "-d", DATABASE,
            *variables, "-f", f"{policy}/{filename}")
    cmd("docker", "exec", name, "psql", "-U", "postgres", "-c",
        f"ALTER ROLE phase0_app_test PASSWORD '{PASSWORD}'")

    # Start the backend exactly as the Task 9 harness does, on an ephemeral port.
    app_env = dict(env, DATABASE_URL=owner_url.replace("phase0_owner:", "phase0_app_test:"))
    http_port = pick_ephemeral_port()
    log = (run / "server.log").open("w")
    server = subprocess.Popen([python, "-m", "tests.disposable_server", "--port", str(http_port)],
                              env=app_env, cwd=run, stdout=log, stderr=log)
    base_url = f"http://127.0.0.1:{http_port}"
    wait_ready(base_url, server)

    # Copied orders for the smoke's public-read assertion, oldest ones hidden.
    create_copied_order_fixture(base_url)

    fixture = run / "scratch-fixture.env"
    fixture.write_text(
        f"DATABASE_URL=postgresql+asyncpg://postgres:{PASSWORD}@127.0.0.1:{port}/{DATABASE}\n")
    os.chmod(fixture, 0o600)
    report = run / f"smoke-report-{uuid.uuid4().hex[:8]}.json"
    if report.exists():
        report.unlink()
    # The external client needs no database guard: it talks HTTP like a browser.
    client_env = dict(os.environ, PYTHONPATH=str(ROOT))
    client_env.pop("PHASE0_PORT", None)
    return subprocess.run(
        [python, "-m", "scripts.phase0.scratch_business_smoke", "--base-url", base_url,
         "--fixture-environment-file", str(fixture), "--report", str(report)],
        env=client_env, cwd=ROOT).returncode


def pick_ephemeral_port() -> int:
    """The disposable server requires the kernel IANA range (49152+)."""
    import socket
    for _ in range(50):
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        if port >= 49152:
            return port
    raise RuntimeError("no ephemeral port in the required range")


def wait_ready(base_url, process) -> None:
    last = "no attempt"
    for _ in range(120):
        try:
            response = httpx.get(base_url + "/health/ready", timeout=2)
            last = f"{response.status_code} {response.text[:120]}"
            if response.status_code == 200:
                return
        except Exception as error:
            last = f"{type(error).__name__}: {error}"
        if process.poll() is not None:
            raise RuntimeError(f"server exited; last={last}")
        time.sleep(0.5)
    raise RuntimeError(f"server not ready; last={last}")


def login(client, email: str) -> dict:
    response = client.post("/api/auth/login",
                           data={"username": email, "password": ITEST_PASSWORD})
    assert response.status_code == 200, response.text
    return {"Authorization": f"Bearer {response.json()['access_token']}"}


def create_order(client, headers: dict, payload: dict) -> str:
    response = client.post("/api/orderbook", headers=headers, json=payload)
    assert response.status_code == 201, response.text
    return str(response.json()["id"])


def hide_as_filled(*order_ids: str) -> None:
    """Hide the oldest fixture orders with the lifecycle-legal FILLED shape."""
    joined = "', '".join(order_ids)
    cmd("docker", "exec", name, "psql", "-U", "postgres", "-d", DATABASE,
        "-v", "ON_ERROR_STOP=1", "-c",
        "UPDATE orderbook_orders SET status = 'FILLED', remaining_quantity_mt = 0 "
        f"WHERE id IN ('{joined}')")


def create_copied_order_fixture(base_url: str) -> None:
    """Leave copied orders behind for the smoke's public-read assertion.

    Real copied data has FILLED orders among the oldest rows, and the public
    rules hide them. The fixture reproduces that: the oldest ASK and the oldest
    BID are marked FILLED with zero remaining quantity (the lifecycle check
    requires it), while newer BIDs and ASKs stay visible. A sampling rule that
    ignores visibility fails here; the public-rule sampling still finds three.
    """
    from app.market_catalog import (
        B30_SPECIFICATION_STANDARD,
        CANONICAL_DELIVERY_POINT_IDS,
        CANONICAL_PRODUCTS,
    )
    product = next(item for item in CANONICAL_PRODUCTS if item.market_product.value == "B30")
    lane = {"product_id": str(product.id),
            "delivery_point_id": str(CANONICAL_DELIVERY_POINT_IDS[0]),
            "availability_window": "Spot"}
    with httpx.Client(base_url=base_url, timeout=20) as client:
        ask_headers = login(client, "runner-supplier@example.invalid")
        bid_headers = login(client, "runner-buyer@example.invalid")
        asks = [
            create_order(client, ask_headers, {
                **lane, "side": "ASK", "quantity_mt": quantity,
                "price_per_mt_usd": price, "certification_declared": True,
                "certification_scheme": "ISCC EU",
                "specification_standard": B30_SPECIFICATION_STANDARD, "msds_available": True,
                "carbon_intensity_gco2_mj": "20.5",
                "carbon_intensity_method": "ISCC EU default pathway",
                "feedstock": "Waste-based", "origin": "Singapore"})
            for quantity, price in ((300, 1300), (310, 1310), (320, 1320))
        ]
        bids = [
            create_order(client, bid_headers, {
                **lane, "side": "BID", "quantity_mt": quantity, "price_per_mt_usd": price})
            for quantity, price in ((200, 1100), (210, 1110))
        ]
    # The oldest order on each side is filled, exactly as the real copy is.
    hide_as_filled(asks[0], bids[0])


if __name__ == "__main__":
    try:
        code = main()
    except Exception as error:
        print(f"runner failed: {type(error).__name__}: {error}")
        code = 1
    finally:
        if server:
            server.terminate()
            try:
                server.wait(timeout=20)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
        if log:
            log.close()
        subprocess.run(["docker", "rm", "-f", "-v", name], stdout=subprocess.DEVNULL)
    sys.exit(code)
