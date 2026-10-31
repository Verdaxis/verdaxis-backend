"""Run Rust stream admission against the disposable PostgreSQL fixture.

The Python side seeds account and organization rows; the Rust admission code
re-reads them with the deployed application role. The Rust test also checks
the database bounds of that read; it holds a table lock through the fixture's
migrator role for that. The Rust test ``postgres_admission`` is ignored in
ordinary builds, so this suite runs it explicitly and fails unless it passed
and reported its coverage lines.
"""

import os
import subprocess
import sys
from pathlib import Path
from uuid import uuid4

import pytest
from sqlalchemy import text

ROOT = Path(__file__).resolve().parents[2]
TARGET = "streams_auth"
TEST = "postgres_admission"

# A fixed issuance instant so cutoff rows can be seeded at exact microseconds.
ISSUED_AT_US = 1_792_497_600_123_456


def _timestamp(microseconds: int | None) -> str:
    if microseconds is None:
        return "NULL"
    seconds, micros = divmod(microseconds, 1_000_000)
    return f"to_timestamp({seconds}) + interval '{micros} microseconds'"


def _sqlx_url(url: str) -> str:
    assert url.startswith("postgresql+asyncpg://"), "the harness supplies asyncpg URLs"
    # SQLx takes the plain libpq scheme.
    return url.replace("postgresql+asyncpg://", "postgresql://", 1)


@pytest.mark.asyncio
async def test_stream_admission_reads_current_account_and_organization(
    pg_session, analytics_pg_url
):
    _, session = pg_session
    approved_org, other_org, pending_org = uuid4(), uuid4(), uuid4()
    await session.execute(
        text(
            "INSERT INTO organizations (id,name,type,verification_status) VALUES "
            "(:approved,'Stream admission approved','FUEL_BUYER','APPROVED'),"
            "(:other,'Stream admission second approved','FUEL_BUYER','APPROVED'),"
            "(:pending,'Stream admission pending','FUEL_BUYER','PENDING')"
        ),
        {"approved": approved_org, "other": other_org, "pending": pending_org},
    )
    # name: (organization, status, must change password, KYC status,
    #        password changed at, authentication revoked at)
    users = {
        "APPROVED": (approved_org, "APPROVED", False, "PENDING", None, None),
        "KYC_REJECTED": (approved_org, "APPROVED", False, "REJECTED", None, None),
        "REJECTED": (approved_org, "REJECTED", False, "PENDING", None, None),
        "PENDING": (approved_org, "PENDING", False, "PENDING", None, None),
        "MUST_CHANGE": (approved_org, "APPROVED", True, "PENDING", None, None),
        # Members of a second approved organization. The Rust test admits
        # OTHER_ORG with a token for that organization and refuses MOVED with
        # a token for approved_org, so only the membership rule refuses MOVED.
        "OTHER_ORG": (other_org, "APPROVED", False, "PENDING", None, None),
        "MOVED": (other_org, "APPROVED", False, "PENDING", None, None),
        "NO_ORGANIZATION": (None, "APPROVED", False, "PENDING", None, None),
        "CUTOFF_EQUAL": (approved_org, "APPROVED", False, "PENDING", ISSUED_AT_US, None),
        "CUTOFF_BEFORE": (
            approved_org,
            "APPROVED",
            False,
            "PENDING",
            ISSUED_AT_US - 1,
            None,
        ),
        "REVOKED": (approved_org, "APPROVED", False, "PENDING", None, ISSUED_AT_US),
        "UNAPPROVED_ORG": (pending_org, "APPROVED", False, "PENDING", None, None),
    }
    env = {
        **os.environ,
        "RELEASE_SHA": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
        "RUST_TEST_PYTHON": sys.executable,
        "SQLX_OFFLINE": "true",
        "STREAM_AUTH_TEST_DATABASE_URL": _sqlx_url(os.environ["DATABASE_URL"]),
        "STREAM_AUTH_TEST_LOCK_DATABASE_URL": _sqlx_url(analytics_pg_url),
        "STREAM_AUTH_TEST_ISSUED_AT_US": str(ISSUED_AT_US),
        "STREAM_AUTH_TEST_APPROVED_ORG": str(approved_org),
        "STREAM_AUTH_TEST_OTHER_ORG": str(other_org),
        "STREAM_AUTH_TEST_PENDING_ORG": str(pending_org),
        "STREAM_AUTH_TEST_MISSING_USER": str(uuid4()),
    }
    for name, (organization, status, must_change, kyc, changed, revoked) in users.items():
        user = uuid4()
        env[f"STREAM_AUTH_TEST_{name}_USER"] = str(user)
        await session.execute(
            text(
                "INSERT INTO users (id,email,password_hash,role,status,organization_id,"
                "email_verified,must_change_password,kyc_status,password_changed_at,"
                "authentication_revoked_at) VALUES "
                "(:id,:email,'unused','BUYER',:status,:organization,true,:must_change,"
                f":kyc,{_timestamp(changed)},{_timestamp(revoked)})"
            ),
            {
                "id": user,
                "email": f"stream-admission-{user}@example.test",
                "status": status,
                "organization": organization,
                "must_change": must_change,
                "kyc": kyc,
            },
        )
    await session.commit()

    result = subprocess.run(
        [
            "cargo",
            "test",
            "--manifest-path",
            "rs/Cargo.toml",
            "--locked",
            "-p",
            "verdaxis-read",
            "--features",
            "contract-tests",
            "--test",
            TARGET,
            "--",
            "--ignored",
            "--exact",
            TEST,
            "--nocapture",
        ],
        cwd=ROOT,
        env=env,
        text=True,
        capture_output=True,
        timeout=1800,
        check=False,
    )
    output = result.stdout + result.stderr
    assert result.returncode == 0, output
    assert f"test {TEST} ... ok" in result.stdout, output
    assert "test result: ok. 1 passed; 0 failed; 0 ignored" in result.stdout, output
    # Four admitted, nine refused and one unavailable store read.
    assert "stream admission postgres coverage: 14 checks" in result.stdout, output
    # Transaction-local bounds, the lock bound, recovery after it, the
    # acquisition deadline and two pooled reads that return their connection.
    assert "stream read bounds postgres coverage: 5 checks" in result.stdout, output
