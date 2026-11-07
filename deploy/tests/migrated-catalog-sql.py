#!/usr/bin/env python3
"""Print the frozen SQL baseline in the catalog form of a migrated database.

`rs/read/migrations/1_baseline.sql` is a pg_dump of the schema at the last
Alembic revision. pg_dump renders a varchar IN-list as a text-cast array, while
a database that the migrations built stores the IN-list form, and the compiled
adoption fingerprint checks that source form. This reparses only those CHECK
and index expressions as IN-lists; every other line is printed unchanged.

Usage: migrated-catalog-sql.py BASELINE_SQL > schema.sql
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

VARCHAR_LIST = re.compile(
    r"\(\(([a-z_][a-z_0-9]*)\)::text (= ANY|<> ALL) "
    r"\(\(ARRAY\[((?:'(?:[^']|'')*'::character varying(?:, )?)+)\]\)::text\[\]\)\)"
)


def source_list(match: re.Match[str]) -> str:
    column, operation, values = match.groups()
    operator = "IN" if operation == "= ANY" else "NOT IN"
    return f"({column} {operator} ({values.replace('::character varying', '')}))"


def migrated_catalog_sql(baseline: str) -> str:
    lines = []
    for line in baseline.splitlines(keepends=True):
        if " CHECK " in line or line.startswith(("CREATE INDEX ", "CREATE UNIQUE INDEX ")):
            line = VARCHAR_LIST.sub(source_list, line)
        lines.append(line)
    return "".join(lines)


def main(arguments: list[str]) -> int:
    if len(arguments) != 1:
        print(__doc__.strip().splitlines()[-1], file=sys.stderr)
        return 2
    sys.stdout.write(migrated_catalog_sql(Path(arguments[0]).read_text()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
