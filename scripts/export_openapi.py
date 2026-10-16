#!/usr/bin/env python3
"""Export the committed OpenAPI contract for the FastAPI application.

The repository owns the API contract: `openapi.json` at the repository root is
committed here and vendored by the frontend at an exact SHA, which generates its
API types from it. `tests/unit/test_openapi_snapshot.py` fails when the
committed file differs from a fresh export, so any route or schema change must
regenerate the file in the same commit.

The export pins the documented test environment instead of inheriting the
ambient shell, so the bytes never depend on a deployment identity:

    ENVIRONMENT=test RELEASE_SHA=test \\
    JWT_SECRET=test-secret-key-for-testing-minimum-32-chars \\
    DATABASE_URL=sqlite+aiosqlite:///:memory: \\
    venv/bin/python scripts/export_openapi.py

The document is written to `openapi.json` at the repository root, or printed to
stdout with `--stdout` for the drift test, for diffing, and for transfers where
this checkout cannot be written. A one-line path/schema count goes to stderr, so
stdout stays pure JSON. Output is deterministic: sorted keys, two-space indent,
a trailing newline, no deployment identity, and no environment-dependent fields
(see `normalize`); the same source tree always produces the same bytes.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import sys

REPO_ROOT = Path(__file__).resolve().parents[1]
OUTPUT_PATH = REPO_ROOT / "openapi.json"

# Running this file by path puts `scripts/` on `sys.path`, so add the repository
# root to import the application package the same way the unit suite does.
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

# The application validates its runtime identity at import time, so the exporter
# fixes the same values the unit-test job uses. `RELEASE_SHA` must be the exact
# environment name or a full 40-hex commit sha; in `test` it is the literal
# "test". Nothing here reaches a database or the network.
EXPORT_ENVIRONMENT = {
    "ENVIRONMENT": "test",
    "RELEASE_SHA": "test",
    "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars",
    "DATABASE_URL": "sqlite+aiosqlite:///:memory:",
}


def pin_export_environment() -> None:
    """Fix the export environment before any `app` module reads it."""
    for name, value in EXPORT_ENVIRONMENT.items():
        if os.environ.get(name) != value:
            os.environ[name] = value


def normalize(document: dict) -> dict:
    """Drop deployment fields so the bytes depend only on the API contract.

    An export under `test`, `development` and `production` produces identical
    bytes today: the document holds only `openapi`, `info`, `paths` and
    `components`, with no release sha, origin, host or absolute URL. FastAPI
    publishes a `servers` entry if the app is ever built with one or with a
    non-empty `root_path`, so drop it as a guard rather than let an
    environment-specific origin reach the committed contract.
    """
    normalized = dict(document)
    normalized.pop("servers", None)
    return normalized


def build_document() -> dict:
    """Return the normalized OpenAPI document for the current source tree."""
    pin_export_environment()
    # Imported after the environment is fixed: `app.config.settings` is built on
    # first import and validates the runtime identity.
    from app.main import app  # noqa: PLC0415

    return normalize(app.openapi())


def render(document: dict) -> str:
    """Serialize a document to the committed on-disk form."""
    return json.dumps(document, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--stdout",
        action="store_true",
        help="print the document instead of writing openapi.json",
    )
    args = parser.parse_args(argv)

    document = build_document()
    rendered = render(document)
    if args.stdout:
        sys.stdout.write(rendered)
    else:
        OUTPUT_PATH.write_text(rendered, encoding="utf-8", newline="\n")
        print(f"wrote {OUTPUT_PATH.relative_to(REPO_ROOT)} ({len(rendered)} bytes)")

    # stderr, so a --stdout export stays pure JSON for a pipe or a redirect.
    print(
        f"{OUTPUT_PATH.name}: {len(document['paths'])} paths, "
        f"{len(document['components']['schemas'])} schemas",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
