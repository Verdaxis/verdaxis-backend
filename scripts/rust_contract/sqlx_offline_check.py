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
        failures = 0
        for package in ("rs/read", "rs/contract"):
            result = subprocess.run(
                [
                    "cargo",
                    "sqlx",
                    "prepare",
                    "--check",
                    "--manifest-path",
                    f"{package}/Cargo.toml",
                ],
                cwd=ROOT,
                env=extra,
                capture_output=True,
                text=True,
            )
            print(f"PREPARE {package} exit={result.returncode}", flush=True)
            if result.returncode != 0:
                failures += 1
                print((result.stdout + result.stderr)[-1500:], flush=True)
        if failures:
            raise SystemExit(f"{failures} package(s) failed the offline check")


def main():
    runner = Check("health", seed=False, keep=False, seed_mode="corpus")
    print(f"database={DB_NAME}", flush=True)
    try:
        runner.run()
    finally:
        runner.cleanup()


if __name__ == "__main__":
    main()
