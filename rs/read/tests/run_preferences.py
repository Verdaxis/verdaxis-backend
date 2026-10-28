"""Run the preferences PostgreSQL route tests on the existing disposable lifecycle.

This covers namespaces that only Rust owns (`dashboard`). The Python-twin
differential for the shared namespaces stays with the phase 2 runner.
"""
import os
import subprocess
import sys

from scripts.rust_contract.phase2_seed import (
    APP_ROLE,
    BACKUP_ROLE,
    MIGRATOR_ROLE,
    PASSWORD,
    connect,
    identifier,
)
from scripts.rust_contract.run_disposable import DB_NAME, ROOT, Runner, docker


def main():
    runner = Runner("all", seed=False, keep=False, seed_mode="ordinary")
    try:
        port = runner.start_database()
        policy_dir = "/tmp/preferences-role-policy"
        docker("exec", runner.container, "mkdir", "-p", policy_dir)
        for name in ("app_acl_policy.sql", "converge_runtime_object_acls.sql", "bootstrap_roles.sql", "validate_roles.sql"):
            docker("cp", str(ROOT / "deploy/postgres" / name), f"{runner.container}:{policy_dir}/{name}")

        def apply_role_policy(script):
            docker("exec", runner.container, "psql", "-X", "-U", "postgres", "-d", DB_NAME,
                   "-v", f"database_name={DB_NAME}", "-v", f"app_role={APP_ROLE}",
                   "-v", f"migrator_role={MIGRATOR_ROLE}", "-v", f"backup_role={BACKUP_ROLE}",
                   "-f", f"{policy_dir}/{script}")

        # As in production, the roles exist and the migrator owns the schema
        # before the migration runs, so SECURITY DEFINER functions have the
        # migrator as owner.
        apply_role_policy("bootstrap_roles.sql")
        docker("exec", runner.container, "psql", "-X", "-U", "postgres", "-d", DB_NAME,
               "-c", f"ALTER ROLE {identifier(MIGRATOR_ROLE)} PASSWORD '{PASSWORD}'")
        migrator_url = f"postgresql+asyncpg://{MIGRATOR_ROLE}:{PASSWORD}@127.0.0.1:{port}/{DB_NAME}"
        environment = {
            **os.environ,
            **runner.urls(port),
            "ENVIRONMENT": "test",
            "RELEASE_SHA": runner.sha,
            "RUST_TEST_PYTHON": sys.executable,
            "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars",
            "DATABASE_URL": migrator_url,
            "MIGRATOR_DATABASE_URL": migrator_url,
        }
        runner.migrate_and_seed(environment)
        for script in ("bootstrap_roles.sql", "validate_roles.sql"):
            apply_role_policy(script)
        with connect(port, DB_NAME) as admin:
            admin.execute(f"ALTER ROLE {identifier(APP_ROLE)} PASSWORD '{PASSWORD}'")
        result = subprocess.run(
            ["cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read",
             "--test", "crud_preferences_postgres", "--", "--nocapture"],
            cwd=ROOT,
            env={**environment, "SQLX_OFFLINE": "true", "REQUIRE_PREFERENCES_TEST": "true",
                 "PREFERENCES_TEST_APP_URL": f"postgresql://{APP_ROLE}:{PASSWORD}@127.0.0.1:{port}/{DB_NAME}",
                 "PREFERENCES_TEST_ADMIN_URL": f"postgresql://postgres:disposable_admin_pw@127.0.0.1:{port}/{DB_NAME}"},
        )
        print(f"FOCUSED_RUST_EXIT={result.returncode}", flush=True)
        return result.returncode
    finally:
        runner.cleanup()


if __name__ == "__main__":
    raise SystemExit(main())
