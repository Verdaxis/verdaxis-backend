#!/usr/bin/env python3
"""Fail closed unless the live database is at the deployed checkpoint."""

from __future__ import annotations

import argparse
import asyncio
from pathlib import Path
import sys


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from apply_migration_checkpoint import (  # noqa: E402
    MigrationCheckpointError,
    verify_current_revision,
)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--expected", required=True)
    args = parser.parse_args()

    try:
        from app.config import settings

        asyncio.run(verify_current_revision(settings, args.expected))
    except (MigrationCheckpointError, OSError) as exc:
        print(f"runtime migration checkpoint refused: {exc}", file=sys.stderr)
        return 1
    print(f"runtime migration checkpoint verified: {args.expected}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
