"""Capture `EXPLAIN (ANALYZE, BUFFERS)` plans for the Task 9/10 fix items.

Decision 110: this pass chooses and keeps a change only by plan work — actual
rows, loops, shared/temp buffer blocks — on the disposable large database. It
runs no latency, memory or load measurement (`TIMING OFF`, no `EXPLAIN` time is
reported).

The composed statements come from the Rust example `explain_statements`, so
this tool never re-composes SQL. Each statement is prepared once and explained
with the same binds the route binds; `plan_cache_mode=force_generic_plan` models
the steady state after the pool has reused each prepared statement.

Run only on a test host, never on the Mac:

    verdaxis-staging-test "$PWD" '/home/verdaxis-test/bin/heavy run 3600 -- \
        bash -lc "venv/bin/python -m scripts.rust_contract.explain_routes"'
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from datetime import date
from pathlib import Path

from scripts.rust_contract import run_disposable as runner

POLICY_PATH = "deploy/rust/market-policy.json"
CLOCK = "2026-10-15T12:00:00+00:00"
SINGAPORE_DATE = "2026-10-15"
SURFACE = {"curve": "candidates", "orderbook": "orderbook", "listings": "listings"}

INTERESTING = (
    "Seq Scan",
    "Index Scan",
    "Index Only Scan",
    "Subquery Scan",
    "Sort",
    "Incremental Sort",
    "HashAggregate",
    "GroupAggregate",
    "Nested Loop",
    "Hash Join",
    "Merge Join",
    "Bitmap Heap Scan",
    "Bitmap Index Scan",
    "CTE Scan",
    "Gather",
    "Gather Merge",
    "Materialize",
    "WindowAgg",
    "Unique",
    "Result",
    "Memoize",
)


def blocks(node: dict) -> int:
    return sum(
        node.get(name, 0)
        for name in (
            "Shared Hit Blocks",
            "Shared Read Blocks",
            "Local Hit Blocks",
            "Local Read Blocks",
        )
    )


def summarize(plan: dict) -> dict:
    def walk(node: dict, depth: int = 0, out: list | None = None) -> list:
        if out is None:
            out = []
        out.append(
            {
                "node": node.get("Node Type"),
                "relation": node.get("Relation Name") or node.get("Index Name") or "",
                "rows": node.get("Actual Rows"),
                "loops": node.get("Actual Loops"),
                "blocks": blocks(node),
                "depth": depth,
            }
        )
        for child in node.get("Plans", []):
            walk(child, depth + 1, out)
        return out

    root = plan["Plan"]
    return {
        "rows": root.get("Actual Rows"),
        "loops": root.get("Actual Loops"),
        "blocks": blocks(root),
        "temp_read": root.get("Temp Read Blocks", 0),
        "temp_written": root.get("Temp Written Blocks", 0),
        "nodes": walk(root),
    }


def default_windows(day: date, board: bool) -> list[str]:
    """The route's own window axis, mirroring `curve_projection::default_windows`."""
    windows = ["SPOT"]
    if board:
        for offset in range(1, 7):
            absolute = day.month - 1 + offset
            windows.append(f"{day.year + absolute // 12}-{absolute % 12 + 1:02d}")
    else:
        quarter = (day.month - 1) // 3
        for month in range(day.month, (quarter + 1) * 3 + 1):
            windows.append(f"{day.year}-{month:02d}")
    quarter = (day.month - 1) // 3
    for offset in range(1, (5 if board else 21)):
        absolute = day.year * 4 + quarter + offset
        windows.append(f"{absolute // 4}-Q{absolute % 4 + 1}")
    if board:
        windows.extend([f"{day.year + 1}-CAL", f"{day.year + 2}-CAL"])
    return windows


def curve_keys(policy: dict, board: bool) -> list[dict]:
    keys = []
    for product in policy["products"]:
        if product["execution_mode"] != "ORDERBOOK":
            continue
        for point in product["lanes"]:
            for window in default_windows(date.fromisoformat(SINGAPORE_DATE), board):
                keys.append(
                    {
                        "product_id": product["id"],
                        "delivery_point_id": point,
                        "availability_window": window,
                    }
                )
    return keys


def bind_literals(
    policy: dict, profile: str, slice_key: tuple[str, str, str] | None = None
) -> list[str]:
    """The exact bind list each statement expects, in `$n` order.

    `slice_key` is a product id, delivery point id and window that the seeded
    corpus actually fills, so the single-slice statements plan real rows.
    """
    policy_text = json.dumps(policy)
    dollar = f"$policy${policy_text}$policy$"
    if slice_key is not None:
        product, point, window = slice_key
    else:
        first_product = next(
            p for p in policy["products"] if p["execution_mode"] == "ORDERBOOK"
        )
        product = first_product["id"]
        point = first_product["lanes"][0]
        window = "SPOT"
    table_keys = json.dumps(curve_keys(policy, board=False))
    board_keys = curve_keys(policy, board=True) + [
        {
            "product_id": product,
            "delivery_point_id": point,
            "availability_window": None,
        }
    ]
    board_keys = json.dumps(board_keys)
    slice_keys = json.dumps(
        [
            {
                "product_id": product,
                "delivery_point_id": point,
                "availability_window": window,
            }
        ]
    )
    shared = {
        "policy": dollar,
        "now": f"'{CLOCK}'",
        "date": f"'{SINGAPORE_DATE}'",
        "off_spec": "false",
        "product": "NULL",
        "point": "NULL",
        "side": "NULL",
        "fuel": "NULL",
        "market": "NULL",
        "region": "NULL",
        "window": "NULL",
        "surface": f"'{SURFACE['orderbook']}'",
        "gate": "true",
    }

    def order(**overrides) -> list[str]:
        values = {**shared, **overrides}
        return [
            values[name]
            for name in (
                "policy",
                "now",
                "date",
                "off_spec",
                "product",
                "point",
                "side",
                "fuel",
                "market",
                "region",
                "window",
                "surface",
                "gate",
            )
        ]

    curves = dict(surface=f"'{SURFACE['curve']}'")
    if profile == "curve_keys":
        return order(**curves) + [_quote(table_keys)]
    if profile == "slice":
        return order(
            product=f"'{product}'",
            point=f"'{point}'",
            window=f"'{window}'",
            **curves,
        ) + [_quote(slice_keys)]
    if profile == "depth":
        return order(**curves) + [f"'{product}'", f"'{point}'", f"'{window}'"]
    if profile == "board":
        return order(**curves) + [_quote(board_keys)]
    if profile == "unknown_keys":
        # A standalone statement: $1 policy, $2 now, $3 date, $4 keys, $5 gate.
        return [
            dollar,
            f"'{CLOCK}'",
            f"'{SINGAPORE_DATE}'",
            _quote(table_keys),
            "true",
        ]
    if profile == "mcp_resolve":
        return order(side="'ASK'") + [_quote(slice_keys)]
    if profile in {"mcp_slice", "mcp_slice_before"}:
        values = order(
            product=f"'{product}'",
            point=f"'{point}'",
            side="'ASK'",
            window=f"'{window}'",
        )
        return values + ([_quote(slice_keys)] if profile == "mcp_slice" else [])
    if profile == "reference":
        return [dollar, "NULL", "NULL", "NULL", "NULL", "NULL"] + ["NULL"] * 5
    if profile == "aggregate":
        return order() + ["1000"]
    if profile == "none":
        return order()
    if profile == "recent":
        return order(side="'ASK'")
    if profile == "regions":
        return order() + ["'regions'"]
    if profile == "fuel-types":
        return order() + ["'fuel-types'"]
    if profile == "listings":
        return order(surface=f"'{SURFACE['listings']}'") + ["0", "50"]
    raise runner.RunnerError(f"unknown bind profile {profile}")


def _quote(text: str) -> str:
    tag = "$keys$"
    while tag in text:
        tag = "$" + tag[1:]
    return f"{tag}{text}{tag}"


class PlanCapture(runner.Runner):
    def sql(self, script: str) -> str:
        return runner.docker(
            "exec",
            "-i",
            self.container,
            "psql",
            "-qXAt",
            "-U",
            "postgres",
            "-d",
            runner.DB_NAME,
            "-v",
            "ON_ERROR_STOP=1",
            input_text=script,
        )

    def explain(self, name: str, sql: str, binds: list[str]) -> dict:
        script = (
            # Every captured route runs its statement in a transaction with
            # `SET LOCAL jit = off`; this session setting mirrors that.
            "SET jit = off;\n"
            "SET plan_cache_mode = force_generic_plan;\n"
            f"PREPARE plan_stmt AS {sql};\n"
            "\\o /tmp/plan.json\n"
            "EXPLAIN (ANALYZE, BUFFERS, TIMING OFF, FORMAT JSON) EXECUTE plan_stmt("
            + ", ".join(binds)
            + ");\n"
        )
        try:
            self.sql(script)
        except runner.RunnerError as error:
            expected = max(
                (int(match.group(1)) for match in re.finditer(r"\$(\d+)", sql)),
                default=0,
            )
            raise runner.RunnerError(
                f"{name}: prepare/execute failed (sql params={expected}, "
                f"binds={len(binds)}): {error}"
            ) from error
        # A large plan can exceed one `docker exec` stream read; reading the
        # file it wrote keeps every byte.

        with tempfile.TemporaryDirectory(prefix="rust-query-plan-") as directory:
            path = Path(directory) / "plan.json"
            runner.docker("cp", f"{self.container}:/tmp/plan.json", str(path))
            output = path.read_text().strip()
        try:
            return json.loads(output)[0]
        except ValueError as error:
            raise runner.RunnerError(
                f"{name}: EXPLAIN printed no JSON plan; output tail:\n{output[-2000:]}"
            ) from error


def composed_statements(sha: str) -> dict:
    """The exact statement bytes from the Rust example (one composer)."""
    result = subprocess.run(
        [
            "cargo",
            "run",
            "--manifest-path",
            "rs/Cargo.toml",
            "--package",
            "verdaxis-read",
            "--example",
            "explain_statements",
        ],
        cwd=runner.ROOT,
        env={**os.environ, "RELEASE_SHA": sha},
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise runner.RunnerError(
            "explain_statements failed:\n" + result.stderr[-4000:]
        )
    return json.loads(result.stdout)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--items", default="")
    parser.add_argument("--size", default="large")
    args = parser.parse_args(argv)
    wanted = [name.strip() for name in args.items.split(",") if name.strip()]

    capture = PlanCapture("all", seed=True, keep=False, seed_mode="corpus")
    statements = composed_statements(capture.sha)
    with open(os.path.join(runner.ROOT, POLICY_PATH)) as handle:
        policy = json.load(handle)
    try:
        port = capture.start_database()
        env = {
            **os.environ,
            **capture.urls(port),
            "ENVIRONMENT": "test",
            "RELEASE_SHA": capture.sha,
            "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars",
            "RUST_CONTRACT_CLOCK": CLOCK,
            "KYC_EXECUTION_GATE_ENABLED": "true",
            "RUST_CORPUS_MANIFEST": str(
                runner.ROOT / "work/rust-phase1/task4b-fixtures.json"
            ),
            "PYTHONPATH": str(runner.ROOT),
        }
        capture.migrate_and_seed(env)
        subprocess.run(
            [
                capture.python_bin,
                "-m",
                "scripts.rust_contract.workload_seed",
                "--size",
                args.size,
            ],
            cwd=runner.ROOT,
            env=env,
            check=True,
        )
        capture.sql("ANALYZE;")
        raw = capture.sql(
            "SELECT product_id, delivery_point_id, availability_window "
            "FROM orderbook_orders WHERE side = 'ASK' AND provenance = 'REAL' "
            "AND status IN ('OPEN', 'PARTIALLY_FILLED') "
            "GROUP BY 1, 2, 3 ORDER BY count(*) DESC LIMIT 1;"
        ).strip()
        if not raw:
            raise runner.RunnerError("the corpus exposed no ASK slice")
        slice_key = tuple(raw.split("|"))
        for name, entry in statements.items():
            if wanted and not any(
                name == item or name.startswith(item + ".") for item in wanted
            ):
                continue
            binds = bind_literals(policy, entry["profile"], slice_key)
            plan = capture.explain(name, entry["sql"], binds)
            summary = summarize(plan)
            print(
                f"=== {name} profile={entry['profile']} "
                f"rows={summary['rows']} loops={summary['loops']} "
                f"blocks={summary['blocks']} "
                f"temp={summary['temp_read']}/{summary['temp_written']}",
                flush=True,
            )
            for node in summary["nodes"][1:]:
                if node["node"] not in INTERESTING:
                    continue
                print(
                    "    "
                    + "  " * node["depth"]
                    + f"{node['node']} {node['relation']} rows={node['rows']} "
                    + f"loops={node['loops']} blocks={node['blocks']}",
                    flush=True,
                )
    finally:
        capture.cleanup()
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
