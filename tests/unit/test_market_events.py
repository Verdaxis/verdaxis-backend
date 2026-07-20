from pathlib import Path
from unittest.mock import AsyncMock, MagicMock
from uuid import uuid4

import pytest

from app.models.market_event import MarketEventOutbox
from app.services import market_events


def _event(*participants):
    return market_events.participant_market_event(
        event_type="trade_created",
        aggregate_type="trade",
        aggregate_id=uuid4(),
        participant_org_ids=participants,
        payload={"status": "PENDING_CONFIRMATION"},
    )


@pytest.mark.asyncio
async def test_market_event_is_durable_and_participant_scoped():
    db = AsyncMock()
    db.add = MagicMock()
    buyer_id, seller_id = uuid4(), uuid4()

    await market_events.enqueue_market_events(db, [_event(seller_id, buyer_id)])

    row = db.add.call_args.args[0]
    assert isinstance(row, MarketEventOutbox)
    assert row.participant_org_ids == sorted(
        [str(buyer_id), str(seller_id)]
    )
    assert row.event_type == "trade_created"
    db.flush.assert_awaited_once()


def test_market_event_rejects_global_or_empty_audience():
    with pytest.raises(ValueError, match="participant"):
        _event()


@pytest.mark.asyncio
async def test_failed_commit_leaves_no_process_local_publish_boundary():
    db = AsyncMock()
    db.add = MagicMock()
    db.commit.side_effect = RuntimeError("commit failed")

    with pytest.raises(RuntimeError, match="commit failed"):
        await market_events.commit_market_events(db, [_event(uuid4())])

    db.flush.assert_awaited_once()
    assert not hasattr(market_events, "event_bus")


def test_update_order_queues_outbox_before_commit():
    source = Path("app/routers/orderbook.py").read_text()
    update_source = source.split("async def update_order", 1)[1].split(
        "async def cancel_order", 1
    )[0]
    assert "event_bus.publish" not in update_source
    assert "commit_market_events(db, committed_events)" in update_source
