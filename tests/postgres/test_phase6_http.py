"""Stream route HTTP contract checks on the role-separated disposable database."""

from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def test_stream_http_component():
    result = subprocess.run(
        ["cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked",
         "-p", "verdaxis-read", "--features", "contract-tests", "--test",
         "streams_http", "--", "--test-threads=1"],
        cwd=ROOT,
        env={**os.environ, "SQLX_OFFLINE": "true"},
        capture_output=True,
        text=True,
        timeout=1800,
        check=False,
    )
    print(result.stdout)
    print(result.stderr[-4000:])
    assert result.returncode == 0
    assert "0 failed" in result.stdout
    assert re.search(r"test result: ok\. 1[0-9] passed", result.stdout)
