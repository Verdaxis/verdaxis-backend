"""Run Rust notification tests on a local disposable database."""

import os
import subprocess

from disposable_postgres import APP_ROLE, DisposablePostgres, ROOT


def main():
    with DisposablePostgres("verdaxis_notifications_test") as database:
        result = subprocess.run(
            ["cargo", "test", "--manifest-path", str(ROOT / "rs/Cargo.toml"), "--locked",
             "-p", "verdaxis-read", "--test", "group_crud", "crud_notifications::", "--", "--nocapture"],
            cwd=ROOT,
            env={**os.environ, "SQLX_OFFLINE": "true", "RELEASE_SHA": database.release_sha,
                 "REQUIRE_NOTIFICATIONS_TEST": "true",
                 "NOTIFICATIONS_TEST_APP_URL": database.url(APP_ROLE),
                 "NOTIFICATIONS_TEST_ADMIN_URL": database.url("postgres", "disposable_admin_pw")},
        )
        print(f"FOCUSED_RUST_EXIT={result.returncode}", flush=True)
        return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
