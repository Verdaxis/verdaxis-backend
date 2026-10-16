"""Run `cargo sqlx prepare --check` against one disposable migrated schema.

The plan requires this gate on the assembled tree. This project uses runtime
`sqlx::query(...)` calls only, so the check verifies that no compile-time query
metadata is missing. It starts its own disposable container, migrates it, and
removes it on exit. Never point this at staging or production.
"""

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from scripts.rust_contract.run_disposable import DB_NAME, Runner


class Check(Runner):
    def run_mixed(self, env, python_port, rust_port):
        extra = {**env, "DATABASE_URL": env["RUST_DATABASE_URL"]}
        # `cargo sqlx` forwards unknown flags to its own parser, so change
        # directory into the workspace root instead of passing --manifest-path.
        result = subprocess.run(
            ["cargo", "sqlx", "prepare", "--check", "--workspace"],
            cwd=ROOT / "rs",
            env=extra,
            capture_output=True,
            text=True,
        )
        print(f"PREPARE exit={result.returncode}", flush=True)
        output = (result.stdout + result.stderr).strip()
        print(output[-1500:], flush=True)
        if result.returncode != 0:
            raise SystemExit("offline query metadata check failed")


def main():
    # mixed_caddy routes this through run_mixed below; no Caddy is started.
    runner = Check("health", seed=False, keep=False, seed_mode="corpus", mixed_caddy=True)
    print(f"database={DB_NAME}", flush=True)
    try:
        runner.run()
    finally:
        runner.cleanup()


if __name__ == "__main__":
    main()
