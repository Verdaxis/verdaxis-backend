"""Lossless PostgreSQL text transport; SQL NULL is never JSON null.

PostgreSQL renders numeric, UUID, JSON, bytea and timestamp values directly.
Geography uses EWKB, including its SRID. No application or JSON float decoder
participates in copying or hashing business values.
"""
from __future__ import annotations

import hashlib
from collections.abc import Mapping
from dataclasses import dataclass

from sqlalchemy import text
from sqlalchemy.dialects.postgresql import dialect
from sqlalchemy.sql.compiler import IdentifierPreparer

_PREPARER = IdentifierPreparer(dialect())
BATCH_SIZE = 256


async def configure_text_session(connection):
    """Use round-trip-safe text on both transport and independent hash reads."""
    # postgres interval syntax also matches the frozen catalog's CHECK text.
    for setting in (
        "TimeZone='UTC'", "DateStyle='ISO, YMD'", "IntervalStyle='postgres'",
        'extra_float_digits=3', "bytea_output='hex'",
    ):
        await connection.execute(text(f'SET LOCAL {setting}'))


def quote(name: str) -> str:
    return _PREPARER.quote_identifier(name)


def relation(name: str) -> str:
    return f'public.{quote(name)}'


@dataclass(frozen=True)
class TableMap:
    name: str
    copied_columns: tuple[str, ...]
    added_values: Mapping[str, object]


def map_row(row: Mapping[str, object], mapping: TableMap) -> dict[str, object]:
    if set(row) != set(mapping.copied_columns):
        raise ValueError('Source column set changed')
    return {**row, **mapping.added_values}


def primary_key(table: dict) -> tuple[str, ...]:
    definitions = [c['definition'] for c in table['constraints'] if c['kind'] == 'p']
    if len(definitions) != 1:
        raise ValueError('Exactly one primary key is required')
    return tuple(part.strip().strip('"') for part in definitions[0].removeprefix('PRIMARY KEY (').removesuffix(')').split(','))


def select_text(table: dict, columns: tuple[str, ...] | None = None) -> str:
    selected = set(columns) if columns is not None else None
    fields = []
    for column in table['columns']:
        name = column['name']
        if selected is not None and name not in selected:
            continue
        field = quote(name)
        if column['type'].startswith(('geography', 'geometry')):
            expression = f"encode(ST_AsEWKB({field}::geometry), 'hex')"
        else:
            expression = f'{field}::text'
        fields.append(f'{expression} AS {field}')
    order = ', '.join(quote(key) for key in primary_key(table))
    return f'SELECT {", ".join(fields)} FROM {relation(table["name"])} ORDER BY {order}'


async def text_batches(connection, table: dict, columns=None):
    await configure_text_session(connection)
    result = await connection.stream(text(select_text(table, columns)))
    try:
        async for batch in result.mappings().partitions(BATCH_SIZE):
            yield [dict(row) for row in batch]
    finally:
        await result.close()


def insert_statement(table: dict, names: tuple[str, ...]):
    types = {column['name']: column['type'] for column in table['columns']}
    expressions = []
    for index, name in enumerate(names):
        sql_type = types[name]
        # Types come only from the exact committed and catalog-attested contract.
        parameter = f'CAST(:v{index} AS text)'
        if sql_type.startswith(('geography', 'geometry')):
            expression = f"ST_GeomFromEWKB(decode({parameter}, 'hex'))::{sql_type}"
        else:
            expression = f'CAST({parameter} AS {sql_type})'
        expressions.append(expression)
    return text(f'INSERT INTO {relation(table["name"])} ({", ".join(map(quote, names))}) VALUES ({", ".join(expressions)})')


def frame(value: str | None) -> bytes:
    if value is None:
        return b'N'
    encoded = value.encode('utf-8')
    return b'V' + len(encoded).to_bytes(8, 'big') + encoded


class RowHash:
    def __init__(self, names):
        self.names = tuple(names)
        self.count = 0
        self.rows = hashlib.sha256()
        self.columns = {name: hashlib.sha256() for name in self.names}

    def add(self, row):
        self.rows.update(b'R')
        for name in self.names:
            value = frame(row[name])
            self.rows.update(value)
            self.columns[name].update(value)
        self.count += 1

    def report(self):
        return dict(count=self.count, sha256=self.rows.hexdigest(),
                    columns={name: value.hexdigest() for name, value in self.columns.items()})


def next_sequence_state(definition, last_value: int, is_called: bool, imported_max: int | None):
    increment = int(definition['increment'])
    if increment <= 0 or definition['cycle']:
        raise ValueError('Only reviewed positive, noncycling sequences are supported')
    low, high = int(definition['minimum']), int(definition['maximum'])
    if not low <= last_value <= high or int(definition['cache']) < 1:
        raise ValueError('Invalid sequence state')
    preserve_uncalled = not is_called and (imported_max is None or imported_max < last_value)
    value = last_value if imported_max is None else max(last_value, imported_max)
    called = not preserve_uncalled
    next_value = value + increment if called else value
    if not low <= next_value <= high:
        raise ValueError('Sequence exhausted')
    return value, called, next_value
