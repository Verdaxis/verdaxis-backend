#!/usr/bin/env python3
"""Read-only deployed runtime preflight for configuration and app identity."""

from __future__ import annotations

import argparse
import asyncio
from pathlib import Path
import sys


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--environment", choices=("production", "staging"), required=True)
    parser.add_argument("--release-sha", required=True)
    args = parser.parse_args()

    # Imports happen after argument parsing so the operator must explicitly
    # provide ENVIRONMENT/RELEASE_SHA to Settings through the invoking command.
    from app.config import settings
    from app.database import verify_database_runtime

    if settings.ENVIRONMENT != args.environment:
        raise RuntimeError("effective ENVIRONMENT does not match preflight target")
    if settings.RELEASE_SHA != args.release_sha:
        raise RuntimeError("effective RELEASE_SHA does not match preflight target")
    asyncio.run(verify_database_runtime(settings))
    print("runtime configuration and database identity preflight passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
