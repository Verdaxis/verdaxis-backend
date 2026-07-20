#!/usr/bin/env python3
"""Apply one source-attested, explicitly allowlisted Alembic checkpoint."""

from __future__ import annotations

import argparse
import asyncio
import os
from pathlib import Path
import re
import subprocess
import sys
from typing import Any

from alembic import command
from alembic.config import Config
from alembic.runtime.migration import MigrationContext
from alembic.script import ScriptDirectory
from alembic.script.revision import RangeNotAncestorError, ResolutionError
from sqlalchemy import pool
from sqlalchemy.engine import make_url
from sqlalchemy.exc import ArgumentError
from sqlalchemy.ext.asyncio import create_async_engine


POLICY_PATH = "deploy/migration-checkpoints.tsv"
_FULL_SHA = re.compile(r"[0-9a-f]{40}")
_REVISION = re.compile(r"[A-Za-z0-9][A-Za-z0-9_]{0,127}")
_SYMBOLIC_REVISIONS = {"base", "head", "heads"}


class MigrationCheckpointError(RuntimeError):
    """The requested deployment migration is not exactly authorized."""


def _validate_revision(value: str) -> str:
    if (
        _REVISION.fullmatch(value) is None
        or value.lower() in _SYMBOLIC_REVISIONS
    ):
        raise MigrationCheckpointError("checkpoint revisions must be literal IDs")
    return value


def parse_checkpoint_policy(text: str) -> set[tuple[str, str]]:
    """Parse exact expected-current/target pairs from the committed policy."""
    transitions: set[tuple[str, str]] = set()
    for line_number, raw_line in enumerate(text.splitlines(), start=1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) != 2:
            raise MigrationCheckpointError(
                f"invalid migration checkpoint policy line {line_number}"
            )
        transition = tuple(_validate_revision(field.strip()) for field in fields)
        if transition in transitions:
            raise MigrationCheckpointError("duplicate migration checkpoint transition")
        transitions.add(transition)
    if not transitions:
        raise MigrationCheckpointError("migration checkpoint policy is empty")
    return transitions


def validate_checkpoint_request(
    *,
    policy: set[tuple[str, str]],
    source_sha: str,
    approved_source_sha: str,
    expected_current: str,
    target: str,
    current_heads: tuple[str, ...],
    script_directory: ScriptDirectory,
) -> None:
    """Validate source identity, live state, allowlist, and graph ancestry."""
    if (
        _FULL_SHA.fullmatch(source_sha) is None
        or _FULL_SHA.fullmatch(approved_source_sha) is None
        or source_sha != approved_source_sha
    ):
        raise MigrationCheckpointError("migration approval source SHA is not exact")
    expected_current = _validate_revision(expected_current)
    target = _validate_revision(target)
    if (expected_current, target) not in policy:
        raise MigrationCheckpointError(
            "expected-current/target migration pair is not allowlisted"
        )
    if current_heads != (expected_current,):
        raise MigrationCheckpointError(
            "live Alembic current revision does not exactly match approval"
        )
    try:
        script_directory.get_revision(expected_current)
        script_directory.get_revision(target)
        if target != expected_current:
            revisions = tuple(
                script_directory.iterate_revisions(target, expected_current)
            )
            if not revisions or revisions[0].revision != target:
                raise MigrationCheckpointError(
                    "migration target is not a descendant checkpoint"
                )
    except RangeNotAncestorError as exc:
        raise MigrationCheckpointError(
            "migration target is not a descendant checkpoint"
        ) from exc
    except (ResolutionError, ValueError) as exc:
        raise MigrationCheckpointError(
            "migration checkpoint is absent from the selected source graph"
        ) from exc


def _git(source_root: Path, *args: str) -> str:
    environment = {
        key: value for key, value in os.environ.items() if not key.startswith("GIT_")
    }
    environment["GIT_CONFIG_GLOBAL"] = "/dev/null"
    environment["GIT_CONFIG_NOSYSTEM"] = "1"
    environment["GIT_NO_REPLACE_OBJECTS"] = "1"
    completed = subprocess.run(
        [
            "/usr/bin/git",
            "-c",
            "core.hooksPath=/dev/null",
            "-C",
            str(source_root),
            *args,
        ],
        check=False,
        capture_output=True,
        text=True,
        env=environment,
    )
    if completed.returncode != 0:
        raise MigrationCheckpointError("unable to read selected source identity")
    return completed.stdout


def read_committed_checkpoint_policy(source_root: Path, source_sha: str) -> str:
    root = source_root.resolve(strict=True)
    if Path(_git(root, "rev-parse", "--show-toplevel").strip()).resolve() != root:
        raise MigrationCheckpointError("migration source root is not a Git root")
    if _git(root, "rev-parse", "HEAD").strip() != source_sha:
        raise MigrationCheckpointError("migration source SHA does not match checkout")
    if _git(root, "status", "--porcelain", "--untracked-files=all"):
        raise MigrationCheckpointError("migration source checkout is dirty")
    return _git(root, "show", f"{source_sha}:{POLICY_PATH}")


def _require_explicit_migrator_url(settings: Any) -> str:
    migration_url = getattr(settings, "MIGRATOR_DATABASE_URL", None)
    app_url = getattr(settings, "DATABASE_URL", None)
    if not isinstance(migration_url, str) or not migration_url.strip():
        raise MigrationCheckpointError(
            "an explicit MIGRATOR_DATABASE_URL is required for checkpoint access"
        )
    if not isinstance(app_url, str) or not app_url.strip():
        raise MigrationCheckpointError(
            "an explicit DATABASE_URL is required to attest distinct checkpoint authority"
        )

    migration_url = migration_url.strip()
    app_url = app_url.strip()
    try:
        parsed_migration_url = make_url(migration_url)
        parsed_app_url = make_url(app_url)
    except (ArgumentError, TypeError, ValueError) as exc:
        raise MigrationCheckpointError(
            "checkpoint database URLs must be valid and identify explicit roles"
        ) from exc

    if (
        migration_url == app_url
        or not parsed_migration_url.username
        or not parsed_app_url.username
        or parsed_migration_url.username == parsed_app_url.username
    ):
        raise MigrationCheckpointError(
            "MIGRATOR_DATABASE_URL must be distinct from DATABASE_URL and use a distinct role"
        )
    migration_endpoint = (
        (parsed_migration_url.host or "").lower(),
        parsed_migration_url.port or 5432,
    )
    app_endpoint = (
        (parsed_app_url.host or "").lower(),
        parsed_app_url.port or 5432,
    )
    if migration_endpoint != app_endpoint:
        raise MigrationCheckpointError(
            "DATABASE_URL and MIGRATOR_DATABASE_URL must use the same database endpoint"
        )
    return migration_url


async def _read_current_heads(settings: Any) -> tuple[str, ...]:
    migration_url = _require_explicit_migrator_url(settings)
    from app.database import migrator_connect_args, verify_migrator_connection

    engine = create_async_engine(
        migration_url,
        poolclass=pool.NullPool,
        connect_args=migrator_connect_args(settings),
        hide_parameters=True,
    )
    try:
        async with engine.connect() as connection:
            await connection.run_sync(
                lambda sync_connection: verify_migrator_connection(
                    sync_connection, settings
                )
            )
            heads = await connection.run_sync(
                lambda sync_connection: MigrationContext.configure(
                    sync_connection
                ).get_current_heads()
            )
    finally:
        await engine.dispose()
    return tuple(sorted(heads))


async def verify_current_revision(settings: Any, expected: str) -> None:
    """Require the live database to remain at the deployed literal checkpoint."""
    expected = _validate_revision(expected)
    if await _read_current_heads(settings) != (expected,):
        raise MigrationCheckpointError(
            "runtime migration revision does not match deployed checkpoint"
        )


async def execute_checkpoint(
    *,
    config: Config,
    settings: Any,
    policy: set[tuple[str, str]],
    source_sha: str,
    approved_source_sha: str,
    expected_current: str,
    target: str,
) -> None:
    """Attest before/after revisions and invoke Alembic with only the target ID."""
    if settings.RELEASE_SHA != source_sha:
        raise MigrationCheckpointError(
            "effective runtime release SHA does not match migration source SHA"
        )
    migration_url = _require_explicit_migrator_url(settings)
    config.set_main_option("sqlalchemy.url", migration_url.replace("%", "%%"))
    script_directory = ScriptDirectory.from_config(config)
    current_heads = await _read_current_heads(settings)
    validate_checkpoint_request(
        policy=policy,
        source_sha=source_sha,
        approved_source_sha=approved_source_sha,
        expected_current=expected_current,
        target=target,
        current_heads=current_heads,
        script_directory=script_directory,
    )
    if target != expected_current:
        command.upgrade(config, target)
    resulting_heads = await _read_current_heads(settings)
    if resulting_heads != (target,):
        raise MigrationCheckpointError(
            "migration did not finish at the exact approved target revision"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--approved-source-sha", required=True)
    parser.add_argument("--expected-current", required=True)
    parser.add_argument("--target", required=True)
    args = parser.parse_args()

    try:
        policy_text = read_committed_checkpoint_policy(
            args.source_root, args.source_sha
        )
        policy = parse_checkpoint_policy(policy_text)
        sys.path.insert(0, str(args.source_root.resolve(strict=True)))
        from app.config import settings

        config = Config(str(args.source_root / "alembic.ini"))
        asyncio.run(
            execute_checkpoint(
                config=config,
                settings=settings,
                policy=policy,
                source_sha=args.source_sha,
                approved_source_sha=args.approved_source_sha,
                expected_current=args.expected_current,
                target=args.target,
            )
        )
    except (MigrationCheckpointError, OSError) as exc:
        print(f"migration checkpoint refused: {exc}", file=sys.stderr)
        return 1
    print(
        "migration checkpoint applied: "
        f"{args.expected_current} -> {args.target} at {args.source_sha}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
