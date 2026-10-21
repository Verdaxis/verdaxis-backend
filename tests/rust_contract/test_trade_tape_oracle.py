"""Canonical page membership is selected from all Python-filtered rows."""

from datetime import UTC, datetime
from decimal import Decimal
from types import SimpleNamespace
from uuid import UUID

import pytest
from scripts.rust_contract.trade_tape_oracle import canonical_tape_page

CLOCK = datetime(2026, 10, 15, 12, tzinfo=UTC)


def row(index):
    return SimpleNamespace(id=UUID(f"12345678-0000-0000-0000-{index:012x}"), confirmed_at=CLOCK,
                           quantity_mt=Decimal(index + 1), price_per_mt_usd=Decimal(750))


def projection(trade, **_options):
    return {"id": trade.id.hex[:8], "quantity_mt": str(trade.quantity_mt), "price_per_mt_usd": str(trade.price_per_mt_usd)}


def test_trade_tape_ties_preserve_page_membership():
    rows = [row(index) for index in range(405)]
    pages = []
    for skip in (0, 200, 400):
        body, proof = canonical_tape_page(rows, {}, CLOCK, skip, 200, projector=projection, snapshot_identity="immutable-seed")
        assert body["total"] == 405
        assert proof["full_ids"] == [str(entry.id) for entry in reversed(rows)][skip:skip + 200]
        pages.extend(proof["full_ids"])
        again, _ = canonical_tape_page(list(reversed(rows)), {}, CLOCK, skip, 200, projector=projection, snapshot_identity="immutable-seed")
        assert again == body
    assert len(pages) == len(set(pages)) == 405


def test_duplicate_source_rows_are_invalid_evidence():
    with pytest.raises(ValueError):
        canonical_tape_page([row(1), row(1)], {}, CLOCK, 0, 50, projector=projection, snapshot_identity="seed")


def test_snapshot_identity_and_valid_page_bounds_are_required():
    for skip, limit, identity in ((-1, 50, "seed"), (0, 201, "seed"), (0, 50, "")):
        with pytest.raises(ValueError):
            canonical_tape_page([row(1)], {}, CLOCK, skip, limit, projector=projection, snapshot_identity=identity)
