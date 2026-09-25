"""PostgreSQL proof for batched browsing deduplication and tenant isolation."""
import os
from datetime import UTC, datetime, timedelta
from uuid import uuid4

from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.models.user import User, UserRole, UserStatus
from app.schemas.user_activity import BrowsingEventsIn
from app.services.user_activity import get_user_activity_page, ingest_browsing_events
from scripts.prune_product_analytics import prune_browsing_events


async def test_app_role_deduplicates_per_user_and_prunes_only_expired_browsing(pg_session):
    _, migrator = pg_session
    first_id, second_id, shared_event_id = uuid4(), uuid4(), uuid4()
    for user_id in (first_id, second_id):
        migrator.add(User(
            id=user_id, email=f"{user_id}@example.test", password_hash="test-only",
            role=UserRole.BUYER, status=UserStatus.APPROVED,
        ))
    await migrator.commit()

    def batch(event_id, page):
        return BrowsingEventsIn.model_validate({
            "consent_version": 2,
            "events": [{"id": str(event_id), "action": "page_view", "page": page}],
        })

    now = datetime.now(UTC)
    engine = create_async_engine(os.environ["DATABASE_URL"], hide_parameters=True)
    try:
        async with async_sessionmaker(engine, expire_on_commit=False)() as app:
            assert await ingest_browsing_events(
                app, user_id=first_id, payload=batch(shared_event_id, "marketplace"),
                received_at=now,
            ) == 1
            assert await ingest_browsing_events(
                app, user_id=first_id, payload=batch(shared_event_id, "marketplace"),
                received_at=now,
            ) == 0
            assert await ingest_browsing_events(
                app, user_id=second_id, payload=batch(shared_event_id, "trades"),
                received_at=now,
            ) == 1
            first = await get_user_activity_page(
                app, user_id=first_id, days=90, kind="browsing",
                limit=50, offset=0, now=now + timedelta(seconds=1),
            )
            second = await get_user_activity_page(
                app, user_id=second_id, days=90, kind="browsing",
                limit=50, offset=0, now=now + timedelta(seconds=1),
            )
            assert [item.details["page"] for item in first.items] == ["marketplace"]
            assert [item.details["page"] for item in second.items] == ["trades"]

            await ingest_browsing_events(
                app, user_id=first_id, payload=batch(uuid4(), "map"),
                received_at=now - timedelta(days=91),
            )
            assert await prune_browsing_events(app, now=now) == 1
            assert await prune_browsing_events(app, now=now) == 0
            retained = await get_user_activity_page(
                app, user_id=first_id, days=90, kind="browsing",
                limit=50, offset=0, now=now + timedelta(seconds=1),
            )
            assert [item.details["page"] for item in retained.items] == ["marketplace"]
    finally:
        await engine.dispose()
