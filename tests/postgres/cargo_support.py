"""Environment for cargo gates run from the Python PostgreSQL suite.

``rs/read/build.rs`` accepts only a full lowercase 40-hex release SHA. The
Python test environment sets a placeholder value, so a cargo subprocess must
receive the repository's own HEAD instead of inheriting it.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]


def repository_release_sha(repo_root: Path = REPO_ROOT) -> str:
    """Return the full SHA of the checked-out commit the cargo gate builds."""
    completed = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=repo_root,
        capture_output=True,
        check=True,
        text=True,
    )
    return completed.stdout.strip()


def cargo_test_environment(
    analytics_pg_url: str,
    repo_root: Path = REPO_ROOT,
) -> dict[str, str]:
    """Return the subprocess environment shared by the Phase 3 cargo gates."""
    return {
        **os.environ,
        "AUTH_EMAIL_TEST_OWNER_URL": analytics_pg_url.replace(
            "postgresql+asyncpg://", "postgresql://"
        ),
        "AUTH_EMAIL_TEST_DATABASE_URL": os.environ["DATABASE_URL"].replace(
            "postgresql+asyncpg://", "postgresql://"
        ),
        "SQLX_OFFLINE": "true",
        "RELEASE_SHA": repository_release_sha(repo_root),
    }
