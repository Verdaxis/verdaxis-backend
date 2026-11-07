"""Create the database schema that the previous release left on its host.

The previous release migrated its database with Alembic to `REVISION`. Alembic
is no longer in the source, so the schema comes from the frozen SQL baseline,
whose recorded Alembic head is that revision. The baseline is rendered in the
catalog form of a migrated database (`deploy/tests/migrated-catalog-sql.py`),
because the current release's adoption fingerprint checks that form. The
migrator owns every object and `alembic_version` records the revision, as the
previous release left them.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

REVISION = "ob_20261031_offboarding"


def schema_sql(root: Path, migrator: str) -> bytes:
    """Return psql input that builds the schema as `migrator` in one database."""
    recorded = json.loads((root / "deploy/rust/sqlx-baseline.json").read_text())
    if recorded["alembic_head"] != REVISION:
        raise RuntimeError(
            f"the SQL baseline records {recorded['alembic_head']}, "
            f"not the previous release's revision {REVISION}"
        )
    baseline = subprocess.run(
        [
            sys.executable,
            str(root / "deploy/tests/migrated-catalog-sql.py"),
            str(root / "rs/read/migrations/1_baseline.sql"),
        ],
        capture_output=True,
        check=True,
    ).stdout
    # The baseline clears search_path, so the history table is schema-qualified.
    return (
        f'SET ROLE "{migrator}";\n'.encode()
        + baseline
        + (
            "\nCREATE TABLE public.alembic_version"
            "(version_num varchar(32) NOT NULL PRIMARY KEY);\n"
            f"INSERT INTO public.alembic_version VALUES ('{REVISION}');\n"
            "RESET ROLE;\n"
        ).encode()
    )
