"""Capture actual statement plans inside the disposable container only."""
import json
from unittest.mock import patch

import httpx

from scripts.rust_contract import run_disposable as runner


class Explain(runner.Runner):
    def start_database(self):
        original = runner.docker

        def instrumented(*args, **kwargs):
            if args[0] == "run":
                args = (*args, "-c", "logging_collector=on", "-c", "log_directory=/tmp/plans",
                        "-c", "log_filename=queries.log")
            return original(*args, **kwargs)

        with patch.object(runner, "docker", instrumented):
            port = super().start_database()
        return port

    def run_mixed(self, env, python_port, rust_port):
        def sql(statement):
            return runner.docker("exec", self.container, "psql", "-XAt", "-U", "postgres",
                                 "-d", runner.DB_NAME, "-v", "ON_ERROR_STOP=1", "-c", statement)

        sql("ANALYZE")
        sql("""ALTER ROLE rust_owner SET session_preload_libraries='auto_explain';
            ALTER ROLE rust_owner SET auto_explain.log_min_duration=100;
            ALTER ROLE rust_owner SET auto_explain.log_analyze=on;
            ALTER ROLE rust_owner SET auto_explain.log_buffers=on;
            ALTER ROLE rust_owner SET auto_explain.log_timing=on;
            ALTER ROLE rust_owner SET auto_explain.log_format='json';""")
        # Replace only idle connections in this owned disposable database.
        sql("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename='rust_owner' AND pid<>pg_backend_pid()")
        with httpx.Client(timeout=65, trust_env=False) as client:
            consumed = 0
            for path in ("/api/orderbook", "/api/orderbook/bids"):
                for name, port in (("python", python_port), ("rust", rust_port)):
                    response = client.get(f"http://127.0.0.1:{port}{path}")
                    response.raise_for_status()
                    print("PLAN_REQUEST", name, path, response.status_code, flush=True)
                    logs = runner.docker("exec", self.container, "cat", "/tmp/plans/queries.log")
                    sections = logs.split("plan:\n")[1:]
                    for section in sections[consumed:]:
                        plan, _ = json.JSONDecoder().raw_decode(section.lstrip())
                        print("ACTUAL_PLAN " + json.dumps(dict(target=name, path=path, plan=plan)), flush=True)
                    consumed = len(sections)


def main():
    experiment = Explain("all", seed=True, keep=False, seed_mode="corpus", mixed_caddy=True)
    try:
        experiment.run()
    finally:
        experiment.cleanup()


if __name__ == "__main__":
    main()
