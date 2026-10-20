"""Disposable twin logical-state support using the declared asyncpg driver."""
import asyncio
import hashlib
import json
from pathlib import Path

import asyncpg

ROOT = Path(__file__).resolve().parents[2]
DATABASES = ("phase2_python_test", "phase2_rust_test")
APP_ROLE = "verdaxis_phase2_app_test"
MIGRATOR_ROLE = "verdaxis_phase2_migrator_test"
BACKUP_ROLE = "verdaxis_phase2_backup_test"
PASSWORD = "phase2-disposable-not-for-runtime"


def identifier(name):
    return '"' + name.replace('"', '""') + '"'


class Rows(list):
    def fetchall(self):
        return self
    def fetchone(self):
        return tuple(self[0])


class Connection:
    """Synchronous lifecycle adapter; all driver calls share one private loop."""
    def __init__(self, port, database, app=False, autocommit=False):
        self.loop = asyncio.new_event_loop()
        self.connection = self.loop.run_until_complete(asyncpg.connect(host="127.0.0.1", port=port, database=database,
            user=APP_ROLE if app else "postgres", password=PASSWORD if app else "disposable_admin_pw"))
        self.transaction = None
        if not autocommit:
            self.transaction = self.connection.transaction()
            self.loop.run_until_complete(self.transaction.start())
    def execute(self, statement, parameters=()):
        for index in range(len(parameters)):
            statement = statement.replace("%s", f"${index + 1}", 1)
        return Rows(self.loop.run_until_complete(self.connection.fetch(statement, *parameters)))
    def commit(self):
        if self.transaction:
            self.loop.run_until_complete(self.transaction.commit())
            self.transaction = None
    def rollback(self):
        if self.transaction:
            self.loop.run_until_complete(self.transaction.rollback())
            self.transaction = None
    def __enter__(self):
        return self
    def __exit__(self, kind, value, traceback):
        try:
            self.rollback() if kind else self.commit()
        finally:
            self.loop.run_until_complete(self.connection.close())
            self.loop.close()


def snapshot(connection):
    tables = connection.execute("SELECT tablename FROM pg_tables WHERE schemaname='public' ORDER BY tablename")
    result = {}
    for (table,) in tables:
        if table in {"alembic_version", "spatial_ref_sys"}:
            continue
        columns = [row[0] for row in connection.execute("SELECT column_name FROM information_schema.columns WHERE table_schema='public' AND table_name=%s ORDER BY ordinal_position", (table,))]
        primary = [row[0] for row in connection.execute("SELECT a.attname FROM pg_index i JOIN pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=ANY(i.indkey) WHERE i.indrelid=%s::regclass AND i.indisprimary ORDER BY array_position(i.indkey,a.attnum)", (table,))]
        rows = connection.execute(f"SELECT row_to_json(t)::text FROM {identifier(table)} t")
        mapped = {}
        for (raw,) in rows:
            row = json.loads(raw)
            key = json.dumps([row[name] for name in primary], sort_keys=True) if primary else json.dumps(row, sort_keys=True)
            if key in mapped:
                raise RuntimeError("duplicate logical row identity")
            mapped[key] = row
        result[table] = {"columns": columns, "rows": mapped}
    return result


def sequence_marks(connection):
    marks = {}
    for schema, name in connection.execute("SELECT schemaname, sequencename FROM pg_sequences WHERE schemaname='public' ORDER BY sequencename"):
        row = connection.execute(f"SELECT last_value,is_called FROM {identifier(schema)}.{identifier(name)}").fetchone()
        marks[name] = {"last_value": row[0], "is_called": row[1]}
    return marks


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def connect(port, database, *, app=False, autocommit=False):
    return Connection(port, database, app, autocommit)
