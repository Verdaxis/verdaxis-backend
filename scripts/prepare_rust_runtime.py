#!/usr/bin/env python3
"""Prepare the minimal long-running Rust runtime environment.

Called by the canonical deploy transaction after the verified artifact is
staged. It reads the deployed `.env` with no-follow regular-file semantics,
projects ONLY the app database settings, the KYC execution switch and the
runtime identity the Rust HTTP process needs, and writes
`.runtime-rust/runtime.env` at mode 0600.

The Rust process must never receive mail, JWT, provider, or migrator
credentials: its read pool is the shared app role, and the migrator-backed
checks stay in the short-lived prestart control plane. The file also names the
staged release directory and the prepared budget artifact so systemd's
`EnvironmentFile=` lines resolve to one reviewed selection.
"""
from __future__ import annotations

import argparse
import io
import json
import os
import stat
import sys
import urllib.parse
from pathlib import Path

from dotenv import dotenv_values

RUST_POLICY_PATH = Path("deploy/rust/staging-policy.json")
BUDGET_SCRIPT = "scripts/prepare_rust_budget.py"
RUNTIME_KEYS = ("ENVIRONMENT", "RELEASE_SHA")
APP_DATABASE_KEYS = ("DATABASE_URL",)
KYC_GATE_KEY = "KYC_EXECUTION_GATE_ENABLED"
# Pydantic accepts these spellings for a boolean setting; the deployed file
# decides the value, exactly as the Python service reads it. A missing key is
# Python's default (enabled), never Rust's own choice.
_BOOL_TRUE = frozenset({"true", "t", "y", "yes", "1", "on"})
_BOOL_FALSE = frozenset({"false", "f", "n", "no", "0", "off"})

_IDENTITIES = {
    "production": {
        "database_name": "verdaxis_prod",
        "app_role": "verdaxis_app",
    },
    "staging": {
        "database_name": "verdaxis_staging",
        "app_role": "verdaxis_app_staging",
    },
}


class RustRuntimeError(RuntimeError):
    """The Rust runtime environment cannot be prepared safely."""


def read_regular(path: Path) -> bytes:
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except OSError as exc:
        raise RustRuntimeError(f"{path} must be a readable regular file") from exc
    with os.fdopen(descriptor, "rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise RustRuntimeError(f"{path} must be a regular file")
        return stream.read()


def load_policy(source_root: Path, environment: str = "staging") -> dict:
    if environment not in _IDENTITIES:
        raise RustRuntimeError("unsupported Rust deployment environment")
    policy_path = Path(f"deploy/rust/{environment}-policy.json")
    try:
        policy = json.loads(read_regular(source_root / policy_path))
    except (ValueError, RustRuntimeError) as exc:
        raise RustRuntimeError(f"{environment} Rust policy is not readable JSON") from exc
    if not isinstance(policy, dict):
        raise RustRuntimeError(f"{environment} Rust policy has unexpected shape")
    staged = policy.get("staged_releases_dir")
    unit = "verdaxis-rust.service" if environment == "production" else "verdaxis-rust-staging.service"
    bind = "127.0.0.1:8010" if environment == "production" else "127.0.0.1:8011"
    if (
        not isinstance(policy, dict)
        or policy.get("version") != 1
        or policy.get("environment") != environment
        or policy.get("service_name") != unit
        or policy.get("bind") != bind
        or policy.get("health_url") != f"http://{bind}/health/ready"
        or not isinstance(policy.get("db_session_time_zone"), str)
        or not isinstance(staged, str)
        or staged.startswith("/")
        or ".." in staged.split("/")
        or policy["db_session_time_zone"] not in {"UTC", "Asia/Singapore"}
    ):
        raise RustRuntimeError("staging Rust policy has unexpected shape")
    return policy


def app_database_url(values: dict, environment: str) -> str:
    """Return the exact deployed app DATABASE_URL, identity-checked.

    The deployed `.env` carries SQLAlchemy's ``postgresql+asyncpg://`` form; the
    Rust process needs libpq's ``postgres://`` form, so the driver suffix is
    rewritten after the identity check, preserving the percent-encoded
    components exactly.
    """
    identity = _IDENTITIES[environment]
    raw = values.get("DATABASE_URL")
    if not isinstance(raw, str) or not raw.strip():
        raise RustRuntimeError("the deployed .env must carry an explicit DATABASE_URL")
    url = urllib.parse.urlsplit(raw.strip())
    if (
        url.scheme not in {"postgres", "postgresql", "postgresql+asyncpg", "postgresql+psycopg2"}
        or url.query
        or url.fragment
        or url.username != identity["app_role"]
        or (url.path.lstrip("/") or None) != identity["database_name"]
        or url.password is None
        or not url.password.strip()
    ):
        raise RustRuntimeError(
            "DATABASE_URL does not match the selected application identity"
        )
    if url.scheme == "postgresql+asyncpg":
        url = url._replace(scheme="postgresql")
    return urllib.parse.urlunsplit(url)


def kyc_execution_gate(values: dict) -> str:
    """Return the KYC execution switch as the Python service resolves it.

    The Rust public reads bind the same switch, so a Rust side that guessed the
    value would answer differently from Python. The deployed ``.env`` is the
    authority and an absent key keeps Python's default (enabled).
    """
    raw = values.get(KYC_GATE_KEY)
    if raw is None:
        return "true"
    if not isinstance(raw, str):
        raise RustRuntimeError(f"{KYC_GATE_KEY} must be true or false")
    normalized = raw.strip().lower()
    if normalized in _BOOL_TRUE:
        return "true"
    if normalized in _BOOL_FALSE:
        return "false"
    raise RustRuntimeError(f"{KYC_GATE_KEY} must be true or false")


def project_runtime_environment(
    environment_file: Path, release_sha: str, migration_revision: str, policy: dict
) -> dict:
    raw = read_regular(environment_file)
    values = dotenv_values(
        stream=io.StringIO(raw.decode("utf-8")), interpolate=False
    )
    if any(values.get(key) is None for key in RUNTIME_KEYS):
        raise RustRuntimeError("the deployed .env must define ENVIRONMENT and RELEASE_SHA")
    environment = policy["environment"]
    if environment == "production" and migration_revision != "p89_20261022_trade_freeze":
        raise RustRuntimeError("production requires its reviewed freeze checkpoint")
    if values["ENVIRONMENT"] != environment or values["RELEASE_SHA"] != release_sha:
        raise RustRuntimeError(
            "the deployed environment identity does not match this release"
        )
    return {
        "ENVIRONMENT": environment,
        "RELEASE_SHA": release_sha,
        "MIGRATION_REVISION": migration_revision,
        "DATABASE_URL": app_database_url(values, environment),
        "DB_SESSION_TIME_ZONE": policy["db_session_time_zone"],
        "READ_ADMISSION_SLOTS": values.get("READ_ADMISSION_SLOTS", "16"),
        KYC_GATE_KEY: kyc_execution_gate(values),
    }


def write_env_file(output_dir: Path, projected: dict) -> Path:
    output_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    if output_dir.is_symlink() or not output_dir.is_dir():
        raise RustRuntimeError("the Rust runtime directory must be a real directory")
    target = output_dir / "runtime.env"
    if target.exists() and target.is_symlink():
        raise RustRuntimeError("runtime.env must never be a symlink")
    # A value with whitespace or shell metacharacters is double-quoted so
    # systemd's EnvironmentFile parser keeps it as one value.
    def render(key: str, value: str) -> str:
        plain = value != "" and urllib.parse.quote(value, safe="") == value
        return f"{key}={value}\n" if plain else f'{key}="{value}"\n'

    body = "".join(render(key, value) for key, value in sorted(projected.items()))
    # Atomic publication: one create, one rename, never an in-place rewrite.
    temporary = output_dir / f".runtime.env.tmp.{os.getpid()}"
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as stream:
        stream.write(body)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, target)
    os.chmod(target, 0o600)
    return target


def prepare(
    environment_file: Path,
    release_sha: str,
    output_dir: Path,
    source_root: Path,
    migration_revision: str,
    environment: str = "staging",
) -> dict:
    policy = load_policy(source_root, environment)
    projected = project_runtime_environment(
        environment_file, release_sha, migration_revision, policy
    )
    written = write_env_file(output_dir, projected)
    return {"policy": policy, "runtime_env": written}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--environment-file", required=True, type=Path)
    parser.add_argument("--release-sha", required=True)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--source-root", default=".", type=Path)
    parser.add_argument("--migration-revision", required=True)
    parser.add_argument("--environment", choices=tuple(_IDENTITIES), default="staging")
    args = parser.parse_args()
    try:
        result = prepare(
            args.environment_file,
            args.release_sha,
            args.output_dir,
            args.source_root,
            args.migration_revision,
            args.environment,
        )
    except (OSError, RustRuntimeError, UnicodeDecodeError) as exc:
        print(f"Rust runtime preparation refused: {exc}", file=sys.stderr)
        return 1
    print(f"prepared {result['runtime_env']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
