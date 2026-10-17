#!/usr/bin/env python3
"""Prepare or verify a private Rust budget artifact. Deployment owns locking."""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
from types import SimpleNamespace

from dotenv import dotenv_values

FIELDS = {
    "services": "DB_SERVICE_COUNT", "workers": "UVICORN_WORKERS",
    "pool": "DB_POOL_SIZE", "overflow": "DB_MAX_OVERFLOW",
    "reserve": "DB_RESERVED_CONNECTIONS", "configured_max_connections": "DB_MAX_CONNECTIONS",
}


def derive_budget(settings, rust_pool):
    """Derive the configured upper bound, including each Python dispatcher."""
    values = {key: getattr(settings, field) for key, field in FIELDS.items()}
    if rust_pool != 9 or any(type(value) is not int or not 0 <= value <= 0xFFFFFFFF for value in values.values()):
        raise ValueError("invalid budget inputs")
    python_budget = values["services"] * values["workers"] * (values["pool"] + values["overflow"] + 1) + values["reserve"]
    total = python_budget + rust_pool
    if total > min(0xFFFFFFFF, values["configured_max_connections"]):
        raise ValueError("connection budget exceeds configured maximum")
    return {**values, "rust_pool": rust_pool, "python_budget": python_budget, "total": total}


def read_regular(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError("input must be a regular file")
        return stream.read()


def effective_settings(raw):
    values = dotenv_values(stream=io.StringIO(raw.decode("utf-8")), interpolate=False)
    # A fresh process avoids ambient Settings overrides and the module singleton.
    # An empty working directory also prevents implicit .env discovery on import.
    root = str(Path(__file__).resolve().parents[1])
    code = (
        "import json, os, sys; "
        "os.environ.clear(); os.environ.update(json.load(sys.stdin)); "
        f"sys.path.insert(0, {root!r}); "
        "from app.config import Settings; s = Settings(_env_file=None); "
        f"print(json.dumps({{k: getattr(s, k) for k in {list(FIELDS.values())!r}}}))"
    )
    with tempfile.TemporaryDirectory() as directory:
        result = subprocess.run([sys.executable, "-I", "-c", code],
            input=json.dumps({key: value for key, value in values.items() if value is not None}),
            text=True, capture_output=True, cwd=directory, env={}, timeout=30, check=False)
    if result.returncode:
        # Settings errors can contain database passwords; never relay stderr.
        raise ValueError("effective Python settings refused")
    return SimpleNamespace(**json.loads(result.stdout))


def artifact_for(environment_file, release_sha):
    if not re.fullmatch(r"[0-9a-f]{40}", release_sha) or release_sha == "0" * 40:
        raise ValueError("full release SHA required")
    raw = read_regular(environment_file)
    return {"version": 1, "release_sha": release_sha,
            "environment_digest": hashlib.sha256(raw).hexdigest(),
            **derive_budget(effective_settings(raw), 9)}


def prepare(environment_file, release_sha, output):
    artifact = artifact_for(environment_file, release_sha)
    # Never overwrite another preparation or follow an output symlink.
    descriptor = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "w") as stream:
        json.dump(artifact, stream, sort_keys=True)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())


def verify(environment_file, release_sha, artifact):
    expected = artifact_for(environment_file, release_sha)
    if json.loads(read_regular(artifact)) != expected:
        raise ValueError("runtime budget or environment digest changed; prepare again under the deployment lock")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--environment-file", required=True, type=Path)
    parser.add_argument("--release-sha", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args()
    try:
        operation = verify if args.verify else prepare
        operation(args.environment_file, args.release_sha, args.output)
    except (OSError, ValueError, subprocess.SubprocessError):
        print("Rust runtime budget refused", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
