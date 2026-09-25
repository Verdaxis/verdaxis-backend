"""PostgreSQL schema and least-privilege checks for browsing activity."""

from __future__ import annotations

import os
from uuid import uuid4

import pytest
from sqlalchemy import text
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import create_async_engine

from app.models.user import User, UserRole, UserStatus


async def test_user_activity_indexes_and_composite_identity_exist(pg_session):
    _engine, session = pg_session
    indexes = set(
        (
            await session.execute(
                text(
                    "SELECT indexname FROM pg_indexes WHERE tablename IN "
                    "('user_browsing_events', 'audit_logs', 'user_login_days')"
                )
            )
        ).scalars()
    )
    assert {
        "pk_user_browsing_events",
        "ix_user_browsing_events_user_received",
        "ix_user_browsing_events_received",
        "ix_audit_logs_user_timestamp",
        "ix_user_login_days_user_last_login",
    } <= indexes

    primary_key_columns = (
        await session.execute(
            text(
                "SELECT a.attname FROM pg_index i "
                "JOIN pg_attribute a ON a.attrelid = i.indrelid "
                "AND a.attnum = ANY(i.indkey) "
                "WHERE i.indrelid = 'user_browsing_events'::regclass "
                "AND i.indisprimary ORDER BY array_position(i.indkey, a.attnum)"
            )
        )
    ).scalars().all()
    assert primary_key_columns == ["user_id", "event_id"]


async def test_app_role_can_insert_read_delete_but_cannot_update_browsing(pg_session):
    _engine, session = pg_session
    user = User(
        id=uuid4(),
        email=f"{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
    )
    session.add(user)
    await session.commit()

    event_id = uuid4()
    app_url = os.environ["DATABASE_URL"]
    app_engine = create_async_engine(app_url, hide_parameters=True)
    try:
        async with app_engine.begin() as connection:
            await connection.execute(
                text(
                    "INSERT INTO user_browsing_events "
                    "(user_id, event_id, action, page) "
                    "VALUES (:user_id, :event_id, 'page_view', 'home')"
                ),
                {"user_id": user.id, "event_id": event_id},
            )
        async with app_engine.connect() as connection:
            assert await connection.scalar(
                text(
                    "SELECT consent_version FROM user_browsing_events "
                    "WHERE user_id = :user_id AND event_id = :event_id"
                ),
                {"user_id": user.id, "event_id": event_id},
            ) is None
        with pytest.raises(DBAPIError):
            async with app_engine.begin() as connection:
                await connection.execute(
                    text(
                        "UPDATE user_browsing_events SET page = 'map' "
                        "WHERE user_id = :user_id AND event_id = :event_id"
                    ),
                    {"user_id": user.id, "event_id": event_id},
                )
        async with app_engine.begin() as connection:
            await connection.execute(
                text(
                    "DELETE FROM user_browsing_events "
                    "WHERE user_id = :user_id AND event_id = :event_id"
                ),
                {"user_id": user.id, "event_id": event_id},
            )
    finally:
        await app_engine.dispose()
