"""Retention prune and systemd artifact tests (Product Analytics plan §2.4).

The prune keeps exactly 800 UTC calendar dates of login-day facts (today plus
the previous 799) and never touches the status-transition history. The
systemd artifacts are asserted verbatim because deployment installs them
byte-for-byte.
"""

from __future__ import annotations

import importlib.util
import sys
from datetime import UTC, date, datetime, timedelta
from pathlib import Path
from types import SimpleNamespace
from uuid import uuid4

import pytest
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.models.product_analytics import UserLoginDay, UserStatusTransition
from app.models.user import User, UserRole, UserStatus

_BACKEND_ROOT = Path(__file__).resolve().parents[2]
_SYSTEMD_DIR = _BACKEND_ROOT / "deploy" / "systemd"
# Bare unit name serves prod, matching verdaxis-backend.service convention.
_PRUNE_ENVIRONMENTS = {
    "production": (
        "verdaxis-product-analytics-prune",
        "/home/verdaxis-prod/verdaxis/prod/be",
    ),
    "staging": (
        "verdaxis-product-analytics-prune-staging",
        "/home/verdaxis-prod/verdaxis/staging/be",
    ),
}
_RELEASE_SHA = "a" * 40


def _load_prune_module():
    name = "prune_product_analytics"
    if name in sys.modules:
        return sys.modules[name]
    spec = importlib.util.spec_from_file_location(
        name, _BACKEND_ROOT / "scripts" / "prune_product_analytics.py"
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture
async def prune_db():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    tables = [User.__table__, UserLoginDay.__table__, UserStatusTransition.__table__]
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        yield session
    await engine.dispose()


def _login_day(user_id, day: date) -> UserLoginDay:
    instant = datetime(day.year, day.month, day.day, 8, tzinfo=UTC)
    return UserLoginDay(
        id=uuid4(),
        activity_date=day,
        user_id=user_id,
        organization_id=None,
        role=UserRole.BUYER,
        login_count=1,
        first_login_at=instant,
        last_login_at=instant,
    )


async def _seed_user(session) -> User:
    user = User(
        id=uuid4(),
        email=f"{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
    )
    session.add(user)
    await session.commit()
    return user


def test_cutoff_retains_exactly_800_dates():
    prune = _load_prune_module()
    today = date(2026, 7, 15)
    cutoff = prune.compute_cutoff(today)
    # today plus the previous 799 dates remain: [today-799, today].
    assert cutoff == today - timedelta(days=799)
    assert (today - cutoff).days + 1 == prune.RETAINED_DATES


async def test_prune_deletes_only_rows_past_the_boundary_and_is_idempotent(prune_db):
    prune = _load_prune_module()
    user = await _seed_user(prune_db)
    today = date(2026, 7, 15)
    cutoff = prune.compute_cutoff(today)
    prune_db.add_all(
        [
            _login_day(user.id, cutoff - timedelta(days=1)),  # deleted
            _login_day(user.id, cutoff),                      # oldest retained date
            _login_day(user.id, today),                       # retained
        ]
    )
    await prune_db.commit()

    deleted = await prune.prune_login_days(prune_db, today=today)
    assert deleted == 1
    remaining = (
        (await prune_db.execute(select(UserLoginDay.activity_date).order_by(UserLoginDay.activity_date)))
        .scalars()
        .all()
    )
    assert [str(day) for day in remaining] == [str(cutoff), str(today)]

    # Idempotent: a rerun deletes nothing further.
    assert await prune.prune_login_days(prune_db, today=today) == 0


async def test_status_transitions_are_never_pruned(prune_db):
    prune = _load_prune_module()
    user = await _seed_user(prune_db)
    prune_db.add(
        UserStatusTransition(
            id=uuid4(),
            user_id=user.id,
            organization_id=None,
            role=UserRole.BUYER,
            from_status=None,
            to_status=UserStatus.PENDING,
            effective_at=datetime(2020, 1, 1, tzinfo=UTC),  # far past retention
            provenance="workflow",
        )
    )
    await prune_db.commit()

    await prune.prune_login_days(prune_db, today=date(2026, 7, 15))
    count = await prune_db.scalar(select(func.count(UserStatusTransition.id)))
    assert count == 1


@pytest.mark.parametrize(
    "argv",
    [
        [],
        ["--environment", "development", "--release-sha", _RELEASE_SHA],
        ["--environment", "production", "--release-sha", "development"],
    ],
)
def test_destructive_prune_cli_requires_explicit_deployed_identity(argv):
    prune = _load_prune_module()

    with pytest.raises(SystemExit):
        prune.parse_cli_args(argv)


@pytest.mark.parametrize(
    "configured_environment,configured_release_sha,error",
    [
        ("development", "development", "environment"),
        ("staging", _RELEASE_SHA, "environment"),
        ("production", "b" * 40, "release"),
    ],
)
async def test_prune_refuses_mismatched_runtime_config_before_database_access(
    monkeypatch, configured_environment, configured_release_sha, error
):
    prune = _load_prune_module()
    import app.config as runtime_config

    def database_access_would_be_unsafe(*args, **kwargs):
        raise AssertionError("database engine opened before runtime identity validation")

    monkeypatch.setattr(prune, "create_async_engine", database_access_would_be_unsafe)
    monkeypatch.setattr(
        runtime_config,
        "settings",
        SimpleNamespace(
            ENVIRONMENT=configured_environment,
            RELEASE_SHA=configured_release_sha,
            DATABASE_URL="database access must remain unreachable",
        ),
    )
    with pytest.raises(RuntimeError, match=error):
        await prune.main(
            expected_environment="production",
            expected_release_sha=_RELEASE_SHA,
        )


@pytest.mark.parametrize("environment", sorted(_PRUNE_ENVIRONMENTS))
def test_systemd_service_artifact_matches_the_specified_unit(environment):
    unit_name, backend_dir = _PRUNE_ENVIRONMENTS[environment]
    content = (_SYSTEMD_DIR / f"{unit_name}.service").read_text()
    for directive in (
        "Wants=network-online.target",
        "Requires=postgresql.service",
        "After=network-online.target postgresql.service",
        f"RequiresMountsFor={backend_dir}",
        "StartLimitIntervalSec=15min",
        "StartLimitBurst=5",
        "Type=oneshot",
        "User=verdaxis-prod",
        "Group=verdaxis-prod",
        f"WorkingDirectory={backend_dir}",
        f"EnvironmentFile={backend_dir}/.env",
        f"EnvironmentFile={backend_dir}/.runtime-release.env",
        f"ExecStartPre=/usr/bin/test -r {backend_dir}/.env",
        f"ExecStartPre=/usr/bin/test -r {backend_dir}/.runtime-release.env",
        f"ExecStartPre=/usr/bin/test ! -e {backend_dir}/.runtime-deploying",
        "ExecStartPre=/usr/bin/pg_isready --quiet --timeout=5",
        (
            f"ExecStart={backend_dir}/venv/bin/python "
            "scripts/prune_product_analytics.py "
            f"--environment {environment} --release-sha ${{RELEASE_SHA}}"
        ),
        "Nice=10",
        "IOSchedulingClass=idle",
        "NoNewPrivileges=true",
        "PrivateTmp=true",
        "PrivateDevices=true",
        "ProtectSystem=strict",
        "ProtectHome=read-only",
        "RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6",
        "RestrictSUIDSGID=true",
        "LockPersonality=true",
        "CapabilityBoundingSet=",
        "AmbientCapabilities=",
        "SystemCallFilter=@system-service",
        "SystemCallErrorNumber=EPERM",
        "Restart=on-failure",
        "RestartSec=30s",
        "TimeoutStartSec=300",
    ):
        assert directive in content, directive
    assert content.count("Restart=") == 1


@pytest.mark.parametrize("environment", sorted(_PRUNE_ENVIRONMENTS))
def test_systemd_timer_artifact_matches_the_specified_schedule(environment):
    unit_name, _ = _PRUNE_ENVIRONMENTS[environment]
    content = (_SYSTEMD_DIR / f"{unit_name}.timer").read_text()
    for directive in (
        f"Unit={unit_name}.service",
        "OnCalendar=*-*-* 03:20:00 Asia/Singapore",
        "Persistent=true",
        "RandomizedDelaySec=15m",
        "WantedBy=timers.target",
    ):
        assert directive in content, directive
