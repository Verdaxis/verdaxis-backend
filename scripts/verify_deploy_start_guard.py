#!/usr/bin/python3
"""Allow a backend start only outside deploys or inside the live restart window."""

from __future__ import annotations

import errno
import fcntl
import os
import re
import stat
import sys
from pathlib import Path

_ALLOWED_ENVIRONMENTS = frozenset({"production", "staging"})
_RELEASE_SHA_RE = re.compile(r"[0-9a-f]{40}")
_MAX_STATE_BYTES = 256
_OPEN_DIRECTORY_FLAGS = os.O_RDONLY | os.O_CLOEXEC | os.O_DIRECTORY | os.O_NOFOLLOW
_OPEN_FILE_FLAGS = os.O_RDONLY | os.O_CLOEXEC | os.O_NONBLOCK | os.O_NOFOLLOW


def _same_entry(left: os.stat_result, right: os.stat_result) -> bool:
    return left.st_dev == right.st_dev and left.st_ino == right.st_ino


def _owner_controlled(entry: os.stat_result, expected_type: int) -> bool:
    return (
        stat.S_IFMT(entry.st_mode) == expected_type
        and entry.st_uid == os.geteuid()
        and (expected_type != stat.S_IFREG or entry.st_nlink == 1)
        and entry.st_mode & 0o022 == 0
    )


def _open_runtime_root(path: Path) -> int:
    descriptor = os.open(path, _OPEN_DIRECTORY_FLAGS)
    opened = os.fstat(descriptor)
    current = os.stat(path, follow_symlinks=False)
    if (
        not _same_entry(opened, current)
        or not stat.S_ISDIR(opened.st_mode)
        or opened.st_uid != os.geteuid()
    ):
        os.close(descriptor)
        raise OSError(errno.EPERM, "untrusted runtime root")
    return descriptor


def _open_verified_file(directory_fd: int, name: str) -> int:
    descriptor = os.open(name, _OPEN_FILE_FLAGS, dir_fd=directory_fd)
    opened = os.fstat(descriptor)
    current = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
    if not _same_entry(opened, current) or not _owner_controlled(
        opened, stat.S_IFREG
    ):
        os.close(descriptor)
        raise OSError(errno.EPERM, "untrusted file")
    return descriptor


def _read_exact_state(directory_fd: int, name: str) -> bytes:
    descriptor = _open_verified_file(directory_fd, name)
    try:
        before = os.fstat(descriptor)
        if before.st_size > _MAX_STATE_BYTES:
            raise OSError(errno.EFBIG, "deployment state is too large")
        data = bytearray()
        while len(data) <= _MAX_STATE_BYTES:
            chunk = os.read(descriptor, _MAX_STATE_BYTES + 1 - len(data))
            if not chunk:
                break
            data.extend(chunk)
        after = os.fstat(descriptor)
        current = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
        if (
            len(data) != before.st_size
            or before.st_size != after.st_size
            or before.st_mtime_ns != after.st_mtime_ns
            or not _same_entry(after, current)
        ):
            raise OSError(errno.EAGAIN, "deployment state changed during validation")
        return bytes(data)
    finally:
        os.close(descriptor)


def _lock_is_busy(directory_fd: int, name: str) -> bool:
    descriptor = _open_verified_file(directory_fd, name)
    try:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            return error.errno in {errno.EACCES, errno.EAGAIN}
        else:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
            return False
    finally:
        os.close(descriptor)


def _start_is_allowed(environment: str, expected_release_sha: str) -> bool:
    if (
        environment not in _ALLOWED_ENVIRONMENTS
        or _RELEASE_SHA_RE.fullmatch(expected_release_sha) is None
    ):
        return False

    helper_path = Path(__file__)
    if not helper_path.is_absolute() or helper_path.parent.name != "scripts":
        return False
    runtime_root = helper_path.parent.parent

    root_fd = -1
    deploy_fd = -1
    try:
        root_fd = _open_runtime_root(runtime_root)
        try:
            deploy_fd = os.open(
                ".runtime-deploy",
                _OPEN_DIRECTORY_FLAGS,
                dir_fd=root_fd,
            )
        except FileNotFoundError:
            return True

        opened_deploy_dir = os.fstat(deploy_fd)
        current_deploy_dir = os.stat(
            ".runtime-deploy",
            dir_fd=root_fd,
            follow_symlinks=False,
        )
        if not _same_entry(
            opened_deploy_dir, current_deploy_dir
        ) or not _owner_controlled(opened_deploy_dir, stat.S_IFDIR):
            return False

        state_name = f"{environment}.state"
        try:
            state = _read_exact_state(deploy_fd, state_name)
        except FileNotFoundError:
            return True

        expected_state = (
            "DEPLOYMENT_STATE=restart-authorized\n"
            f"ENVIRONMENT={environment}\n"
            f"RELEASE_SHA={expected_release_sha}\n"
        ).encode("ascii")
        if state != expected_state:
            return False

        return _lock_is_busy(
            deploy_fd, f"{environment}.lock"
        ) and _lock_is_busy(
            deploy_fd, f"{environment}.restart.lock"
        )
    except (OSError, UnicodeError, ValueError):
        return False
    finally:
        if deploy_fd >= 0:
            os.close(deploy_fd)
        if root_fd >= 0:
            os.close(root_fd)


def main(arguments: list[str]) -> int:
    allowed = (
        len(arguments) == 5
        and arguments[1] == "--environment"
        and arguments[3] == "--expected-release-sha"
        and _start_is_allowed(arguments[2], arguments[4])
    )
    if allowed:
        return 0
    sys.stderr.write("Backend start denied by deployment guard.\n")
    return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
