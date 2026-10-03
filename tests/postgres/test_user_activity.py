"""PostgreSQL proof for batched browsing deduplication and tenant isolation."""
import os
from datetime import UTC, datetime, timedelta
from uuid import uuid4

from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.models.user import User, UserRole, UserStatus
from app.schemas.user_activity import BrowsingEventsIn
from app.services.user_activity import get_user_activity_page, ingest_browsing_events
from scripts.prune_product_analytics import (
    prune_activity_delivery_reports,
    prune_browsing_events,
)


async def test_app_role_deduplicates_per_user_and_prunes_only_expired_browsing(pg_session):
    _, migrator = pg_session
    first_id, second_id, shared_event_id = uuid4(), uuid4(), uuid4()
    for user_id in (first_id, second_id):
        migrator.add(User(
            id=user_id, email=f"{user_id}@example.test", password_hash="test-only",
            role=UserRole.BUYER, status=UserStatus.APPROVED,
        ))
    await migrator.commit()

    def batch(event_id, page, *, report_id=None, dropped=0, rejected=0):
        payload = {
            "events": [{"id": str(event_id), "action": "page_view", "page": page}],
        }
        if report_id is not None:
            payload["delivery_loss"] = {
                "report_id": str(report_id),
                "dropped_events": dropped,
                "rejected_events": rejected,
            }
        return BrowsingEventsIn.model_validate(payload)

    now = datetime.now(UTC)
    shared_report_id = uuid4()
    engine = create_async_engine(os.environ["DATABASE_URL"], hide_parameters=True)
    try:
        async with async_sessionmaker(engine, expire_on_commit=False)() as app:
            assert await ingest_browsing_events(
                app, user_id=first_id, payload=batch(shared_event_id, "marketplace"),
                received_at=now,
            ) == 1
            assert await ingest_browsing_events(
                app,
                user_id=first_id,
                payload=batch(
                    shared_event_id,
                    "marketplace",
                    report_id=shared_report_id,
                    dropped=2,
                    rejected=1,
                ),
                received_at=now,
            ) == 0
            assert await ingest_browsing_events(
                app,
                user_id=first_id,
                payload=batch(
                    shared_event_id,
                    "marketplace",
                    report_id=shared_report_id,
                    dropped=2,
                    rejected=1,
                ),
                received_at=now,
            ) == 0
            assert await ingest_browsing_events(
                app,
                user_id=second_id,
                payload=batch(
                    shared_event_id,
                    "trades",
                    report_id=shared_report_id,
                    dropped=5,
                    rejected=0,
                ),
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
            assert first.browser_reported_delivery_loss.reports_received == 1
            assert first.browser_reported_delivery_loss.dropped_events == 2
            assert first.browser_reported_delivery_loss.rejected_events == 1
            assert second.browser_reported_delivery_loss.reports_received == 1
            assert second.browser_reported_delivery_loss.dropped_events == 5

            await ingest_browsing_events(
                app,
                user_id=first_id,
                payload=batch(
                    uuid4(),
                    "map",
                    report_id=uuid4(),
                    dropped=1,
                    rejected=0,
                ),
                received_at=now - timedelta(days=91),
            )
            assert await prune_browsing_events(app, now=now) == 1
            assert await prune_browsing_events(app, now=now) == 0
            assert await prune_activity_delivery_reports(app, now=now) == 1
            assert await prune_activity_delivery_reports(app, now=now) == 0
            retained = await get_user_activity_page(
                app, user_id=first_id, days=90, kind="browsing",
                limit=50, offset=0, now=now + timedelta(seconds=1),
            )
            assert [item.details["page"] for item in retained.items] == ["marketplace"]
            assert retained.browser_reported_delivery_loss.reports_received == 1
    finally:
        await engine.dispose()
