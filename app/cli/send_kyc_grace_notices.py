"""Operator-only, at-most-once KYC grace notices. No worker or timer owner."""
import argparse
import asyncio
import re
from datetime import UTC, datetime

from sqlalchemy import select
from sqlalchemy.dialects.postgresql import insert


def parse_cli_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--environment", choices=("staging", "production"), required=True)
    parser.add_argument("--release-sha", required=True)
    args = parser.parse_args(argv)
    if re.fullmatch(r"[0-9a-f]{40}", args.release_sha) is None:
        parser.error("release SHA must be a full lowercase 40-hex commit SHA")
    return args


def assert_runtime_identity(settings, environment: str, release_sha: str) -> None:
    if environment not in {"staging", "production"} or re.fullmatch(r"[0-9a-f]{40}", release_sha) is None:
        raise RuntimeError("An explicit deployed environment and full release SHA are required")
    if settings.ENVIRONMENT != environment or settings.RELEASE_SHA != release_sha:
        raise RuntimeError("Runtime identity does not match the notice target")


async def send_notices(db) -> tuple[int, int]:
    from app.models.kyc_execution_grace import KYCExecutionGrace, KYCGraceNotice
    from app.models.user import User
    from app.services.email import send_kyc_grace_notice

    rows = (await db.execute(select(KYCExecutionGrace.id, KYCExecutionGrace.expires_at, User.email).join(
        User, User.id == KYCExecutionGrace.user_id,
    ).where(~select(KYCGraceNotice.grace_id).where(
        KYCGraceNotice.grace_id == KYCExecutionGrace.id,
    ).exists()).order_by(KYCExecutionGrace.id))).all()
    sent = failed = 0
    for grace_id, expires_at, email in rows:
        claimed = (await db.execute(insert(KYCGraceNotice).values(
            grace_id=grace_id, claimed_at=datetime.now(UTC),
        ).on_conflict_do_nothing().returning(KYCGraceNotice.grace_id))).scalar_one_or_none()
        # Commit before provider I/O. An interrupted or ambiguous attempt must
        # never be sent twice, including beyond the provider's dedupe window.
        await db.commit()
        if claimed is None:
            continue
        if await send_kyc_grace_notice(email, expires_at, grace_id=grace_id):
            notice = await db.get(KYCGraceNotice, grace_id)
            notice.sent_at = datetime.now(UTC)
            await db.commit()
            sent += 1
        else:
            failed += 1
    return sent, failed


async def run_once(environment: str, release_sha: str) -> int:
    from app.config import settings

    assert_runtime_identity(settings, environment, release_sha)
    from app.database import AsyncSessionLocal

    async with AsyncSessionLocal() as db:
        sent, failed = await send_notices(db)
    print(f"KYC grace notices: sent={sent} uncertain_or_failed={failed}")
    return 1 if failed else 0


def main(argv: list[str] | None = None) -> int:
    args = parse_cli_args(argv)
    return asyncio.run(run_once(args.environment, args.release_sha))


if __name__ == "__main__":
    raise SystemExit(main())
