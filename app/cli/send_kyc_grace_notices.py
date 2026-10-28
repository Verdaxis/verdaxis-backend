"""Send the one-time KYC grace notice emails. Operator-run; there is no timer.

Usage on a deployed host:
    python -m app.cli.send_kyc_grace_notices --environment production --release-sha <sha>

The command is idempotent: each grace row gets at most one recorded email,
so a repeated run sends only the notices that are still unsent.
"""

from __future__ import annotations

import argparse
import asyncio
import re

from app.config import settings
from app.database import AsyncSessionLocal, verify_database_runtime
from app.services.kyc_execution import kyc_execution_gate_enabled
from app.services.kyc_grace_notices import KycGraceNoticeInProgress, send_kyc_grace_notices

_DEPLOYED_ENVIRONMENTS = ("production", "staging")
_FULL_RELEASE_SHA = re.compile(r"[0-9a-f]{40}")
_GATE_DISABLED_MESSAGE = (
    "kyc grace notices refused: the KYC execution gate is disabled"
)


class KycGraceNoticeGateDisabled(RuntimeError):
    """The gate is off, so sent notices would be misleading."""


def _release_sha(value: str) -> str:
    if _FULL_RELEASE_SHA.fullmatch(value) is None:
        raise argparse.ArgumentTypeError(
            "release SHA must be a full lowercase 40-hex commit SHA"
        )
    return value


def parse_cli_args(argv: list[str] | None = None) -> argparse.Namespace:
    """Require an explicit deployed identity; there is no development default."""
    parser = argparse.ArgumentParser(description="Send the one-time KYC grace notices")
    parser.add_argument("--environment", choices=_DEPLOYED_ENVIRONMENTS, required=True)
    parser.add_argument("--release-sha", type=_release_sha, required=True)
    return parser.parse_args(argv)


def assert_runtime_identity(
    *,
    configured_environment: str,
    configured_release_sha: str,
    expected_environment: str,
    expected_release_sha: str,
) -> None:
    """Bind the run to the validated deployed runtime configuration."""
    if configured_environment != expected_environment:
        raise RuntimeError("runtime environment does not match the notice target")
    if configured_release_sha != expected_release_sha:
        raise RuntimeError("runtime release does not match the notice target")


async def run_once(*, expected_environment: str, expected_release_sha: str) -> dict[str, int]:
    assert_runtime_identity(
        configured_environment=settings.ENVIRONMENT,
        configured_release_sha=settings.RELEASE_SHA,
        expected_environment=expected_environment,
        expected_release_sha=expected_release_sha,
    )
    if not kyc_execution_gate_enabled():
        # With the gate off no user depends on grace, so a notice would be
        # wrong. Refuse before contacting the database.
        raise KycGraceNoticeGateDisabled(_GATE_DISABLED_MESSAGE)
    await verify_database_runtime()
    async with AsyncSessionLocal() as db:
        try:
            return await send_kyc_grace_notices(db)
        except KycGraceNoticeInProgress:
            await db.rollback()
            raise


def main(argv: list[str] | None = None) -> int:
    args = parse_cli_args(argv)
    try:
        report = asyncio.run(
            run_once(
                expected_environment=args.environment,
                expected_release_sha=args.release_sha,
            )
        )
    except KycGraceNoticeInProgress:
        print("kyc grace notices stopped: another run is active")
        return 0
    except KycGraceNoticeGateDisabled as refusal:
        print(str(refusal))
        return 2
    print(
        "kyc grace notices complete: "
        + " ".join(f"{key}={value}" for key, value in sorted(report.items()))
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
