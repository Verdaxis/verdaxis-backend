"""Test-only canonical tape pages from the complete Python eligibility relation.

This module selects no Rust IDs and contains no comparator. Full UUID evidence
is private test evidence; the public projection keeps truncated IDs and redaction.
"""

import hashlib
from datetime import timedelta
from pathlib import Path

from sqlalchemy import or_, select

RULE_VERSION = "decision85-v1"


def canonical_tape_page(rows, filters, clock, skip, limit, *, snapshot_identity, projector=None):
    if not snapshot_identity or clock.tzinfo is None or skip < 0 or not 1 <= limit <= 200:
        raise ValueError("canonical tape requires immutable snapshot, aware clock and valid page bounds")
    if projector is None:
        from app.routers.trade_tape import _build_tape_entry
        projector = _build_tape_entry
    rows = list(rows)
    identities = [str(row.id) for row in rows]
    if len(identities) != len(set(identities)) or any(row.confirmed_at is None for row in rows):
        raise ValueError("duplicate or incomplete canonical source relation")
    ordered = sorted(rows, key=lambda row: (row.confirmed_at, row.id.int), reverse=True)
    selected = ordered[skip:skip + limit]
    items = []
    for row in selected:
        item = projector(row, expose_delivery_point=filters.get("delivery_point_id") is not None)
        items.append(item.model_dump(mode="json") if hasattr(item, "model_dump") else item)
    proof = {"rule_version": RULE_VERSION, "snapshot_identity": snapshot_identity,
             "clock": clock.isoformat(), "filters": filters, "skip": skip, "limit": limit,
             "unpaged_count": len(rows), "source_ids": sorted(identities),
             "full_ids": [str(row.id) for row in selected],
             "oracle_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    return {"items": items, "total": len(rows), "market_hours": True}, proof


async def read_canonical_tape_page(session, filters, clock, skip, limit, *, snapshot_identity):
    """Read all filtered rows in the caller's immutable repeatable-read snapshot."""
    from app.models.orderbook import Trade
    from app.services.availability_windows import normalize_availability_window
    from app.services.market_provenance import (
        MarketEvidenceScope,
        public_trade_evidence_clause,
        trade_evidence_clause,
    )

    predicate = (trade_evidence_clause(Trade, MarketEvidenceScope.DEMO, confirmed_since=clock - timedelta(days=7))
                 if filters.get("demo_only", False) else
                 public_trade_evidence_clause(Trade, confirmed_since=clock - timedelta(days=7)))
    query = select(Trade).where(predicate)
    for field in ("fuel_type", "market_product", "delivery_point_id"):
        if filters.get(field) is not None:
            query = query.where(getattr(Trade, field) == filters[field])
    if filters.get("region") is not None:
        region = filters["region"]
        query = query.where(or_(Trade.delivery_point_region == region, Trade.delivery_point_name == region))
    if filters.get("availability_window") is not None:
        query = query.where(Trade.availability_window == normalize_availability_window(filters["availability_window"]))
    rows = (await session.execute(query)).scalars().all()
    return canonical_tape_page(rows, filters, clock, skip, limit, snapshot_identity=snapshot_identity)
