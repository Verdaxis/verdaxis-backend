#!/usr/bin/env python3
"""Prepare or verify a private Rust budget artifact. Deployment owns locking."""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace

from dotenv import dotenv_values

FIELDS = {
    "services": "DB_SERVICE_COUNT", "workers": "UVICORN_WORKERS",
    "pool": "DB_POOL_SIZE", "overflow": "DB_MAX_OVERFLOW",
    "reserve": "DB_RESERVED_CONNECTIONS", "configured_max_connections": "DB_MAX_CONNECTIONS",
}


def derive_budget(settings, rust_pool, cache_listener=0, stream_listener=0):
    """Derive the configured upper bound, including each Python dispatcher.

    `cache_listener` is the one dedicated connection of the public cache change
    listener. It is zero for every existing start; the key is then left out of
    the artifact so disabled-cache artifacts stay byte-identical.
    """
    values = {key: getattr(settings, field) for key, field in FIELDS.items()}
    if (type(rust_pool) is not int or not 2 <= rust_pool <= 0xFFFFFFFF
            or type(cache_listener) is not int or cache_listener not in (0, 1)
            or type(stream_listener) is not int or stream_listener not in (0, 1)
            or (stream_listener == 1 and values["workers"] < 4)
            or any(type(value) is not int or not 0 <= value <= 0xFFFFFFFF for value in values.values())):
        raise ValueError("invalid budget inputs")
    python_budget = values["services"] * values["workers"] * (values["pool"] + values["overflow"] + 1) + values["reserve"]
    total = python_budget + rust_pool + cache_listener + stream_listener
    if total > min(0xFFFFFFFF, values["configured_max_connections"]):
        raise ValueError("connection budget exceeds configured maximum")
    budget = {**values, "rust_pool": rust_pool, "python_budget": python_budget, "total": total}
    if cache_listener:
        budget["cache_listener"] = cache_listener
    if stream_listener:
        budget["stream_listener"] = stream_listener
    return budget


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


def artifact_for(environment_file, release_sha, cache_listener=0, stream_listener=0):
    if not re.fullmatch(r"[0-9a-f]{40}", release_sha) or release_sha == "0" * 40:
        raise ValueError("full release SHA required")
    raw = read_regular(environment_file)
    environment = dotenv_values(stream=io.StringIO(raw.decode("utf-8")), interpolate=False)
    raw_slots = environment.get("READ_ADMISSION_SLOTS", "16")
    if raw_slots is None or not re.fullmatch(r"[0-9]+", raw_slots.strip()):
        raise ValueError("READ_ADMISSION_SLOTS must be a positive integer")
    slots = int(raw_slots)
    return {"version": 1, "release_sha": release_sha,
            "environment_digest": hashlib.sha256(raw).hexdigest(),
            **derive_budget(effective_settings(raw), slots + 1, cache_listener, stream_listener)}


def prepare(environment_file, release_sha, output, cache_listener=0, stream_listener=0):
    artifact = artifact_for(environment_file, release_sha, cache_listener, stream_listener)
    # The deploy lock serializes writers. Open the directory without following
    # a symlink, write a private sibling, then publish by atomic rename.
    directory = os.open(output.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    temporary = f".{output.name}.{os.getpid()}.tmp"
    try:
        try:
            previous = os.stat(output.name, dir_fd=directory, follow_symlinks=False)
        except FileNotFoundError:
            previous = None
        if previous is not None and not stat.S_ISREG(previous.st_mode):
            raise ValueError("budget output must be a regular file")
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                             0o600, dir_fd=directory)
        try:
            with os.fdopen(descriptor, "w") as stream:
                json.dump(artifact, stream, sort_keys=True)
                stream.write("\n")
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, output.name, src_dir_fd=directory, dst_dir_fd=directory)
            os.fsync(directory)
        finally:
            try:
                os.unlink(temporary, dir_fd=directory)
            except FileNotFoundError:
                pass
    finally:
        os.close(directory)


def verify(environment_file, release_sha, artifact):
    recorded = json.loads(read_regular(artifact))
    if not isinstance(recorded, dict):
        raise ValueError("runtime budget artifact is not an object")
    # The listener choice was made when the artifact was prepared under the
    # deployment lock; verification recomputes everything else from the inputs.
    expected = artifact_for(environment_file, release_sha, recorded.get("cache_listener", 0),
                            recorded.get("stream_listener", 0))
    if recorded != expected:
        raise ValueError("runtime budget or environment digest changed; prepare again under the deployment lock")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--environment-file", required=True, type=Path)
    parser.add_argument("--release-sha", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--verify", action="store_true")
    parser.add_argument("--cache-listener", type=int, choices=(0, 1), default=0,
                        help="budget the dedicated cache listener connection (prepare only)")
    parser.add_argument("--stream-listener", type=int, choices=(0, 1), default=0,
                        help="budget the dedicated stream election connection (prepare only)")
    args = parser.parse_args()
    try:
        if args.verify:
            verify(args.environment_file, args.release_sha, args.output)
        else:
            prepare(args.environment_file, args.release_sha, args.output, args.cache_listener,
                    args.stream_listener)
    except (OSError, ValueError, subprocess.SubprocessError):
        print("Rust runtime budget refused", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
