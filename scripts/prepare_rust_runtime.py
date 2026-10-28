#!/usr/bin/env python3
"""Prepare the minimal long-running Rust runtime environment.

Called by the canonical deploy transaction after the verified artifact is
staged. It reads the deployed `.env` with no-follow regular-file semantics,
projects ONLY the app database settings, the KYC execution switch, the runtime
identity, the optional stream status heartbeat settings and (Task 23) the Phase 2
verification/invite settings the Rust HTTP process needs, and writes `.runtime-rust/runtime.env` at mode 0600.

The migrator URL and every unrelated provider key stay out: its read pool is
the shared app role, and the migrator-backed checks stay in the short-lived
prestart control plane. The Phase 2 JWT and Resend values go into this private
file only; the Python runtime keeps reading its own `.env`. The file also names
the staged release directory and the prepared budget artifact so systemd's
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
# Task 23: the six authenticated CRUD groups verify tokens and send one invite,
# so these are required in the deployed environment. The previous key is the
# one optional value: it exists only while a rotation is in progress.
PHASE2_REQUIRED_KEYS = (
    "JWT_SECRET",
    "JWT_ISSUER",
    "JWT_AUDIENCE",
    "FRONTEND_URL",
    "EMAIL_FROM",
    "RESEND_API_KEY",
)
PHASE2_PREVIOUS_KEY = "JWT_SECRET_PREVIOUS"
# The status every stream heartbeat reports. Both keys are optional: an absent
# or blank key is not projected, and the Rust runtime then reports
# "operational" with no message.
SYSTEM_STATUS_KEY = "SYSTEM_STATUS"
SYSTEM_STATUS_MESSAGE_KEY = "SYSTEM_STATUS_MESSAGE"
SYSTEM_STATUSES = frozenset({"operational", "degraded", "maintenance"})
MAX_SYSTEM_STATUS_MESSAGE_CHARS = 280
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


# The deployed Python unit's optional second environment file and the keys the
# Rust runtime takes from it.
ANALYTICS_ENVIRONMENT_FILE = ".env.analytics"
UMAMI_KEYS = ("UMAMI_BASE_URL", "UMAMI_WEBSITE_ID", "UMAMI_API_USERNAME", "UMAMI_API_PASSWORD")


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


def phase2_settings(values: dict) -> dict:
    """Return the private Phase 2 settings, refusing a partial projection.

    A missing key is refused here, before a restart, because the Rust service
    would refuse the same environment after the transaction committed. Values
    keep their deployed spelling: the Rust side parses and validates the token
    authority and the URL itself.
    """
    projected = {}
    for key in PHASE2_REQUIRED_KEYS:
        raw = values.get(key)
        if not isinstance(raw, str) or not raw.strip():
            raise RustRuntimeError(f"the deployed .env must define {key} for Phase 2")
        projected[key] = raw
    if len(projected["JWT_SECRET"]) < 32:
        raise RustRuntimeError("JWT_SECRET must be at least 32 characters")
    previous = values.get(PHASE2_PREVIOUS_KEY)
    if previous is not None:
        if not isinstance(previous, str):
            raise RustRuntimeError(f"{PHASE2_PREVIOUS_KEY} must be text")
        if previous.strip():
            if len(previous) < 32:
                raise RustRuntimeError(
                    f"{PHASE2_PREVIOUS_KEY} must be at least 32 characters"
                )
            projected[PHASE2_PREVIOUS_KEY] = previous
    frontend = urllib.parse.urlsplit(projected["FRONTEND_URL"])
    if (
        frontend.scheme not in {"http", "https"}
        or not frontend.netloc
        or any(character.isspace() for character in projected["FRONTEND_URL"])
    ):
        raise RustRuntimeError("FRONTEND_URL must be an absolute http or https URL")
    return projected


def phase3_settings(values: dict) -> dict:
    """Project the monitor authority and installed, versioned domain datasets.

    Missing datasets disable domain matching, as in the registration component.
    No provider or domain lookup runs in a request.
    """
    try:
        from disposable_email_domains import blocklist
        from free_email_domains import whitelist
    except ImportError:
        datasets = None
    else:
        datasets = {"public": sorted(whitelist), "disposable": sorted(blocklist)}
    projected = {"AUTH_DOMAIN_DATASETS": json.dumps(datasets, separators=(",", ":"))}
    monitor = values.get("MONITOR_TOKEN")
    if monitor is not None:
        if not isinstance(monitor, str):
            raise RustRuntimeError("MONITOR_TOKEN must be text")
        if monitor:
            projected["MONITOR_TOKEN"] = monitor
    return projected


def system_status_settings(values: dict) -> dict:
    """Return the validated stream heartbeat status and message, if set.

    The Rust service refuses an unknown status or a long message at startup, so
    the same check runs here, before a restart. The status is projected trimmed
    and lowercase; the message is projected trimmed. A blank value is the same
    as an absent one.
    """
    projected = {}
    status = values.get(SYSTEM_STATUS_KEY)
    if status is not None:
        if not isinstance(status, str):
            raise RustRuntimeError(f"{SYSTEM_STATUS_KEY} must be text")
        normalized = status.strip().lower()
        if normalized:
            if normalized not in SYSTEM_STATUSES:
                raise RustRuntimeError(
                    f"{SYSTEM_STATUS_KEY} must be operational, degraded or maintenance"
                )
            projected[SYSTEM_STATUS_KEY] = normalized
    message = values.get(SYSTEM_STATUS_MESSAGE_KEY)
    if message is not None:
        if not isinstance(message, str):
            raise RustRuntimeError(f"{SYSTEM_STATUS_MESSAGE_KEY} must be text")
        message = message.strip()
        if len(message) > MAX_SYSTEM_STATUS_MESSAGE_CHARS:
            raise RustRuntimeError(
                f"{SYSTEM_STATUS_MESSAGE_KEY} must be at most "
                f"{MAX_SYSTEM_STATUS_MESSAGE_CHARS} characters"
            )
        if message:
            projected[SYSTEM_STATUS_MESSAGE_KEY] = message
    return projected


def project_runtime_environment(
    environment_file: Path, release_sha: str, migration_revision: str, policy: dict
) -> dict:
    raw = read_regular(environment_file)
    values = dotenv_values(
        stream=io.StringIO(raw.decode("utf-8")), interpolate=False
    )
    # The Python unit also loads the Umami aggregate settings from an optional
    # second file next to .env; its values win, as in the unit's file order.
    analytics_file = environment_file.with_name(ANALYTICS_ENVIRONMENT_FILE)
    if analytics_file.exists() or analytics_file.is_symlink():
        analytics = dotenv_values(
            stream=io.StringIO(read_regular(analytics_file).decode("utf-8")),
            interpolate=False,
        )
        for key in UMAMI_KEYS:
            if analytics.get(key) is not None:
                values[key] = analytics[key]
    if any(values.get(key) is None for key in RUNTIME_KEYS):
        raise RustRuntimeError("the deployed .env must define ENVIRONMENT and RELEASE_SHA")
    environment = policy["environment"]
    if environment == "production" and migration_revision not in {
        "p0_20261016_union",
        "prc_20261026_changes",
        "p3_20261020_auth_tokens",
        "sse_20261021_replay_floor",
        "p5_20261020_read_indexes",
    }:
        raise RustRuntimeError("production requires its reviewed union checkpoint")
    if values["ENVIRONMENT"] != environment or values["RELEASE_SHA"] != release_sha:
        raise RustRuntimeError(
            "the deployed environment identity does not match this release"
        )
    projected = {
        "ENVIRONMENT": environment,
        "RELEASE_SHA": release_sha,
        "MIGRATION_REVISION": migration_revision,
        "DATABASE_URL": app_database_url(values, environment),
        "DB_SESSION_TIME_ZONE": policy["db_session_time_zone"],
        "READ_ADMISSION_SLOTS": values.get("READ_ADMISSION_SLOTS", "16"),
        KYC_GATE_KEY: kyc_execution_gate(values),
    }
    projected.update(phase2_settings(values))
    projected.update(phase3_settings(values))
    # Read-only aggregates and seller fees are the only business additions.
    for key, default in (("SELLER_FEE_PILOT_PER_MT_USD", "2"),
                         ("SELLER_FEE_PROFESSIONAL_PER_MT_USD", "1.50")):
        from decimal import Decimal, InvalidOperation
        try:
            rate = Decimal(values.get(key) or default)
            if not rate.is_finite() or rate < 0 or rate > 100000 or rate.as_tuple().exponent < -2:
                raise ValueError("invalid seller fee")
        except (InvalidOperation, ValueError) as exc:
            raise RustRuntimeError("seller fee must be finite and nonnegative") from exc
        projected[key] = str(rate)
    for key in UMAMI_KEYS:
        value = values.get(key)
        if isinstance(value, str) and value.strip():
            projected[key] = value
    projected["UMAMI_ENABLED"] = "true" if all(projected.get(key) for key in UMAMI_KEYS) else "false"
    projected.update(system_status_settings(values))
    # Refuse control characters; quotes and backslashes are escaped by the
    # EnvironmentFile renderer without changing the deployed value.
    for key, value in projected.items():
        if any(ord(character) < 32 or ord(character) == 127 for character in value):
            raise RustRuntimeError(
                f"{key} contains a character the runtime file cannot carry"
            )
    return projected


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
        if any(ord(character) < 32 or ord(character) == 127 for character in value):
            raise RustRuntimeError(f"{key} contains a control character")
        escaped = value.replace("\\", "\\\\").replace('"', '\\"')
        return f"{key}={value}\n" if plain else f'{key}="{escaped}"\n'

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
    # Domain datasets exceed operating-system per-variable limits. Publish them
    # beside the private runtime file, and carry only their absolute path.
    domains = projected.pop("AUTH_DOMAIN_DATASETS")
    output_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    if output_dir.is_symlink():
        raise RustRuntimeError("the Rust runtime directory must be a real directory")
    domain_path = output_dir / "auth-domains.json"
    temporary = output_dir / f".auth-domains.tmp.{os.getpid()}"
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as stream:
        stream.write(domains)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, domain_path)
    projected["AUTH_DOMAIN_DATASETS_FILE"] = str(domain_path.resolve())
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
