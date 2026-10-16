"""Disposable-only index experiment for the list and bids pages.

Creates one candidate partial index INSIDE the disposable test database
(never a migration file), measures warm p95 for /api/orderbook and
/api/orderbook/bids on Python and Rust with and without it, captures
EXPLAIN (ANALYZE, BUFFERS) plans for the page-walk shape, and reports the
numbers. The index is dropped before cleanup. No acceptance is claimed.
The Rust server runs cargo's release profile so the p95 cells measure
optimized code (PM decision).
"""
import json
import math
import statistics
import subprocess
import time

import httpx

from scripts.rust_contract import run_disposable as runner

INDEX_NAME = "ix_lane13_open_page_experiment"
INDEX_DDL = (
    f"CREATE INDEX {INDEX_NAME} ON orderbook_orders "
    "(side, created_at DESC, id DESC) "
    "WHERE status IN ('OPEN', 'PARTIALLY_FILLED')"
)
PATHS = ("/api/orderbook", "/api/orderbook/bids")


class IndexExperiment(runner.Runner):
    def __init__(self):
        super().__init__(
            "all",
            seed=True,
            keep=False,
            seed_mode="corpus",
            mixed_caddy=True,
            release=True,
        )
        self.launches = []
        self.plans = {}
        self.route_plans: list[dict] = []

    def start_database(self):
        # auto_explain captures the real bound route statements' plans, the
        # same instrumentation the earlier explain_lists evidence used.
        original = runner.docker

        def instrumented(*args, **kwargs):
            if args[0] == "run":
                args = (*args, "-c", "logging_collector=on",
                        "-c", "log_directory=/tmp/plans",
                        "-c", "log_filename=queries.log")
            return original(*args, **kwargs)

        from unittest.mock import patch

        with patch.object(runner, "docker", instrumented):
            port = super().start_database()
        return port

    def spawn(self, args, env):
        self.launches.append((args, env))
        super().spawn(args, env)

    def sql(self, statement):
        return runner.docker(
            "exec", self.container, "psql", "-XAt", "-U", "postgres",
            "-d", runner.DB_NAME, "-v", "ON_ERROR_STOP=1", "-c", statement,
        ).strip()

    def restart(self, index, port):
        """Fresh service process so no cached plan survives the index change."""
        child = self.children[index]
        child.terminate()
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
        args, env = self.launches[index]
        self.children[index] = subprocess.Popen(
            args, cwd=runner.ROOT, env=env, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        runner.wait_for_port(port)
        runner.attest(f"http://127.0.0.1:{port}", self.token)

    def measure(self, label, python_port, rust_port):
        samples = []
        targets = [("python", python_port), ("rust", rust_port)]
        with httpx.Client(timeout=65, trust_env=False) as client:
            for path in PATHS:
                for _, port in targets:
                    client.get(f"http://127.0.0.1:{port}{path}").raise_for_status()
                for sample in range(30):
                    for name, port in targets[::1 if sample % 2 == 0 else -1]:
                        started = time.perf_counter()
                        response = client.get(f"http://127.0.0.1:{port}{path}")
                        elapsed = time.perf_counter() - started
                        response.raise_for_status()
                        samples.append(dict(
                            label=label, target=name, path=path, sample=sample,
                            seconds=elapsed, bytes=len(response.content),
                        ))
        for path in PATHS:
            for name, _ in targets:
                times = sorted(
                    s["seconds"] for s in samples
                    if s["target"] == name and s["path"] == path
                )
                summary = dict(
                    label=label, target=name, path=path, n=len(times),
                    p50=round(statistics.median(times), 3),
                    p95=round(times[math.ceil(0.95 * len(times)) - 1], 3),
                )
                print("INDEX_RESULT " + json.dumps(summary), flush=True)
        return samples

    def capture_plans(self, label):
        for side in ("BID", "ASK"):
            plan = self.sql(f"""
                EXPLAIN (ANALYZE, BUFFERS, COSTS OFF)
                SELECT o.id FROM orderbook_orders o
                WHERE o.status IN ('OPEN','PARTIALLY_FILLED') AND o.side = '{side}'
                ORDER BY o.created_at DESC, o.id DESC
                LIMIT 20 OFFSET 0
            """)
            self.plans[f"{label}:{side}"] = plan
            print(f"INDEX_PLAN {label} {side}:\n{plan}", flush=True)

    def capture_route_plans(self, label, python_port, rust_port):
        """Record the actual bound route statements' plans via auto_explain."""
        self.sql("""ALTER ROLE rust_owner SET session_preload_libraries='auto_explain';
            ALTER ROLE rust_owner SET auto_explain.log_min_duration=100;
            ALTER ROLE rust_owner SET auto_explain.log_analyze=on;
            ALTER ROLE rust_owner SET auto_explain.log_buffers=on;
            ALTER ROLE rust_owner SET auto_explain.log_timing=on;
            ALTER ROLE rust_owner SET auto_explain.log_format='json';""")
        # Only idle connections are replaced in this owned disposable database.
        self.sql("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename='rust_owner' AND pid<>pg_backend_pid()")
        logs = lambda: runner.docker(
            "exec", self.container, "cat", "/tmp/plans/queries.log")
        with httpx.Client(timeout=65, trust_env=False) as client:
            for path in PATHS:
                for name, port in (("python", python_port), ("rust", rust_port)):
                    before = len(logs().split("plan:\n"))
                    client.get(f"http://127.0.0.1:{port}{path}").raise_for_status()
                    sections = logs().split("plan:\n")[1:]
                    for section in sections[before:]:
                        plan, _ = json.JSONDecoder().raw_decode(section.lstrip())
                        self.route_plans.append(dict(
                            label=label, target=name, path=path, plan=plan))
                        print("ROUTE_PLAN " + json.dumps(dict(
                            label=label, target=name, path=path,
                            top_node=plan.get("Plan", {}).get("Node Type"),
                            startup=plan.get("Planning Time"),
                            total=plan.get("Execution Time"))), flush=True)
        self.sql("""ALTER ROLE rust_owner RESET session_preload_libraries;
            ALTER ROLE rust_owner RESET auto_explain.log_min_duration;
            ALTER ROLE rust_owner RESET auto_explain.log_analyze;
            ALTER ROLE rust_owner RESET auto_explain.log_buffers;
            ALTER ROLE rust_owner RESET auto_explain.log_timing;
            ALTER ROLE rust_owner RESET auto_explain.log_format;""")

    def run_mixed(self, env, python_port, rust_port):
        self.sql("ANALYZE")
        records = []
        self.capture_route_plans("without", python_port, rust_port)
        self.capture_plans("without")
        records += self.measure("without", python_port, rust_port)
        self.sql(INDEX_DDL)
        self.sql("ANALYZE orderbook_orders")
        # Fresh service processes so neither runtime keeps a cached plan from
        # the no-index phase.
        self.restart(0, python_port)
        self.restart(1, rust_port)
        self.capture_route_plans("with", python_port, rust_port)
        self.capture_plans("with")
        records += self.measure("with", python_port, rust_port)
        self.sql(f"DROP INDEX IF EXISTS {INDEX_NAME}")
        out = runner.ROOT / "work/rust-phase1/task13-lane13-index.json"
        out.write_text(json.dumps(
            dict(samples=records, plans=self.plans, route_plans=self.route_plans),
            indent=2) + "\n")
        print("INDEX_JSON_PATH " + str(out), flush=True)


def main():
    experiment = IndexExperiment()
    try:
        experiment.run()
    finally:
        experiment.cleanup()


if __name__ == "__main__":
    main()
