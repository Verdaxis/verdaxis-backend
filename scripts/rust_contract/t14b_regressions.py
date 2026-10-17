"""Task 14b diagnostic: name the R20/R24/R27 p95 regressions.

One disposable environment at the small workload, then for each of R20
(`/api/orderbook/product-counts`), R24 (`/api/orderbook/regions`) and R27
(`/api/listings`): alternating Python/Rust warm samples with per-request wall
time, `pg_stat_statements` statement count and total time per request, and the
actual `EXPLAIN (ANALYZE, BUFFERS)` plan of each statement each stack sends.

A kept diagnostic in the family of `explain_lists.py` and `profile_lists.py`:
it changes no route code and writes its evidence under `work/rust-phase1/`.
"""

from __future__ import annotations

import json
import os
import statistics
import subprocess
import time
from pathlib import Path
from unittest.mock import patch

import httpx

from scripts.rust_contract import run_disposable as runner

PATHS = {
    "R20": "/api/orderbook/product-counts?side=BID",
    "R24": "/api/orderbook/regions",
    "R27": "/api/listings?limit=100",
}


class Diagnose(runner.Runner):
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
        subprocess.run(
            self.build_command(), cwd=runner.ROOT,
            env={**os.environ, "RELEASE_SHA": self.sha}, check=True,
        )
        env = self.start_environment("all")
        try:
            subprocess.run(
                [self.python_bin, "-m", "scripts.rust_contract.workload_seed", "--size", "small"],
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
            for name, path in PATHS.items():
                stats[name] = self.measure(name, path, env["python_url"], env["rust_url"])
            out = runner.ROOT / "work/rust-phase1/t14b-r20-r24-r27.json"
            out.write_text(json.dumps(stats, indent=2) + "\n")
            for name, cells in stats.items():
                print(f"=== {name}")
                for target, data in cells.items():
                    print(f"  {target}: n={data['n']} p50={data['p50']:.4f} "
                          f"p95={data['p95']:.4f} max={data['max']:.4f} "
                          f"statements/req={data['statements_per_request']:.2f} "
                          f"sql_ms/req={data['sql_ms_per_request']:.2f} "
                          f"route_sql_ms={data['route_statement_ms']}")
                for target, data in cells.items():
                    for plan in data["plans"]:
                        print(f"  PLAN {target} exec_ms={plan['exec_ms']} "
                              f"rows={plan['rows']} node={plan['top_node']} "
                              f"tree={json.dumps(plan.get('tree'))}")
        finally:
            self.stop_environment()

    def measure(self, name, path, python_url, rust_url):
        targets = [("python", python_url), ("rust", rust_url)]
        cells = {target: {"samples": [], "statements": [], "plans": []} for target, _ in targets}
        with httpx.Client(timeout=65, trust_env=False) as client:
            for target, url in targets:
                client.get(url + path).raise_for_status()
            for index in range(30):
                for target, url in targets[::1 if index % 2 == 0 else -1]:
                    self.reset_stats()
                    with httpx.Client(timeout=65, trust_env=False) as client:
                        started = time.perf_counter()
                        response = client.get(url + path)
                        elapsed = time.perf_counter() - started
                    response.raise_for_status()
                    cell = cells[target]
                    cell["samples"].append(elapsed)
                    cell["statements"].append(self.route_statements(name))
                    if index == 0:
                        cell["plans"] = self.route_plans(name)
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
                "statements_per_request": statistics.median(
                    sum(row["calls"] for row in rows) for rows in cell["statements"]
                ),
                "sql_ms_per_request": statistics.median(
                    sum(row["total_exec_time"] for row in rows) for rows in cell["statements"]
                ),
                "route_statement_ms": statistics.median(
                    row["total_exec_time"]
                    for rows in cell["statements"]
                    for row in rows
                    if row["is_route"]
                )
                if any(row["is_route"] for rows in cell["statements"] for row in rows)
                else None,
                "plans": cell["plans"],
                "statement_texts": sorted(
                    {row["query"] for rows in cell["statements"] for row in rows}
                ),
            }
        return cells

    def reset_stats(self):
        self.sql("SELECT pg_stat_statements_reset()")
        runner.docker("exec", self.container, "truncate", "-s", "0", "/tmp/plans/queries.log",
                      check=False)

    def route_statements(self, name):
        """Every non-background statement from the last request, tagged."""
        rows = json.loads(self.sql("""
            SELECT coalesce(json_agg(row_to_json(s)), '[]') FROM (
              SELECT query, calls, rows, total_exec_time, shared_blks_hit, shared_blks_read
              FROM pg_stat_statements
              WHERE query NOT ILIKE '%pg_stat_statements%'
              ORDER BY total_exec_time DESC) s"""))
        background = ("market_event_outbox", "WITH pending AS", "BEGIN", "COMMIT", "ROLLBACK")
        for row in rows:
            row["is_route"] = not any(marker in row["query"] for marker in background)
        return rows

    def route_plans(self, name):
        """The plan of the route statement the last request executed."""
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
            found.append(
                {
                    "exec_ms": plan.get("Plan", {}).get("Actual Total Time"),
                    "top_node": plan.get("Plan", {}).get("Node Type"),
                    "rows": plan.get("Plan", {}).get("Actual Rows"),
                    "query_head": text[:80],
                    "tree": _tree(plan.get("Plan", {}), depth=0, limit=3),
                }
            )
        return found


def _tree(node, depth, limit):
    """A compact plan tree: node type, actual rows, total time, children."""
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
    d = Diagnose("all", seed=True, keep=False, seed_mode="corpus", mixed_caddy=True)
    try:
        d.run()
    finally:
        d.cleanup()


if __name__ == "__main__":
    main()
