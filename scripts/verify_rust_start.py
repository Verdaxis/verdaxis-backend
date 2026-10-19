#!/usr/bin/env python3
"""Check deployed Rust budget identity without application imports or secrets.

Deployment freezes its migrator-verified revision under the deploy lock.
Deployment owns migration authority and environment-drift preparation.
"""
import argparse
import json
import os
import stat
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--budget-file", required=True)
    parser.add_argument("--expected-release-sha", required=True)
    parser.add_argument("--expected-revision", required=True)
    parser.add_argument("--revision-file", required=True)
    args = parser.parse_args()
    try:
        fd = os.open(args.budget_file, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd) as stream:
            metadata = os.fstat(stream.fileno())
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077:
                raise ValueError("private regular budget required")
            budget = json.load(stream)
        fd = os.open(args.revision_file, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd) as stream:
            metadata = os.fstat(stream.fileno())
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077:
                raise ValueError("private regular revision required")
            revision = json.load(stream)
        if revision != {"release_sha": args.expected_release_sha, "revision": args.expected_revision}:
            raise ValueError("revision identity differs")
        if budget["release_sha"] != args.expected_release_sha or budget["version"] != 1:
            raise ValueError("budget identity differs")
    except (OSError, ValueError, KeyError):
        print("Rust startup budget refused", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
