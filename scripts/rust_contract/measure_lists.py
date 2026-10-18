"""Same-database list latency experiment, using the one disposable lifecycle.

Cold means a fresh service process and fresh connection/statement caches, not
an OS page-cache flush. Warm means one service process after one warmup request.
The experiment never changes host caches or configuration. The Rust server is
always built and run with cargo's release profile: latency must be measured on
optimized code, while correctness gates keep the dev profile (PM decision).
"""
import json
import math
import statistics
import subprocess
import time

import httpx

from scripts.rust_contract import run_disposable as runner


def require_latency_parity(summary):
    indexed = {(row["path"], row["mode"], row["target"]): row for row in summary}
    for path, mode, target in indexed:
        if target == "rust" and (path, mode, "python") in indexed:
            assert indexed[path, mode, target]["p95"] <= indexed[path, mode, "python"]["p95"], f"Rust slower: {path} {mode}"
    expected = {(path, mode, target)
                for path in ("/api/orderbook", "/api/orderbook/bids")
                for mode in ("cold-process", "warm")
                for target in ("python", "rust")}
    assert set(indexed) == expected and all(row["n"] >= 30 for row in summary), "complete paired measurements required"


class Measurement(runner.Runner):
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

    def spawn(self, args, env):
        self.launches.append((args, env))
        super().spawn(args, env)

    def sql(self, statement):
        return runner.docker(
            "exec", self.container, "psql", "-XAt", "-U", "postgres",
            "-d", runner.DB_NAME, "-v", "ON_ERROR_STOP=1", "-c", statement,
        ).strip()

    def restart(self, index, port):
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

    def run_mixed(self, env, python_port, rust_port):
        self.sql("ANALYZE")
        records = []
        targets = [("python", python_port, 0), ("rust", rust_port, 1)]
        for path in ("/api/orderbook", "/api/orderbook/bids"):
            for mode in ("cold-process", "warm"):
                if mode == "warm":
                    with httpx.Client(timeout=65, trust_env=False) as client:
                        for _, port, _ in targets:
                            client.get(f"http://127.0.0.1:{port}{path}").raise_for_status()
                for sample in range(30):
                    # Alternate which runtime is measured first to reduce order bias.
                    for name, port, index in targets[::1 if sample % 2 == 0 else -1]:
                        if mode == "cold-process":
                            self.restart(index, port)
                        self.sql("SELECT pg_stat_statements_reset()")
                        with httpx.Client(timeout=65, trust_env=False) as client:
                            started = time.perf_counter()
                            response = client.get(f"http://127.0.0.1:{port}{path}")
                            elapsed = time.perf_counter() - started
                        stats = json.loads(self.sql("""
                            SELECT coalesce(json_agg(row_to_json(s)), '[]') FROM (
                              SELECT queryid::text, query, calls, rows,
                                     total_exec_time, total_plan_time,
                                     shared_blks_hit, shared_blks_read,
                                     jit_functions, jit_generation_time,
                                     jit_inlining_time, jit_optimization_time,
                                     jit_emission_time
                              FROM pg_stat_statements
                              WHERE userid=(SELECT oid FROM pg_roles WHERE rolname='rust_owner')
                                AND dbid=(SELECT oid FROM pg_database WHERE datname=current_database())
                              ORDER BY total_exec_time DESC
                            ) s
                        """))
                        record = dict(path=path, mode=mode, target=name, sample=sample,
                                      seconds=elapsed, status=response.status_code,
                                      bytes=len(response.content), statements=stats)
                        records.append(record)
                        print("SAMPLE " + json.dumps({k:v for k,v in record.items() if k != 'statements'}), flush=True)
                        response.raise_for_status()
        summary = []
        for path in ("/api/orderbook", "/api/orderbook/bids"):
            for mode in ("cold-process", "warm"):
                for name, _, _ in targets:
                    rows = [r for r in records if (r['path'],r['mode'],r['target']) == (path,mode,name)]
                    times = sorted(r['seconds'] for r in rows)
                    summary.append(dict(path=path,mode=mode,target=name,n=len(times),
                                        p50=statistics.median(times),p95=times[math.ceil(.95*len(times))-1]))
        output = runner.ROOT / "work/rust-phase1/task13-list-measurement.json"
        output.write_text(json.dumps(dict(summary=summary,records=records), indent=2))
        for row in summary:
            print("SUMMARY " + json.dumps(row), flush=True)
        # Emit full evidence so the approved wrapper's caller retains it locally.
        print("MEASUREMENT_JSON " + output.read_text().replace("\n", ""), flush=True)
        require_latency_parity(summary)


def main():
    experiment = Measurement()
    try:
        experiment.run()
    finally:
        experiment.cleanup()


if __name__ == "__main__":
    main()
