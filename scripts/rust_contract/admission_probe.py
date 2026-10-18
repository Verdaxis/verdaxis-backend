"""Decision 88: Rust-only small-workload admission probe on a disposable stack."""
from __future__ import annotations

import json
import os
import subprocess
import threading
import time
from pathlib import Path

from scripts.rust_contract import measure
from scripts.rust_contract import run_disposable as runner

ROOT = runner.ROOT
RATES = (5, 10, 15, 20, 30, 40)


def slice_cpu() -> int:
    """Account for Rust, PostgreSQL, Caddy, and the load generator together."""
    uid = os.getuid()
    path = Path(f"/sys/fs/cgroup/user.slice/user-{uid}.slice/cpu.stat")
    return measure.parse_cpu_usage_usec(path.read_text())


def run() -> None:
    measure._ensure_user_session()
    probe = measure.Measurement(
        "small", output_dir=ROOT / "work/admission-probe", warmup_seconds=15,
        windows=[], repetitions=1,
    )
    evidence = {"source_sha": probe.sha, "profile": "release", "size": "small", "steps": []}
    try:
        subprocess.run(probe.build_command(), cwd=ROOT,
                       env={**os.environ, "RELEASE_SHA": probe.sha}, check=True)
        subprocess.run(["cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked",
                        "--release", "-p", "verdaxis-read"], cwd=ROOT,
                       env={**os.environ, "RELEASE_SHA": probe.sha}, check=True)
        port = probe.start_database()
        env = {
            **os.environ, "ENVIRONMENT": "test", "RELEASE_SHA": probe.sha,
            "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars",
            "DISPOSABLE_TEST_TOKEN": probe.token, "RUST_CONTRACT_CLOCK": runner.CORPUS_CLOCK,
            "KYC_EXECUTION_GATE_ENABLED": "true",
            "RUST_CORPUS_MANIFEST": str(ROOT / "work/rust-phase1/task4b-fixtures.json"),
            **probe.urls(port),
        }
        probe.migrate_and_seed(env)
        probe._seed_workload(env)
        runner.docker("exec", probe.container, "psql", "-XAt", "-U", "postgres",
                      "-d", runner.DB_NAME, "-v", "ON_ERROR_STOP=1", "-c", "ANALYZE")
        settings = [8, 16]
        for slots in settings:
            rust_port = runner.ephemeral_port()
            config = json.dumps({
                "environment": "test", "release_sha": probe.sha,
                "migration_revision": runner.MIGRATION_REVISION,
                "database_url": env["RUST_DATABASE_URL"],
                "db_session_time_zone": runner.ORACLE_TIMEZONE,
                "bind": f"127.0.0.1:{rust_port}", "read_admission_slots": slots,
                "pool_max": slots + 1, "python_budget": 52, "configured_max_connections": 100,
            })
            handle = probe._start_service_unit("rust", 0, rust_port, config, env)
            caddy = None
            steps = []
            try:
                probe._await_ready(rust_port)
                caddy, caddy_port = probe._start_caddy(f"127.0.0.1:{rust_port}")
                probe.load_series(caddy_port, 15, 5)
                for rate in RATES:
                    counters = []
                    stop = threading.Event()
                    scraper = threading.Thread(target=probe._scrape_loop,
                                               args=(handle, stop, counters), daemon=True)
                    scraper.start()
                    cpu = slice_cpu()
                    started = time.monotonic()
                    try:
                        samples = probe.load_series(caddy_port, 60, rate)
                    finally:
                        stop.set()
                        scraper.join(timeout=5)
                    elapsed = time.monotonic() - started
                    successes = [s for s in samples if 200 <= s["status"] < 300]
                    step = {
                        "slots": slots, "rps": rate, "seconds": elapsed,
                        "successes": len(successes),
                        "busy_503": sum(s["status"] == 503 for s in samples),
                        "other_statuses": {str(status): sum(s["status"] == status for s in samples)
                                           for status in sorted({s["status"] for s in samples})
                                           if not 200 <= status < 300 and status != 503},
                        "success_rps": len(successes) / elapsed,
                        "p95_ms": measure.percentile(sorted(s["seconds"] * 1000 for s in successes), .95)
                                  if successes else None,
                        "slice_cpu_cores": (slice_cpu() - cpu) / 1_000_000 / elapsed,
                        "samples": samples, "runtime_counters": counters,
                        "pool_waits": "not instrumented; no pool-wait duration claim",
                    }
                    steps.append(step)
                    evidence["steps"].append(step)
                    print("ADMISSION_STEP " + json.dumps({k: v for k, v in step.items()
                                                          if k not in {"samples", "runtime_counters"}}), flush=True)
                    output = ROOT / "work/admission-probe/results.json"
                    output.parent.mkdir(parents=True, exist_ok=True)
                    output.write_text(json.dumps(evidence, indent=2) + "\n")
            finally:
                if caddy is not None:
                    caddy.terminate()
                    caddy.wait(timeout=10)
                probe.stop_service(handle)
            if slots == 16 and all(not s["busy_503"] for s in steps):
                if steps[-1]["success_rps"] > steps[-2]["success_rps"]:
                    settings.append(24)
    finally:
        for handle in list(probe.services):
            probe.stop_service(handle)
        probe.cleanup()


if __name__ == "__main__":
    run()
