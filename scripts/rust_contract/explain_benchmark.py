"""Capture the key-first benchmark statement's actual plan (disposable only).

Enables auto_explain for the app role, drives one list request through the
Rust contract server, and prints every plan the database recorded — the
benchmark statement is the one whose text starts with
'WITH requested_keys AS MATERIALIZED'. Diagnostic evidence only; no gate.
"""
import json
from unittest.mock import patch

import httpx

from scripts.rust_contract import run_disposable as runner


class ExplainBenchmark(runner.Runner):
    def __init__(self):
        super().__init__(
            "all", seed=True, keep=False, seed_mode="corpus", mixed_caddy=True
        )

    def start_database(self):
        original = runner.docker

        def instrumented(*args, **kwargs):
            if args[0] == "run":
                args = (*args, "-c", "logging_collector=on",
                        "-c", "log_directory=/tmp/plans",
                        "-c", "log_filename=queries.log")
            return original(*args, **kwargs)

        with patch.object(runner, "docker", instrumented):
            port = super().start_database()
        return port

    def sql(self, statement):
        return runner.docker(
            "exec", self.container, "psql", "-XAt", "-U", "postgres",
            "-d", runner.DB_NAME, "-v", "ON_ERROR_STOP=1", "-c", statement,
        )

    def run_mixed(self, env, python_port, rust_port):
        self.sql("ANALYZE")
        self.sql("""ALTER ROLE rust_owner SET session_preload_libraries='auto_explain';
            ALTER ROLE rust_owner SET auto_explain.log_min_duration=100;
            ALTER ROLE rust_owner SET auto_explain.log_analyze=on;
            ALTER ROLE rust_owner SET auto_explain.log_buffers=on;
            ALTER ROLE rust_owner SET auto_explain.log_timing=on;
            ALTER ROLE rust_owner SET auto_explain.log_format='json';""")
        self.sql("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename='rust_owner' AND pid<>pg_backend_pid()")
        with httpx.Client(timeout=65, trust_env=False) as client:
            response = client.get(f"http://127.0.0.1:{rust_port}/api/orderbook")
            response.raise_for_status()
        logs = runner.docker("exec", self.container, "cat", "/tmp/plans/queries.log")
        for section in logs.split("plan:\n")[1:]:
            plan, _ = json.JSONDecoder().raw_decode(section.lstrip())
            query = plan.get("Query Text", "")
            print("PLAN_QUERY " + query[:160].replace("\n", " "), flush=True)
            print("PLAN_JSON " + json.dumps(plan), flush=True)
        self.sql("""ALTER ROLE rust_owner RESET session_preload_libraries;
            ALTER ROLE rust_owner RESET auto_explain.log_min_duration;
            ALTER ROLE rust_owner RESET auto_explain.log_analyze;
            ALTER ROLE rust_owner RESET auto_explain.log_buffers;
            ALTER ROLE rust_owner RESET auto_explain.log_timing;
            ALTER ROLE rust_owner RESET auto_explain.log_format;""")


def main():
    explain = ExplainBenchmark()
    try:
        explain.run()
    finally:
        explain.cleanup()


if __name__ == "__main__":
    main()
