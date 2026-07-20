#!/usr/bin/env python3
"""Validate a deployment readiness response without substring matching."""

from __future__ import annotations

import argparse
import json
import sys


class HealthResponseError(ValueError):
    """The readiness payload does not identify the expected release."""


def validate_health_response(
    payload: str,
    *,
    expected_environment: str,
    expected_release_sha: str,
) -> None:
    try:
        document = json.loads(payload)
    except (TypeError, json.JSONDecodeError) as exc:
        raise HealthResponseError("readiness response is not valid JSON") from exc
    if not isinstance(document, dict):
        raise HealthResponseError("readiness response must be a JSON object")
    expected = {
        "status": "ok",
        "db": "ok",
        "environment": expected_environment,
        "release_sha": expected_release_sha,
    }
    for field, value in expected.items():
        if document.get(field) != value:
            raise HealthResponseError(f"readiness response has unexpected {field}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--environment", required=True)
    parser.add_argument("--release-sha", required=True)
    args = parser.parse_args()
    try:
        validate_health_response(
            sys.stdin.read(),
            expected_environment=args.environment,
            expected_release_sha=args.release_sha,
        )
    except HealthResponseError as exc:
        print(str(exc), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
