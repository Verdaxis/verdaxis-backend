#!/usr/bin/env python3
"""Publish and rotate the public-read cache authority (deploy/recovery owner).

The Rust read service may serve a cached trading payload only while an owner
record, kept outside every database, names an active authority, and the database
connection answering the request reports the same identity. This script is the
only writer of that record and of the database row it must match.

Commands (run by the operator, never by an ordinary deploy):

  revoke    Publish a revoked record and wait until every Rust process has
            re-read it or lost its lease. Use it before stopping or restoring a
            database.
  activate  Revoke and drain, bind the database incarnation row to a fresh
            epoch, primary identity and revision through the migrator role, then
            publish the matching active record. Run it after a promotion, a
            restore or a database replacement, before traffic returns.
  status    Print the record without printing full identities.

Order is what makes this safe: the record is revoked before the database row
changes, the drain wait exceeds the Rust lease, and the active record is
published only after the database row is committed. Any failure in between
leaves the record revoked, so trading caching stays off.
"""
from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
import re
import secrets
import stat
import sys
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

RECORD_NAME = "cache-authority.json"
RECORD_VERSION = 1
# Same bound as AUTHORITY_LEASE in rs/read/src/runtime/cache_authority.rs; a unit
# test keeps the two aligned. A Rust process that cannot re-read the record stops
# serving hits after this long, with no routing change.
LEASE_SECONDS = 5
# Twice the lease: every process has either re-read the revoked record or let its
# lease expire before the successor identity can be bound.
FENCE_DRAIN_SECONDS = 2 * LEASE_SECONDS
# The record is a few hundred bytes; the Rust reader refuses anything larger.
MAX_RECORD_BYTES = 4096
# The revision is stored in a BIGINT column and read as a signed value by Rust.
MAX_REVISION = 2**63 - 1
_MAX_RECORD_REVISION = 2**64 - 1
_IDENTITY = re.compile(r"[0-9a-f]{32,128}")

Sleep = Callable[[float], Awaitable[None]]


class AuthorityError(RuntimeError):
    """The authority record or incarnation cannot be changed safely."""


@dataclass(frozen=True)
class Authority:
    database_epoch: str
    primary_identity: str
    revision: int


@dataclass(frozen=True)
class Record:
    """One owner record. `authority` is None for a deliberate revocation."""

    revision: int
    authority: Authority | None


@dataclass(frozen=True)
class IncarnationRow:
    database_epoch: str
    primary_identity: str
    authority_revision: int
    active: bool
    in_recovery: bool


class IncarnationStore(Protocol):
    async def read(self) -> IncarnationRow: ...

    async def bind(self, authority: Authority) -> None: ...


def new_identity() -> str:
    """A fresh random identity, allocated here and never by a database."""
    return secrets.token_hex(32)


def _is_revision(value: object, minimum: int) -> bool:
    return type(value) is int and minimum <= value <= _MAX_RECORD_REVISION


def parse_record(text: str) -> Record:
    """Parse an owner record. The Rust reader accepts and refuses the same texts."""
    try:
        raw = json.loads(text)
    except ValueError as exc:
        raise AuthorityError("cache authority record is not JSON") from exc
    allowed = {"version", "state", "authority_revision", "database_epoch", "primary_identity"}
    if not isinstance(raw, dict) or not set(raw) <= allowed:
        raise AuthorityError("cache authority record has unexpected fields")
    if raw.get("version") != RECORD_VERSION or type(raw.get("version")) is not int:
        raise AuthorityError("unsupported cache authority record version")
    revision = raw.get("authority_revision")
    epoch = raw.get("database_epoch")
    primary = raw.get("primary_identity")
    state = raw.get("state")
    if state == "revoked" and epoch is None and primary is None and _is_revision(revision, 0):
        return Record(revision, None)
    if (
        state == "active"
        and isinstance(epoch, str)
        and isinstance(primary, str)
        and _IDENTITY.fullmatch(epoch)
        and _IDENTITY.fullmatch(primary)
        and _is_revision(revision, 1)
    ):
        return Record(revision, Authority(epoch, primary, revision))
    raise AuthorityError("invalid cache authority record state")


def render_record(record: Record) -> str:
    body: dict[str, object] = {
        "version": RECORD_VERSION,
        "authority_revision": record.revision,
        "state": "revoked" if record.authority is None else "active",
    }
    if record.authority is not None:
        body["database_epoch"] = record.authority.database_epoch
        body["primary_identity"] = record.authority.primary_identity
    return json.dumps(body, sort_keys=True, separators=(",", ":")) + "\n"


def read_record(runtime_dir: Path) -> Record | None:
    """Return the current record, None when absent. Refuse links and odd files."""
    target = runtime_dir / RECORD_NAME
    try:
        descriptor = os.open(target, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except FileNotFoundError:
        return None
    except OSError as exc:
        raise AuthorityError("cache authority record must be a readable regular file") from exc
    with os.fdopen(descriptor, "rb") as stream:
        status = os.fstat(stream.fileno())
        if not stat.S_ISREG(status.st_mode) or status.st_size > MAX_RECORD_BYTES:
            raise AuthorityError("cache authority record must be a small regular file")
        return parse_record(stream.read().decode("utf-8", errors="replace"))


def publish_record(runtime_dir: Path, record: Record) -> Path:
    """Publish atomically: one private temporary file, one rename, a directory sync."""
    if runtime_dir.is_symlink() or not runtime_dir.is_dir():
        raise AuthorityError("the Rust runtime directory must be a real directory")
    target = runtime_dir / RECORD_NAME
    temporary = runtime_dir / f".{RECORD_NAME}.tmp.{os.getpid()}"
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(descriptor, "w") as stream:
            stream.write(render_record(record))
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, target)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise
    directory = os.open(runtime_dir, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
    return target


def _current_revision(runtime_dir: Path) -> int:
    """The published revision, or zero for an absent or unreadable record."""
    try:
        record = read_record(runtime_dir)
    except AuthorityError:
        return 0
    return 0 if record is None else record.revision


async def revoke_and_drain(runtime_dir: Path, *, floor: int, sleep: Sleep) -> int:
    """Publish a revoked record, then wait out every Rust lease. Returns its revision."""
    revision = max(floor, _current_revision(runtime_dir)) + 1
    if revision > MAX_REVISION - 1:
        raise AuthorityError("cache authority revision space is exhausted")
    publish_record(runtime_dir, Record(revision, None))
    await sleep(FENCE_DRAIN_SECONDS)
    return revision


async def activate(
    runtime_dir: Path, store: IncarnationStore, *, sleep: Sleep = asyncio.sleep
) -> Authority:
    """Revoke, drain, bind the database to a fresh identity, publish it."""
    row = await store.read()
    if row.in_recovery:
        raise AuthorityError("the database is a replica; promote it before activation")
    revoked = await revoke_and_drain(runtime_dir, floor=row.authority_revision, sleep=sleep)
    authority = Authority(new_identity(), new_identity(), revoked + 1)
    # A copied or reused identity would defeat the fence; allocation is random,
    # so a collision means the entropy source is broken.
    previous = read_record(runtime_dir)
    reused = {row.database_epoch, row.primary_identity}
    if {authority.database_epoch, authority.primary_identity} & reused or (
        previous is not None
        and previous.authority is not None
        and {authority.database_epoch, authority.primary_identity}
        & {previous.authority.database_epoch, previous.authority.primary_identity}
    ):
        raise AuthorityError("allocated identity repeats an earlier identity")
    # A failure here leaves the record revoked, so caching stays off.
    await store.bind(authority)
    publish_record(runtime_dir, Record(authority.revision, authority))
    return authority


class SqlIncarnationStore:
    """The migrator-owned incarnation row, read and bound through one connection.

    The connection must have no open transaction: each method runs in its own.
    """

    def __init__(self, connection) -> None:
        self._connection = connection

    async def read(self) -> IncarnationRow:
        from sqlalchemy import text

        async with self._connection.begin():
            result = await self._connection.execute(
                text(
                    "SELECT database_epoch, primary_identity, authority_revision, active, "
                    "pg_catalog.pg_is_in_recovery() AS in_recovery "
                    "FROM public.public_read_cache_incarnation WHERE singleton"
                )
            )
            row = result.one_or_none()
        if row is None:
            raise AuthorityError("the database has no cache incarnation row")
        return IncarnationRow(*row)

    async def bind(self, authority: Authority) -> None:
        """Bind one fresh identity with a higher revision, in one transaction."""
        from sqlalchemy import text

        if authority.revision > MAX_REVISION:
            raise AuthorityError("cache authority revision space is exhausted")
        async with self._connection.begin():
            current = (
                await self._connection.execute(
                    text(
                        "SELECT database_epoch, primary_identity, authority_revision "
                        "FROM public.public_read_cache_incarnation WHERE singleton FOR UPDATE"
                    )
                )
            ).one_or_none()
            if current is None:
                raise AuthorityError("the database has no cache incarnation row")
            if authority.revision <= current.authority_revision:
                raise AuthorityError("authority revision must increase")
            if {authority.database_epoch, authority.primary_identity} & {
                current.database_epoch,
                current.primary_identity,
            }:
                raise AuthorityError("authority identities must be fresh")
            updated = await self._connection.execute(
                text(
                    "UPDATE public.public_read_cache_incarnation "
                    "SET database_epoch = :epoch, primary_identity = :primary, "
                    "authority_revision = :revision, active = true WHERE singleton"
                ),
                {
                    "epoch": authority.database_epoch,
                    "primary": authority.primary_identity,
                    "revision": authority.revision,
                },
            )
            if updated.rowcount != 1:
                raise AuthorityError("the cache incarnation row was not bound")


def _fingerprint(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()[:12]


def describe(record: Record | None) -> dict[str, object]:
    """Operator-facing summary that never prints a full identity."""
    if record is None:
        return {"state": "absent"}
    if record.authority is None:
        return {"state": "revoked", "authority_revision": record.revision}
    return {
        "state": "active",
        "authority_revision": record.revision,
        "database_epoch_fingerprint": _fingerprint(record.authority.database_epoch),
        "primary_identity_fingerprint": _fingerprint(record.authority.primary_identity),
    }


async def _activate_through_migrator(
    runtime_dir: Path, environment_file: Path, source_root: Path
) -> Authority:
    # Import lazily: the pure record functions above need none of these.
    import importlib

    import apply_migration_checkpoint as checkpoint
    from sqlalchemy import pool
    from sqlalchemy.ext.asyncio import create_async_engine

    checkpoint.load_environment_file(environment_file)
    root = checkpoint.activate_source_root(source_root)
    config_module = importlib.import_module("app.config")
    database_module = importlib.import_module("app.database")
    checkpoint.require_module_from_source(config_module, root)
    checkpoint.require_module_from_source(database_module, root)
    settings = config_module.settings
    # The one definition of "explicit, distinct migrator authority"; never
    # duplicate it here.
    migration_url = checkpoint._require_explicit_migrator_url(settings)
    engine = create_async_engine(
        migration_url,
        poolclass=pool.NullPool,
        connect_args=database_module.migrator_connect_args(settings),
        hide_parameters=True,
    )
    try:
        async with engine.connect() as connection:
            await connection.run_sync(
                lambda sync_connection: database_module.verify_migrator_connection(
                    sync_connection, settings
                )
            )
            # The attestation above begins an implicit transaction.
            await connection.rollback()
            return await activate(runtime_dir, SqlIncarnationStore(connection))
    finally:
        await engine.dispose()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("revoke", "activate", "status"):
        command = commands.add_parser(name)
        command.add_argument("--runtime-dir", required=True, type=Path)
    activate_parser = commands.choices["activate"]
    activate_parser.add_argument("--environment-file", required=True, type=Path)
    activate_parser.add_argument("--source-root", default=".", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "status":
            print(json.dumps(describe(read_record(args.runtime_dir)), sort_keys=True))
        elif args.command == "revoke":
            revision = asyncio.run(
                revoke_and_drain(args.runtime_dir, floor=0, sleep=asyncio.sleep)
            )
            print(f"cache authority revoked at revision {revision}")
        else:
            authority = asyncio.run(
                _activate_through_migrator(
                    args.runtime_dir, args.environment_file, args.source_root
                )
            )
            print(f"cache authority activated at revision {authority.revision}")
    except (AuthorityError, OSError) as exc:
        print(f"cache authority refused: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
