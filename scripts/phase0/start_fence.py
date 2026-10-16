#!/usr/bin/python3
"""Read-only external start fence. Install outside the checkout as root.

This guard adds a condition; it never replaces verify_deploy_start_guard.py.
Only the CLI is a deployed entry point. Tests inject a private directory/UID.
"""
from __future__ import annotations

import argparse
import errno
import fcntl
import json
import os
import re
import stat
from pathlib import Path

CONTROL = Path("/var/lib/verdaxis/phase0")
DATABASES = {"production": "verdaxis_prod", "staging": "verdaxis_staging"}
PERMIT_KEYS = {"environment", "release_sha", "database", "lock_device", "lock_inode"}
FLAGS = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC


def _check_file(fd: int, uid: int) -> os.stat_result:
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode) or info.st_uid != uid or info.st_nlink != 1:
        raise ValueError("Untrusted control file")
    if stat.S_IMODE(info.st_mode) != 0o600:
        raise ValueError("Control file must be private")
    return info


def _read(directory: int, name: str, uid: int) -> bytes:
    fd = os.open(name, FLAGS, dir_fd=directory)
    try:
        before = _check_file(fd, uid)
        data = os.read(fd, 4097)
        after = os.fstat(fd)
        current = os.stat(name, dir_fd=directory, follow_symlinks=False)
        if (len(data) > 4096 or len(data) != before.st_size
                or before.st_size != after.st_size
                or before.st_mtime_ns != after.st_mtime_ns
                or before.st_ctime_ns != after.st_ctime_ns):
            raise ValueError("Control file changed or is oversized")
        if (after.st_dev, after.st_ino) != (current.st_dev, current.st_ino):
            raise ValueError("Control file replaced")
        return data
    finally:
        os.close(fd)


def _unique_pairs(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("Duplicate permit field")
        value[key] = item
    return value


def start_is_allowed(control: Path, environment: str, release_sha: str,
                     database: str, *, owner_uid: int = 0) -> bool:
    """Allow absence only in a valid control directory; fail closed on errors."""
    directory = lock = -1
    try:
        if environment not in DATABASES or re.fullmatch(r"[0-9a-f]{40}", release_sha) is None:
            return False
        # The old tuple is valid only for supervised pre-write rollback. A
        # root-issued permit must still bind it; no runtime name override exists.
        if database not in ({"verdaxis_prod", "verdaxis"} if environment == "production"
                            else {"verdaxis_staging"}):
            return False
        directory = os.open(control, FLAGS | os.O_DIRECTORY)
        info = os.fstat(directory)
        if info.st_uid != owner_uid or stat.S_IMODE(info.st_mode) != 0o700:
            return False
        try:
            os.stat("fence", dir_fd=directory, follow_symlinks=False)
        except FileNotFoundError:
            return True
        if _read(directory, "fence", owner_uid) != b"phase0\n":
            return False
        permit = json.loads(_read(directory, "permit.json", owner_uid), object_pairs_hook=_unique_pairs)
        if not isinstance(permit, dict) or set(permit) != PERMIT_KEYS:
            return False
        if (permit["environment"], permit["release_sha"], permit["database"]) != (environment, release_sha, database):
            return False
        lock = os.open("operator.lock", FLAGS, dir_fd=directory)
        info = _check_file(lock, owner_uid)
        if any(type(permit[key]) is not int for key in ("lock_device", "lock_inode")):
            return False
        if (info.st_dev, info.st_ino) != (permit["lock_device"], permit["lock_inode"]):
            return False
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            if error.errno not in (errno.EAGAIN, errno.EACCES):
                return False
        else:
            fcntl.flock(lock, fcntl.LOCK_UN)
            return False
        current = os.stat("operator.lock", dir_fd=directory, follow_symlinks=False)
        return (info.st_dev, info.st_ino) == (current.st_dev, current.st_ino)
    except (OSError, ValueError, TypeError, UnicodeError):
        return False
    finally:
        if lock >= 0:
            os.close(lock)
        if directory >= 0:
            os.close(directory)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--environment", choices=DATABASES, required=True)
    parser.add_argument("--expected-release-sha", required=True)
    parser.add_argument("--database", choices=("verdaxis", "verdaxis_prod", "verdaxis_staging"), required=True)
    args = parser.parse_args()
    # No user-controlled parent may replace the external guard's control root.
    for path in (Path("/var"), Path("/var/lib"), Path("/var/lib/verdaxis")):
        info = path.lstat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022:
            return 1
    return 0 if start_is_allowed(CONTROL, args.environment, args.expected_release_sha, args.database) else 1


if __name__ == "__main__":
    raise SystemExit(main())
