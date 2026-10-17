"""Direct SQL shape probe for the curve-trade statement (throwaway).

Starts the disposable container with the large fixture, then EXPLAIN (ANALYZE)
the exact bound statement through PREPARE/EXECUTE for several candidate shapes,
printing each plan's top nodes. This iterates on shapes without rebuilding Rust.
"""

from __future__ import annotations

import json
import os
import sys

sys.path.insert(0, os.getcwd())

from scripts.rust_contract import run_disposable as runner
from scripts.rust_contract.workload_seed import seed_workload
from scripts.rust_contract import workload as workload_mod


def main() -> int:
    manifest = workload_mod.load()
    counts = workload_mod.sizes(manifest)["large"]
    r = runner.Runner("health", seed=True, keep=False, seed_mode="corpus")
    port = r.start_database()
    try:
        env = {
            **os.environ,
            "ENVIRONMENT": "test",
            "RUST_CONTRACT_CLOCK": runner.CORPUS_CLOCK,
            **r.urls(port),
        }
        from unittest.mock import patch

        with patch.dict(os.environ, env):
            r.migrate_and_seed(env)
        runner.docker(
            "exec", r.container, "psql", "-XAt", "-U", "postgres", "-d", runner.DB_NAME,
            "-v", "ON_ERROR_STOP=1", "-c", "ANALYZE",
        )

        def sql(stmt: str) -> str:
            return runner.docker(
                "exec", r.container, "psql", "-XAt", "-U", "postgres", "-d", runner.DB_NAME,
                "-v", "ON_ERROR_STOP=1", "-c", stmt,
            ).strip()

        import subprocess

        subprocess.run(
            ["venv/bin/python", "-m", "scripts.rust_contract.workload_seed", "--size", "large"],
            cwd=runner.ROOT, env={**env, "PYTHONPATH": str(runner.ROOT)}, check=True,
        )
        policy = open("deploy/rust/market-policy.json").read()
        # The live table route binds the lane-restricted product x point x
        # window grid (888 distinct keys). Rebuild it from the same axes.
        import itertools

        axes = json.loads(Path("rs/read/src/market/orderbook_query.json").read_text()) \
            if False else None
        products = ["B30", "B100", "BIO_METHANOL", "E_METHANOL", "BIO_ETHANOL", "SYNTHETIC_ETHANOL"]
        points = [
            "73835e92-820e-584b-8280-bb61c63aa28e",  # Singapore
            "1379d36c-1ca9-55b7-9c0d-5235a0ba1f36",  # Rotterdam
            "78281fb3-e726-5396-802e-34b01fda21e8",  # Dalian
            "262d36ae-6f35-5785-b9e8-9e221b0f1b78",  # Busan
            "633c0593-f9b9-52ad-abe5-a0285f6888d3",  # Shanghai
            "a083db06-b050-56c2-a274-3897eac2fdae",  # Houston
            "1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534",  # Los Angeles
        ]
        windows = ["SPOT", "2026-10", "2026-11", "2026-12"] + [
            f"{y}-Q{q}" for y in (2026, 2027, 2028, 2029, 2030, 2031) for q in (1, 2, 3, 4)
        ]
        keys = json.dumps([
            {"market_product": prod, "delivery_point_id": point, "availability_window": window}
            for prod in products for point in points for window in windows
        ])
        base = (
            open("rs/read/sql/formal_trade_snapshots.sql").read()
            + open("rs/read/sql/trade_evidence.sql").read()
        )
        tail = open("rs/read/sql/curves/trades.sql").read()
        stmt = f"WITH eligible_trades AS ({base}) {tail}"
        esc = lambda s: s.replace("'", "''")  # noqa: E731

        def one_session(script: str) -> str:
            # PREPARE and EXECUTE must share one psql session.
            return runner.docker(
                "exec", "-i", r.container, "psql", "-XAt", "-U", "postgres",
                "-d", runner.DB_NAME, "-v", "ON_ERROR_STOP=1",
                input_text=script,
            ).strip()

        # Execute five times so PostgreSQL switches to a generic plan, then
        # EXPLAIN the sixth: this is what the pooled server connection sees.
        exec_line = (
            "EXPLAIN (ANALYZE, COSTS OFF) EXECUTE probe("
            f"'{esc(policy)}','2026-09-15 20:00:00+08',NULL,NULL,NULL,NULL,'{esc(keys)}');"
        )
        script = (
            f"PREPARE probe(text,timestamptz,timestamptz,uuid,uuid,text,text) AS {stmt};\n"
            + "EXECUTE probe("
            f"'{esc(policy)}',NULL,NULL,NULL,NULL,NULL,'{esc(keys)}');\n" * 5
            + exec_line + "\n"
            "DEALLOCATE probe;"
        )
        out = one_session(script)
        print("=== CURRENT SHAPE (timing off, so rows only)")
        print("\n".join(out.splitlines()[:20]))

        # Candidate: a single text[] containment over the materialized key
        # set. `= ANY(array)` is a per-row hashed lookup, not a join, so the
        # planner cannot choose a nested loop over it.
        tail2 = tail
        exists_block = (
            "    WHERE EXISTS (\n"
            "        SELECT 1\n"
            "        FROM requested_curve_keys AS k\n"
            "        WHERE k.market_product = t.market_product\n"
            "          AND k.delivery_point_id = t.delivery_point_id\n"
            "          AND k.availability_window = t.availability_window\n"
            "    )"
        )
        any_block = (
            "    WHERE (t.market_product || '\u0001' || t.delivery_point_id::text\n"
            "           || '\u0001' || t.availability_window) = ANY (ARRAY(\n"
            "        SELECT k.market_product || '\u0001' || k.delivery_point_id::text\n"
            "               || '\u0001' || k.availability_window\n"
            "        FROM requested_curve_keys AS k))\n"
        )
        assert exists_block in tail2, "key EXISTS block not found"
        tail2 = tail2.replace(exists_block, any_block)
        stmt2 = f"WITH eligible_trades AS ({base}) {tail2}"
        script2 = (
            f"PREPARE probe2(text,timestamptz,timestamptz,uuid,uuid,text,text) AS {stmt2};\n"
            "EXPLAIN (ANALYZE, COSTS OFF) EXECUTE probe2("
            f"'{esc(policy)}',NULL,NULL,NULL,NULL,NULL,'{esc(keys)}');\n"
            "DEALLOCATE probe2;"
        )
        out2 = one_session(script2)
        print("=== JOIN SHAPE")
        print("\n".join(out2.splitlines()[:20]))
        return 0
    finally:
        r.cleanup()


if __name__ == "__main__":
    raise SystemExit(main())
