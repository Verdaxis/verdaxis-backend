"""Keep restricted ORM defaults aligned with the runtime column ACL."""

from __future__ import annotations

import re
from pathlib import Path

from sqlalchemy import Column, MetaData, String, Table

import app.models  # noqa: F401 - registers all model tables on Base.metadata
from app.model_base import Base


_POLICY_PATH = (
    Path(__file__).resolve().parents[2] / "deploy/postgres/app_acl_policy.sql"
)


def _acl_policy() -> tuple[dict[str, set[str]], dict[tuple[str, str], set[str]]]:
    source = _POLICY_PATH.read_text()
    table_source = source.split(
        "INSERT INTO app_table_policy (table_name, privileges) VALUES", 1
    )[1].split("CREATE TEMP TABLE app_column_policy", 1)[0]
    column_source = source.split(
        "INSERT INTO app_column_policy (table_name, column_name, privilege_type) VALUES",
        1,
    )[1].split("CREATE TEMP TABLE app_sequence_policy", 1)[0]

    table_policy = {
        table_name: set(re.findall(r"'(SELECT|INSERT|UPDATE|DELETE)'", privileges))
        for table_name, privileges in re.findall(
            r"\('([^']+)', ARRAY\[([^\]]+)\]\)", table_source
        )
    }
    column_policy: dict[tuple[str, str], set[str]] = {}
    for table_name, column_name, privilege in re.findall(
        r"\('([^']+)', '([^']+)', '(INSERT|UPDATE)'\)", column_source
    ):
        column_policy.setdefault((table_name, privilege), set()).add(column_name)
    return table_policy, column_policy


def _ungranted_client_defaults(
    metadata: MetaData,
    table_policy: dict[str, set[str]],
    column_policy: dict[tuple[str, str], set[str]],
) -> set[str]:
    violations: set[str] = set()
    for (table_name, privilege), allowed_columns in column_policy.items():
        table = metadata.tables.get(table_name)
        if table is None:
            violations.add(f"{table_name}: column ACL has no mapped table")
            continue

        missing_columns = allowed_columns.difference(table.c.keys())
        violations.update(
            f"{table_name}.{column}: column ACL has no mapped column"
            for column in missing_columns
        )

        if privilege in table_policy.get(table_name, set()):
            continue
        generated_columns = {
            column.name
            for column in table.columns
            if (
                column.default is not None
                if privilege == "INSERT"
                else column.onupdate is not None
            )
        }
        violations.update(
            f"{table_name}.{column}: client-side {privilege} default is not granted"
            for column in generated_columns.difference(allowed_columns)
        )
    return violations


def test_restricted_model_defaults_match_runtime_column_acl():
    table_policy, column_policy = _acl_policy()

    assert _ungranted_client_defaults(
        Base.metadata, table_policy, column_policy
    ) == set()


def test_conformance_check_detects_an_ungranted_client_default():
    metadata = MetaData()
    Table(
        "restricted_table",
        metadata,
        Column("allowed_insert", String, default="allowed"),
        Column("denied_insert", String, default="denied"),
        Column("allowed_update", String, onupdate="allowed"),
        Column("denied_update", String, onupdate="denied"),
    )

    assert _ungranted_client_defaults(
        metadata,
        {"restricted_table": {"SELECT"}},
        {
            ("restricted_table", "INSERT"): {"allowed_insert"},
            ("restricted_table", "UPDATE"): {"allowed_update"},
        },
    ) == {
        "restricted_table.denied_insert: client-side INSERT default is not granted",
        "restricted_table.denied_update: client-side UPDATE default is not granted",
    }
