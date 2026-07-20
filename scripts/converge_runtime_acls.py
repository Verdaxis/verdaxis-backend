#!/usr/bin/env python3
"""Converge committed runtime object/column ACL policy as the object owner."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import stat
import subprocess
import sys
from typing import Mapping, NamedTuple

from dotenv import dotenv_values
from sqlalchemy.engine import URL, make_url
from sqlalchemy.exc import ArgumentError


_IDENTITIES = {
    "production": {
        "database_name": "verdaxis",
        "app_role": "verdaxis_app",
        "migrator_role": "verdaxis_migrator",
        "backup_role": "verdaxis_backup",
    },
    "staging": {
        "database_name": "verdaxis_staging",
        "app_role": "verdaxis_app_staging",
        "migrator_role": "verdaxis_migrator_staging",
        "backup_role": "verdaxis_backup_staging",
    },
}
_BUNDLE_FILES = (
    "deploy/postgres/app_acl_policy.sql",
    "deploy/postgres/converge_runtime_object_acls.sql",
)


class RuntimeAclConvergenceError(RuntimeError):
    """The post-migration ACL boundary is not safe to execute."""


class RuntimeAclTarget(NamedTuple):
    database_name: str
    app_role: str
    migrator_role: str
    backup_role: str
    migration_url: URL


def _required_url(values: Mapping[str, object], name: str) -> str:
    raw = values.get(name)
    if not isinstance(raw, str) or not raw.strip():
        raise RuntimeAclConvergenceError(
            f"an explicit {name} is required for runtime ACL convergence"
        )
    return raw.strip()


def resolve_acl_target(
    environment: str, values: Mapping[str, object]
) -> RuntimeAclTarget:
    """Bind database URLs and role names to one deployed environment."""
    try:
        identity = _IDENTITIES[environment]
    except KeyError as exc:
        raise RuntimeAclConvergenceError(
            "runtime ACL convergence is limited to production or staging"
        ) from exc

    app_text = _required_url(values, "DATABASE_URL")
    migration_text = _required_url(values, "MIGRATOR_DATABASE_URL")
    try:
        app_url = make_url(app_text)
        migration_url = make_url(migration_text)
    except (ArgumentError, TypeError, ValueError) as exc:
        raise RuntimeAclConvergenceError(
            "runtime ACL database URLs must be valid PostgreSQL URLs"
        ) from exc

    if (
        not app_url.drivername.startswith("postgresql")
        or not migration_url.drivername.startswith("postgresql")
        or app_url.query
        or migration_url.query
    ):
        raise RuntimeAclConvergenceError(
            "runtime ACL database URLs must be PostgreSQL URLs without query routing"
        )
    if (
        app_url.database != identity["database_name"]
        or app_url.username != identity["app_role"]
    ):
        raise RuntimeAclConvergenceError(
            "DATABASE_URL does not match the selected application database identity"
        )
    if (
        migration_url.database != identity["database_name"]
        or migration_url.username != identity["migrator_role"]
    ):
        raise RuntimeAclConvergenceError(
            "MIGRATOR_DATABASE_URL does not match the selected migrator identity"
        )
    app_endpoint = ((app_url.host or "").lower(), app_url.port or 5432)
    migration_endpoint = (
        (migration_url.host or "").lower(),
        migration_url.port or 5432,
    )
    if app_endpoint != migration_endpoint:
        raise RuntimeAclConvergenceError(
            "DATABASE_URL and MIGRATOR_DATABASE_URL must use the same database endpoint"
        )
    if (
        app_text == migration_text
        or app_url.username == migration_url.username
        or not app_url.password
        or not migration_url.password
        or not migration_url.host
    ):
        raise RuntimeAclConvergenceError(
            "explicit app and migrator URLs must use distinct roles and credentials"
        )
    return RuntimeAclTarget(**identity, migration_url=migration_url)


def build_psql_invocation(
    bundle_root: Path, target: RuntimeAclTarget
) -> tuple[list[str], dict[str, str]]:
    script = bundle_root / "deploy/postgres/converge_runtime_object_acls.sql"
    command = [
        "/usr/bin/psql",
        "-X",
        "--single-transaction",
        "--set",
        "ON_ERROR_STOP=1",
    ]
    for name in ("database_name", "app_role", "migrator_role", "backup_role"):
        command.extend(("--set", f"{name}={getattr(target, name)}"))
    command.extend(("--file", str(script)))

    url = target.migration_url
    process_environment = {
        "PATH": "/usr/bin:/bin",
        "PGHOST": url.host or "",
        "PGPORT": str(url.port or 5432),
        "PGDATABASE": target.database_name,
        "PGUSER": target.migrator_role,
        "PGPASSWORD": url.password or "",
    }
    for name in ("LANG", "LC_ALL", "SSL_CERT_FILE", "SSL_CERT_DIR"):
        if value := os.environ.get(name):
            process_environment[name] = value
    return command, process_environment


def _attest_bundle(bundle_root: Path) -> Path:
    root = bundle_root.resolve(strict=True)
    for relative in _BUNDLE_FILES:
        path = root / relative
        try:
            metadata = path.lstat()
        except OSError as exc:
            raise RuntimeAclConvergenceError(
                f"committed ACL bundle is missing {relative}"
            ) from exc
        if not stat.S_ISREG(metadata.st_mode) or path.is_symlink():
            raise RuntimeAclConvergenceError(
                f"committed ACL bundle artifact is not regular: {relative}"
            )
    return root


def _load_database_values(environment_file: Path) -> dict[str, object]:
    descriptor: int | None = None
    try:
        flags = os.O_RDONLY
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        descriptor = os.open(environment_file, flags)
        if not stat.S_ISREG(os.fstat(descriptor).st_mode):
            raise RuntimeAclConvergenceError(
                "deployed environment file must be a regular file"
            )
        with os.fdopen(descriptor, mode="r", encoding="utf-8") as stream:
            descriptor = None
            file_values = dotenv_values(stream=stream, interpolate=False)
    except OSError as exc:
        raise RuntimeAclConvergenceError(
            "unable to read the deployed environment file"
        ) from exc
    finally:
        if descriptor is not None:
            os.close(descriptor)
    return dict(file_values)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--environment", choices=sorted(_IDENTITIES), required=True
    )
    parser.add_argument("--environment-file", type=Path, required=True)
    parser.add_argument("--bundle-root", type=Path, required=True)
    args = parser.parse_args()

    try:
        bundle_root = _attest_bundle(args.bundle_root)
        target = resolve_acl_target(
            args.environment, _load_database_values(args.environment_file)
        )
        command, process_environment = build_psql_invocation(bundle_root, target)
        completed = subprocess.run(
            command,
            check=False,
            env=process_environment,
        )
        if completed.returncode != 0:
            raise RuntimeAclConvergenceError(
                "owner-executable runtime ACL convergence failed"
            )
    except (OSError, RuntimeAclConvergenceError) as exc:
        print(f"runtime ACL convergence refused: {exc}", file=sys.stderr)
        return 1
    print(
        "runtime ACL convergence complete: "
        f"{target.database_name} app={target.app_role} backup={target.backup_role}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
