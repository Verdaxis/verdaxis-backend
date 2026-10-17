"""Task 14c diagnostic: per-route shape trials on the large fixture.

For each blocking route, alternating Python/Rust warm samples with per-request
wall time, `pg_stat_statements` statement time, and the actual
`EXPLAIN (ANALYZE, BUFFERS)` plan each stack sends (auto_explain JSON, which
carries the bind parameters). `--label` keeps each trial's evidence separate so
a SQL edit can be measured as its own run: edit the shared SQL, rebuild, rerun.

It changes no route code and writes its evidence under `work/rust-phase1/`.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import time
from unittest.mock import patch

import httpx

from scripts.rust_contract import run_disposable as runner

ROTTERDAM = "1379d36c-1ca9-55b7-9c0d-5235a0ba1f36"

PATHS = {
    "R06": "/api/trade-tape",
    "R09": "/api/curves/forward/table",
    "R10": f"/api/curves/forward/slice?market_product=BIO_LNG&delivery_point_id={ROTTERDAM}&availability_window=SPOT",
    "R11": "/api/curves/forward/board",
    "R13": "/api/prices/reference",
    "R14": "/api/prices/reference/export",
    "R19": "/api/orderbook/aggregated",
    "R20": "/api/orderbook/product-counts?side=BID",
    "R21": "/api/orderbook/map-summary",
    "R24": "/api/orderbook/regions",
    "R25": "/api/orderbook/fuel-types",
    "R27": "/api/listings?limit=100",
}


class Diagnose(runner.Runner):
    def __init__(self, label: str, size: str, samples: int, only: list[str]):
        super().__init__("all", seed=True, keep=False, seed_mode="corpus", mixed_caddy=True)
        self.label = label
        self.size = size
        self.samples = samples
        self.only = only

    def start_database(self):
        original = runner.docker

        def instrumented(*args, **kwargs):
            if args[0] == "run":
                args = (*args, "-c", "logging_collector=on",
                        "-c", "log_directory=/tmp/plans", "-c", "log_filename=queries.log")
            return original(*args, **kwargs)

        with patch.object(runner, "docker", instrumented):
            return super().start_database()

    def sql(self, statement):
        return runner.docker(
            "exec", self.container, "psql", "-XAt", "-U", "postgres", "-d", runner.DB_NAME,
            "-v", "ON_ERROR_STOP=1", "-c", statement,
        ).strip()

    def run(self):
        # The runner's own build already produced the contract server for this
        # tree; the serving binary is rebuilt too so a shared-SQL edit is live.
        subprocess.run(
            self.build_command(), cwd=runner.ROOT,
            env={**os.environ, "RELEASE_SHA": self.sha}, check=True,
        )
        env = self.start_environment("all")
        try:
            subprocess.run(
                [self.python_bin, "-m", "scripts.rust_contract.workload_seed", "--size", self.size],
                cwd=runner.ROOT, env={**env["base_env"], "PYTHONPATH": str(runner.ROOT)}, check=True,
            )
            self.sql("ANALYZE")
            self.sql("""ALTER ROLE rust_owner SET session_preload_libraries='auto_explain';
                ALTER ROLE rust_owner SET auto_explain.log_min_duration=0;
                ALTER ROLE rust_owner SET auto_explain.log_analyze=on;
                ALTER ROLE rust_owner SET auto_explain.log_buffers=on;
                ALTER ROLE rust_owner SET auto_explain.log_timing=on;
                ALTER ROLE rust_owner SET auto_explain.log_format='json';
                ALTER ROLE rust_owner SET auto_explain.log_nested_statements=on;""")
            self.sql("SELECT pg_terminate_backend(pid) FROM pg_stat_activity "
                     "WHERE usename='rust_owner' AND pid<>pg_backend_pid()")
            stats = {}
            for name in self.only:
                stats[name] = self.measure(name, PATHS[name], env["python_url"], env["rust_url"])
            out = runner.ROOT / f"work/rust-phase1/t14c-{self.label}.json"
            out.write_text(json.dumps(stats, indent=2) + "\n")
            for name, cells in stats.items():
                print(f"=== {name}")
                for target, data in cells.items():
                    print(f"  {target}: n={data['n']} p50={data['p50']:.4f} "
                          f"p95={data['p95']:.4f} max={data['max']:.4f} "
                          f"route_sql_ms={data['route_statement_ms']}")
                for target, data in cells.items():
                    for plan in data.get("full") or data["plans"]:
                        if "flat" in plan:
                            print(f"  FULL {target} exec_ms={plan['exec_ms']} "
                                  f"rows={plan['rows']} head={plan['query_head'][:60]}")
                            for node in plan["flat"]:
                                print("    " + "  " * node["depth"]
                                      + f"{node['node']} {node['detail']} "
                                      + f"rows={node['rows']} loops={node['loops']} "
                                      + f"t={node['time']} w={node['width']}")
                        else:
                            print(f"  PLAN {target} exec_ms={plan['exec_ms']} "
                                  f"rows={plan['rows']} node={plan['top_node']} "
                                  f"tree={json.dumps(plan.get('tree'))}")
        finally:
            self.stop_environment()

    def measure(self, name, path, python_url, rust_url):
        targets = [("python", python_url), ("rust", rust_url)]
        cells = {t: {"samples": [], "statements": [], "plans": []} for t, _ in targets}
        with httpx.Client(timeout=180, trust_env=False) as client:
            for target, url in targets:
                response = client.get(url + path)
                response.raise_for_status()
            for index in range(self.samples):
                for target, url in targets[::1 if index % 2 == 0 else -1]:
                    self.reset_stats()
                    with httpx.Client(timeout=180, trust_env=False) as client:
                        started = time.perf_counter()
                        response = client.get(url + path)
                        elapsed = time.perf_counter() - started
                    response.raise_for_status()
                    cell = cells[target]
                    cell["samples"].append(elapsed)
                    cell["statements"].append(self.route_statements())
                    if index == 0:
                        cell["plans"] = self.route_plans()
                        if getattr(self, "full_plan", False):
                            cell["full"] = self.full_plans()
                    self.reset_stats()
        for target, _ in targets:
            cell = cells[target]
            ordered = sorted(cell["samples"])
            import math
            cells[target] = {
                "n": len(ordered),
                "p50": statistics.median(ordered),
                "p95": ordered[math.ceil(0.95 * len(ordered)) - 1],
                "max": ordered[-1],
                "route_statement_ms": statistics.median(
                    row["total_exec_time"]
                    for rows in cell["statements"]
                    for row in rows
                    if row["is_route"]
                )
                if any(row["is_route"] for rows in cell["statements"] for row in rows)
                else None,
                "plans": cell["plans"],
                **({"full": cell["full"]} if "full" in cell else {}),
            }
        return cells

    def reset_stats(self):
        self.sql("SELECT pg_stat_statements_reset()")
        runner.docker("exec", self.container, "truncate", "-s", "0", "/tmp/plans/queries.log",
                      check=False)

    def route_statements(self):
        rows = json.loads(self.sql("""
            SELECT coalesce(json_agg(row_to_json(s)), '[]') FROM (
              SELECT query, calls, rows, total_exec_time
              FROM pg_stat_statements
              WHERE query NOT ILIKE '%pg_stat_statements%'
              ORDER BY total_exec_time DESC) s"""))
        background = ("market_event_outbox", "WITH pending AS", "BEGIN", "COMMIT", "ROLLBACK")
        for row in rows:
            row["is_route"] = not any(marker in row["query"] for marker in background)
        return rows

    def full_plans(self):
        """The complete plan of every route statement, unlimited depth."""
        try:
            logs = runner.docker("exec", self.container, "cat", "/tmp/plans/queries.log")
        except runner.RunnerError:
            return []
        background = ("market_event_outbox", "WITH pending AS")
        found = []
        for section in logs.split("plan:\n")[1:]:
            try:
                plan, _ = json.JSONDecoder().raw_decode(section.lstrip())
            except ValueError:
                continue
            text = plan.get("Query Text", "")
            if any(marker in text for marker in background):
                continue
            rows = []
            _full(plan.get("Plan", {}), rows)
            found.append({
                "exec_ms": plan.get("Plan", {}).get("Actual Total Time"),
                "rows": plan.get("Plan", {}).get("Actual Rows"),
                "query_head": text[:120],
                "flat": rows,
            })
        return found

    def route_plans(self):
        try:
            logs = runner.docker("exec", self.container, "cat", "/tmp/plans/queries.log")
        except runner.RunnerError:
            return []
        background = ("market_event_outbox", "WITH pending AS")
        found = []
        for section in logs.split("plan:\n")[1:]:
            try:
                plan, _ = json.JSONDecoder().raw_decode(section.lstrip())
            except ValueError:
                continue
            text = plan.get("Query Text", "")
            if any(marker in text for marker in background):
                continue
            if getattr(self, "full_plan", False):
                rows = []
                _full(plan.get("Plan", {}), rows)
                found.append({"exec_ms": plan.get("Plan", {}).get("Actual Total Time"),
                              "rows_flat": rows})
                continue
            found.append({
                "exec_ms": plan.get("Plan", {}).get("Actual Total Time"),
                "top_node": plan.get("Plan", {}).get("Node Type"),
                "rows": plan.get("Plan", {}).get("Actual Rows"),
                "query_head": text[:80],
                "tree": _tree(plan.get("Plan", {}), 0, 3),
                "params": plan.get("Query Parameters"),
            })
        return found


def _full(node, out, depth=0):
    """Every node, unlimited depth, with the fields that name the cost."""
    if not isinstance(node, dict):
        return
    out.append({
        "node": node.get("Node Type"),
        "detail": node.get("Relation Name") or node.get("Index Name") or "",
        "rows": node.get("Actual Rows"),
        "loops": node.get("Actual Loops"),
        "time": node.get("Actual Total Time"),
        "width": node.get("Plan Width"),
        "filter": (node.get("Filter") or "")[:120],
        "depth": depth,
    })
    for child in node.get("Plans", []):
        _full(child, out, depth + 1)


def _tree(node, depth, limit):
    if depth > limit or not isinstance(node, dict):
        return None
    children = [_tree(child, depth + 1, limit) for child in node.get("Plans", [])]
    return {
        "node": node.get("Node Type"),
        "rows": node.get("Actual Rows"),
        "time": node.get("Actual Total Time"),
        "detail": (node.get("Relation Name") or node.get("Index Name") or ""),
        "children": [c for c in children if c],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--label", required=True)
    parser.add_argument("--size", default="large")
    parser.add_argument("--samples", type=int, default=6)
    parser.add_argument("--only", default="", help="comma-separated route ids")
    parser.add_argument("--full-plan", action="store_true", help="dump complete plans")
    args = parser.parse_args()
    only = [name for name in args.only.split(",") if name] or list(PATHS)
    unknown = [name for name in only if name not in PATHS]
    if unknown:
        raise SystemExit(f"unknown route(s): {unknown}")
    d = Diagnose(args.label, args.size, args.samples, only)
    d.full_plan = args.full_plan
    try:
        d.run()
    finally:
        d.cleanup()


if __name__ == "__main__":
    main()
