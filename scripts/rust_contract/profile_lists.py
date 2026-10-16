"""One-request latency breakdown for the list and bids routes.

For one list request and one bids request, per runtime (Python, Rust):
wall time, per-statement pg_stat_statements rows (calls, execution time),
and the Rust request log's own duration_ms. Sequential requests, one
ANALYZEd disposable corpus database, no other load. This is lane-13
diagnostic evidence, not an acceptance gate.
"""
import json
import time

import httpx

from scripts.rust_contract import run_disposable as runner


class Profile(runner.Runner):
    def __init__(self):
        super().__init__("all", seed=True, keep=False, seed_mode="corpus", mixed_caddy=True)

    def sql(self, statement):
        return runner.docker(
            "exec", self.container, "psql", "-XAt", "-U", "postgres",
            "-d", runner.DB_NAME, "-v", "ON_ERROR_STOP=1", "-c", statement,
        ).strip()

    def statements(self):
        return json.loads(self.sql("""
            SELECT coalesce(json_agg(row_to_json(s)), '[]') FROM (
              SELECT queryid::text, left(query, 120) AS query, calls, rows,
                     round(total_exec_time::numeric, 1) AS total_exec_time,
                     round(total_plan_time::numeric, 1) AS total_plan_time
              FROM pg_stat_statements
              WHERE userid=(SELECT oid FROM pg_roles WHERE rolname='rust_owner')
                AND dbid=(SELECT oid FROM pg_database WHERE datname=current_database())
                -- The Python oracle's background outbox poll is not part of the
                -- request under measurement; exclude it by its table name.
                AND query NOT LIKE '%market_event_outbox%'
              ORDER BY total_exec_time DESC
            ) s
        """))

    def run_mixed(self, env, python_port, rust_port):
        self.sql("ANALYZE")
        targets = [("python", python_port), ("rust", rust_port)]
        report = []
        for path in ("/api/orderbook", "/api/orderbook/bids"):
            for name, port in targets:
                # One warmup so process/pool/statement caches match the p95
                # measurement's warm cell; then one measured request.
                with httpx.Client(timeout=65, trust_env=False) as client:
                    client.get(f"http://127.0.0.1:{port}{path}").raise_for_status()
                    self.sql("SELECT pg_stat_statements_reset()")
                    started = time.perf_counter()
                    response = client.get(f"http://127.0.0.1:{port}{path}")
                    wall = time.perf_counter() - started
                response.raise_for_status()
                stats = self.statements()
                sql_total = sum(row["total_exec_time"] for row in stats) / 1000.0
                report.append(dict(
                    path=path, target=name, wall_s=round(wall, 3),
                    status=response.status_code, bytes=len(response.content),
                    sql_total_s=round(sql_total, 3),
                    outside_sql_s=round(wall - sql_total, 3),
                    statements=stats,
                ))
                summary = {k: v for k, v in report[-1].items() if k != "statements"}
                print("PROFILE " + json.dumps(summary), flush=True)
        out = runner.ROOT / "work/rust-phase1/task13-lane13-profile.json"
        out.write_text(json.dumps(report, indent=2) + "\n")
        print("PROFILE_JSON_PATH " + str(out), flush=True)


def main():
    profile = Profile()
    try:
        profile.run()
    finally:
        profile.cleanup()


if __name__ == "__main__":
    main()
