"""Explicit, attested remediation for known synthetic market rows.

Nothing in this module is called by Alembic or application request paths.
Every mutation is exact-ID scoped, dry-run capable, and writes an audit copy
before deleting an active market row.
"""

from __future__ import annotations

import json
import re
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from hashlib import sha256
from typing import Any, Iterable, Literal
from uuid import UUID, uuid4

from sqlalchemy import JSON, bindparam, text
from sqlalchemy.ext.asyncio import AsyncConnection

from app.demo_identities import (
    CANONICAL_DEMO_ORG_NAMES,
    DEMO_MARKET_ORG_IDS,
    KNOWN_TEST_ORG_IDS,
)
from app.services.market_locks import market_slice_lock_key


MAX_QUARANTINE_IDS = 500
MAX_ACCEPTED_RFQ_IDS = 20
KNOWN_ZERO_SENTINEL_ID = UUID("00000000-dead-beef-0000-aaa0e15eed01")


@dataclass(frozen=True)
class OperatorContext:
    environment: str
    database_name: str
    operator: str
    reason: str
    reference: str


@dataclass(frozen=True)
class OrderQuarantineReport:
    order_id: UUID
    found: bool
    already_quarantined: bool
    original_row: dict[str, Any] | None
    dependencies: dict[str, list[dict[str, Any]]]
    external_order_ids: tuple[UUID, ...]
    inventory_item_id: UUID | None

    def as_json(self) -> dict[str, Any]:
        value = asdict(self)
        value["order_id"] = str(self.order_id)
        value["external_order_ids"] = [str(item) for item in self.external_order_ids]
        value["inventory_item_id"] = (
            str(self.inventory_item_id) if self.inventory_item_id else None
        )
        return value


@dataclass(frozen=True)
class DiscoveryReport:
    scope: str
    matching_count: int
    order_ids: tuple[UUID, ...]
    truncated: bool

    def as_json(self) -> dict[str, Any]:
        return {
            "scope": self.scope,
            "matching_count": self.matching_count,
            "order_ids": [str(order_id) for order_id in self.order_ids],
            "truncated": self.truncated,
        }


@dataclass(frozen=True)
class AcceptedRFQQuarantineReport:
    rfq_id: UUID
    found: bool
    already_quarantined: bool
    original_row: dict[str, Any] | None
    quote_rows: tuple[dict[str, Any], ...]
    accepted_quote_id: UUID | None
    trade_candidates: tuple[dict[str, Any], ...]
    selected_trade_id: UUID | None
    commission_rows: tuple[dict[str, Any], ...]
    blocking_dependencies: dict[str, list[dict[str, Any]]]

    def as_json(self) -> dict[str, Any]:
        return {
            "rfq_id": str(self.rfq_id),
            "found": self.found,
            "already_quarantined": self.already_quarantined,
            "original_row": self.original_row,
            "quote_rows": list(self.quote_rows),
            "accepted_quote_id": (
                str(self.accepted_quote_id) if self.accepted_quote_id else None
            ),
            "trade_candidates": list(self.trade_candidates),
            "selected_trade_id": (
                str(self.selected_trade_id) if self.selected_trade_id else None
            ),
            "commission_rows": list(self.commission_rows),
            "blocking_dependencies": self.blocking_dependencies,
        }


@dataclass(frozen=True)
class OrganizationMarketApprovalReport:
    organization_id: UUID
    previous_verification_status: str
    eligible_user_ids: tuple[UUID, ...]
    provenance: str | None
    already_approved: bool
    snapshot_sha256: str
    reviewed_snapshot: dict[str, Any]

    def as_json(self) -> dict[str, Any]:
        return {
            "organization_id": str(self.organization_id),
            "previous_verification_status": self.previous_verification_status,
            "eligible_trader_count": len(self.eligible_user_ids),
            "eligible_user_ids": [str(value) for value in self.eligible_user_ids],
            "provenance": self.provenance,
            "already_approved": self.already_approved,
            "snapshot_sha256": self.snapshot_sha256,
            "reviewed_snapshot": self.reviewed_snapshot,
        }


def validate_write_authorization(
    context: OperatorContext,
    *,
    apply: bool,
    production_approval_reference: str | None,
) -> None:
    if not apply:
        return
    for field_name in ("operator", "reason", "reference"):
        if not getattr(context, field_name).strip():
            raise ValueError(f"{field_name} is required for an applied remediation")
    if context.environment == "production" and (
        not production_approval_reference
        or production_approval_reference != context.reference
    ):
        raise ValueError(
            "production remediation requires an explicit product approval "
            "reference identical to --reference"
        )


def validate_order_ids(values: Iterable[str | UUID]) -> tuple[UUID, ...]:
    unique: dict[UUID, None] = {}
    for value in values:
        unique[UUID(str(value))] = None
    if not unique:
        raise ValueError("at least one explicit order ID is required")
    if len(unique) > MAX_QUARANTINE_IDS:
        raise ValueError(
            f"at most {MAX_QUARANTINE_IDS} explicit order IDs may be remediated"
        )
    return tuple(sorted(unique, key=str))


def validate_rfq_ids(values: Iterable[str | UUID]) -> tuple[UUID, ...]:
    ids = tuple(sorted({UUID(str(value)) for value in values}, key=str))
    if not ids:
        raise ValueError("at least one explicit accepted RFQ ID is required")
    if len(ids) > MAX_ACCEPTED_RFQ_IDS:
        raise ValueError(
            f"at most {MAX_ACCEPTED_RFQ_IDS} accepted RFQ IDs may be quarantined"
        )
    return ids


async def _acquire_quarantine_locks(
    connection: AsyncConnection,
    *,
    operation: str,
    source_ids: Iterable[UUID],
    slices: Iterable[tuple[UUID, UUID | None, str]],
) -> None:
    """Acquire maintenance idempotency then canonical sorted slice locks."""
    exact_ids = tuple(sorted(set(source_ids), key=str))
    operation_key = int.from_bytes(
        sha256(
            (operation + "|" + "|".join(map(str, exact_ids))).encode("utf-8")
        ).digest()[:8],
        byteorder="big",
        signed=True,
    ) or 1
    await connection.execute(text("SET LOCAL lock_timeout = '500ms'"))
    await connection.execute(text("SET LOCAL statement_timeout = '5s'"))
    await connection.execute(
        text("SELECT pg_advisory_xact_lock(:lock_key)"),
        {"lock_key": operation_key},
    )
    unique_slices = sorted(
        set(slices), key=lambda value: (str(value[0]), str(value[1] or ""), value[2])
    )
    for product_id, delivery_point_id, availability_window in unique_slices:
        await connection.execute(
            text("SELECT pg_advisory_xact_lock(:lock_key)"),
            {
                "lock_key": market_slice_lock_key(
                    side="BID",
                    product_id=product_id,
                    delivery_point_id=delivery_point_id,
                    availability_window=availability_window,
                )
            },
        )


async def connected_database_name(connection: AsyncConnection) -> str:
    return str((await connection.execute(text("SELECT current_database()"))).scalar_one())


async def _has_column(
    connection: AsyncConnection,
    *,
    table_name: str,
    column_name: str,
) -> bool:
    return bool(
        (
            await connection.execute(
                text(
                    "SELECT EXISTS ("
                    "SELECT 1 FROM information_schema.columns "
                    "WHERE table_schema = 'public' AND table_name = :table_name "
                    "AND column_name = :column_name)"
                ),
                {"table_name": table_name, "column_name": column_name},
            )
        ).scalar_one()
    )


async def approve_real_organizations(
    connection: AsyncConnection,
    organization_ids: Iterable[str | UUID],
    *,
    context: OperatorContext,
    apply: bool,
    expected_snapshots: dict[UUID, str] | None = None,
) -> tuple[OrganizationMarketApprovalReport, ...]:
    """Approve exact eligible organizations and record REAL provenance authority."""
    ids = validate_order_ids(organization_ids)
    reserved_ids = set(DEMO_MARKET_ORG_IDS) | set(KNOWN_TEST_ORG_IDS)
    if set(ids) & reserved_ids:
        raise ValueError("demo/test organizations cannot be approved as REAL")
    expected_snapshots = expected_snapshots or {}
    if apply and set(expected_snapshots) != set(ids):
        raise ValueError(
            "apply requires one --expected-snapshot for every organization ID"
        )
    if any(not re.fullmatch(r"[0-9a-f]{64}", value) for value in expected_snapshots.values()):
        raise ValueError("expected snapshot hashes must be lowercase SHA-256 values")

    has_provenance = await _has_column(
        connection,
        table_name="organizations",
        column_name="provenance",
    )
    provenance_projection = "provenance" if has_provenance else "NULL::text AS provenance"
    organizations = (
        await connection.execute(
            text(
                "SELECT id, name, domain, type, country_code, tax_id, verification_status, "
                f"{provenance_projection} FROM organizations "
                "WHERE id = ANY(CAST(:organization_ids AS uuid[])) "
                "ORDER BY id FOR UPDATE"
            ),
            {"organization_ids": [str(value) for value in ids]},
        )
    ).mappings().all()
    by_id = {UUID(str(row["id"])): row for row in organizations}
    missing = set(ids) - set(by_id)
    if missing:
        raise ValueError(
            "organization IDs do not exist: "
            + ", ".join(str(value) for value in sorted(missing, key=str))
        )

    member_rows = (
        await connection.execute(
            text(
                "SELECT id, organization_id, role, status, email_verified, "
                "must_change_password, kyc_status, kyc_organization_id FROM users "
                "WHERE organization_id = ANY(CAST(:organization_ids AS uuid[])) "
                "ORDER BY organization_id, id FOR UPDATE"
            ),
            {"organization_ids": [str(value) for value in ids]},
        )
    ).mappings().all()
    eligible_by_org: dict[UUID, list[UUID]] = {value: [] for value in ids}
    for row in member_rows:
        organization_id = UUID(str(row["organization_id"]))
        if (
            str(row["role"]) in {"BUYER", "SUPPLIER"}
            and str(row["status"]) == "APPROVED"
            and row["email_verified"] is True
            and row["must_change_password"] is False
            and str(row["kyc_status"] or "PENDING") != "REJECTED"
            and (
                row["kyc_organization_id"] is None
                or UUID(str(row["kyc_organization_id"])) == organization_id
            )
        ):
            eligible_by_org[organization_id].append(UUID(str(row["id"])))

    existing = {
        UUID(str(row["organization_id"])): row
        for row in (
            await connection.execute(
                text(
                    "SELECT organization_id, previous_verification_status, "
                    "reviewed_snapshot, environment, database_name "
                    "FROM organization_market_approvals "
                    "WHERE organization_id = ANY(CAST(:organization_ids AS uuid[])) "
                    "ORDER BY organization_id FOR UPDATE"
                ),
                {"organization_ids": [str(value) for value in ids]},
            )
        ).mappings()
    }

    if has_provenance:
        active_unknown = (
            await connection.execute(
                text(
                    "SELECT id FROM orderbook_orders "
                    "WHERE organization_id = ANY(CAST(:organization_ids AS uuid[])) "
                    "AND status NOT IN ('CANCELLED', 'EXPIRED') "
                    "AND provenance = 'UNKNOWN' ORDER BY id FOR UPDATE"
                ),
                {"organization_ids": [str(value) for value in ids]},
            )
        ).scalars().all()
        if active_unknown:
            raise ValueError(
                "post-integrity REAL approval requires quarantine, cancellation, "
                "or expiry of nonterminal UNKNOWN orders: "
                + ", ".join(str(value) for value in active_unknown)
            )

    reports: list[OrganizationMarketApprovalReport] = []
    snapshots: dict[UUID, dict[str, Any]] = {}
    for organization_id in ids:
        organization = by_id[organization_id]
        status = str(organization["verification_status"] or "").upper()
        provenance = organization["provenance"]
        eligible_user_ids = tuple(eligible_by_org[organization_id])
        if status == "REJECTED":
            raise ValueError(f"organization {organization_id} is rejected")
        if not eligible_user_ids:
            raise ValueError(
                f"organization {organization_id} has no approved email-verified trader"
            )
        if provenance not in (None, "UNKNOWN", "REAL"):
            raise ValueError(
                f"organization {organization_id} has incompatible provenance {provenance}"
            )
        existing_approval = existing.get(organization_id)
        already_approved = existing_approval is not None
        snapshot = {
            "organization": {
                "id": str(organization_id),
                "name": organization["name"],
                "domain": organization["domain"],
                "type": str(organization["type"]),
                "country_code": organization["country_code"],
                "tax_id": organization["tax_id"],
                "verification_status": "APPROVED",
            },
            "eligible_user_ids": [str(value) for value in eligible_user_ids],
        }
        snapshots[organization_id] = snapshot
        snapshot_json = json.dumps(snapshot, sort_keys=True, separators=(",", ":"))
        snapshot_hash = sha256(snapshot_json.encode("utf-8")).hexdigest()
        if existing_approval is not None:
            if (
                existing_approval["environment"] != context.environment
                or existing_approval["database_name"] != context.database_name
                or existing_approval["reviewed_snapshot"] != snapshot
            ):
                raise ValueError(
                    f"organization {organization_id} approval record has drifted"
                )
            previous_status = existing_approval["previous_verification_status"]
        else:
            previous_status = status
        if apply and expected_snapshots[organization_id] != snapshot_hash:
            raise ValueError(
                f"organization {organization_id} snapshot changed after dry-run review"
            )
        reports.append(
            OrganizationMarketApprovalReport(
                organization_id=organization_id,
                previous_verification_status=previous_status,
                eligible_user_ids=eligible_user_ids,
                provenance=provenance,
                already_approved=already_approved,
                snapshot_sha256=snapshot_hash,
                reviewed_snapshot=snapshot,
            )
        )

    if not apply:
        return tuple(reports)

    for report in reports:
        if not report.already_approved:
            await connection.execute(
                text(
                    "INSERT INTO organization_market_approvals "
                    "(organization_id, previous_verification_status, reviewed_snapshot, "
                    "environment, database_name, reason, operator, reference) VALUES "
                    "(:organization_id, :previous_status, CAST(:reviewed_snapshot AS json), "
                    ":environment, :database_name, :reason, :operator, :reference)"
                ),
                {
                    "organization_id": report.organization_id,
                    "previous_status": report.previous_verification_status,
                    "reviewed_snapshot": json.dumps(snapshots[report.organization_id]),
                    "environment": context.environment,
                    "database_name": context.database_name,
                    "reason": context.reason,
                    "operator": context.operator,
                    "reference": context.reference,
                },
            )
        await connection.execute(
            text(
                "UPDATE organizations SET verification_status = 'APPROVED' "
                "WHERE id = :organization_id"
            ),
            {"organization_id": report.organization_id},
        )
        if not report.already_approved:
            await connection.execute(
                text(
                    "INSERT INTO audit_logs "
                    "(id, action, resource_type, resource_id, changes) VALUES "
                    "(:id, 'MARKET_ORGANIZATION_APPROVED', 'organization', "
                    ":resource_id, CAST(:changes AS jsonb))"
                ),
                {
                    "id": uuid4(),
                    "resource_id": str(report.organization_id),
                    "changes": json.dumps(
                        {
                            "verification_status": {
                                "from": report.previous_verification_status,
                                "to": "APPROVED",
                            },
                            "provenance": {
                                "from": report.provenance,
                                "to": "REAL",
                            },
                            "operator": context.operator,
                            "reason": context.reason,
                            "reference": context.reference,
                        }
                    ),
                },
            )
        if has_provenance:
            await connection.execute(
                text(
                    "UPDATE organizations SET provenance = 'REAL' "
                    "WHERE id = :organization_id AND provenance = 'UNKNOWN'"
                ),
                {"organization_id": report.organization_id},
            )
    return tuple(reports)


async def discover_order_ids(
    connection: AsyncConnection,
    *,
    scope: Literal["sentinel", "stale-demo", "test"],
    now: datetime | None = None,
    limit: int = MAX_QUARANTINE_IDS,
) -> DiscoveryReport:
    if limit < 1 or limit > MAX_QUARANTINE_IDS:
        raise ValueError(f"limit must be between 1 and {MAX_QUARANTINE_IDS}")

    params: dict[str, Any] = {"limit": limit + 1}
    if scope == "sentinel":
        predicate = "id = :sentinel_id"
        params["sentinel_id"] = KNOWN_ZERO_SENTINEL_ID
    elif scope == "stale-demo":
        predicate = (
            "organization_id = ANY(CAST(:organization_ids AS uuid[])) "
            "AND status IN ('OPEN','PARTIALLY_FILLED') "
            "AND (expires_at IS NULL OR expires_at <= :now)"
        )
        params["organization_ids"] = [str(value) for value in DEMO_MARKET_ORG_IDS]
        params["now"] = now or datetime.now(UTC)
    elif scope == "test":
        has_provenance = await _has_column(
            connection,
            table_name="orderbook_orders",
            column_name="provenance",
        )
        provenance_clause = " OR provenance = 'TEST'" if has_provenance else ""
        predicate = (
            "(organization_id = ANY(CAST(:organization_ids AS uuid[]))"
            f"{provenance_clause})"
        )
        params["organization_ids"] = [str(value) for value in KNOWN_TEST_ORG_IDS]
    else:  # pragma: no cover - Literal protects callers
        raise ValueError(f"unsupported discovery scope {scope!r}")

    count = int(
        (
            await connection.execute(
                text(f"SELECT count(*) FROM orderbook_orders WHERE {predicate}"),
                params,
            )
        ).scalar_one()
    )
    rows = (
        await connection.execute(
            text(
                f"SELECT id FROM orderbook_orders WHERE {predicate} "
                "ORDER BY id LIMIT :limit"
            ),
            params,
        )
    ).scalars().all()
    selected = tuple(UUID(str(value)) for value in rows[:limit])
    return DiscoveryReport(
        scope=scope,
        matching_count=count,
        order_ids=selected,
        truncated=count > len(selected),
    )


async def _dependency_rows(
    connection: AsyncConnection,
    *,
    order_ids: tuple[UUID, ...],
) -> tuple[
    dict[UUID, dict[str, Any]],
    dict[str, list[dict[str, Any]]],
    set[UUID],
]:
    params = {"order_ids": [str(value) for value in order_ids]}
    has_inventory_link = await _has_column(
        connection,
        table_name="orderbook_orders",
        column_name="inventory_item_id",
    )
    inventory_projection = (
        "inventory_item_id" if has_inventory_link else "NULL::uuid AS inventory_item_id"
    )
    order_rows = (
        await connection.execute(
            text(
                f"SELECT id, {inventory_projection}, to_jsonb(o) AS original_row "
                "FROM orderbook_orders AS o "
                "WHERE id = ANY(CAST(:order_ids AS uuid[]))"
            ),
            params,
        )
    ).mappings().all()
    orders = {UUID(str(row["id"])): dict(row) for row in order_rows}

    queries = {
        "trades": (
            "SELECT id, bid_order_id, ask_order_id, to_jsonb(t) AS row "
            "FROM trades AS t WHERE bid_order_id = ANY(CAST(:order_ids AS uuid[])) "
            "OR ask_order_id = ANY(CAST(:order_ids AS uuid[]))"
        ),
        "match_suggestions": (
            "SELECT id, bid_order_id, ask_order_id, to_jsonb(m) AS row "
            "FROM match_suggestions AS m WHERE bid_order_id = ANY(CAST(:order_ids AS uuid[])) "
            "OR ask_order_id = ANY(CAST(:order_ids AS uuid[]))"
        ),
        "watchlist_targets": (
            "SELECT id, order_id, to_jsonb(w) AS row FROM watchlist_targets AS w "
            "WHERE order_id = ANY(CAST(:order_ids AS uuid[]))"
        ),
        "negotiations": (
            "SELECT id, bid_order_id, ask_order_id, to_jsonb(n) AS row "
            "FROM negotiations AS n WHERE bid_order_id = ANY(CAST(:order_ids AS uuid[])) "
            "OR ask_order_id = ANY(CAST(:order_ids AS uuid[]))"
        ),
    }
    raw: dict[str, list[dict[str, Any]]] = {}
    companion_order_ids: set[UUID] = set()
    for table_name, query in queries.items():
        rows = (await connection.execute(text(query), params)).mappings().all()
        raw[table_name] = [dict(row) for row in rows]
        for row in rows:
            for key in ("bid_order_id", "ask_order_id"):
                value = row.get(key)
                if value is not None:
                    companion_order_ids.add(UUID(str(value)))

    trade_ids = [str(row["id"]) for row in raw["trades"]]
    if trade_ids:
        raw["commissions"] = [
            dict(row)
            for row in (
                await connection.execute(
                    text(
                        "SELECT id, trade_id, to_jsonb(c) AS row FROM commissions AS c "
                        "WHERE trade_id = ANY(CAST(:trade_ids AS uuid[]))"
                    ),
                    {"trade_ids": trade_ids},
                )
            ).mappings().all()
        ]
    else:
        raw["commissions"] = []

    target_ids = [str(row["id"]) for row in raw["watchlist_targets"]]
    if target_ids:
        raw["watchlist_events"] = [
            dict(row)
            for row in (
                await connection.execute(
                    text(
                        "SELECT id, watchlist_target_id, to_jsonb(e) AS row "
                        "FROM watchlist_events AS e "
                        "WHERE watchlist_target_id = ANY(CAST(:target_ids AS uuid[]))"
                    ),
                    {"target_ids": target_ids},
                )
            ).mappings().all()
        ]
    else:
        raw["watchlist_events"] = []

    negotiation_ids = [str(row["id"]) for row in raw["negotiations"]]
    if negotiation_ids:
        raw["negotiation_rounds"] = [
            dict(row)
            for row in (
                await connection.execute(
                    text(
                        "SELECT id, negotiation_id, to_jsonb(r) AS row "
                        "FROM negotiation_rounds AS r "
                        "WHERE negotiation_id = ANY(CAST(:negotiation_ids AS uuid[]))"
                    ),
                    {"negotiation_ids": negotiation_ids},
                )
            ).mappings().all()
        ]
    else:
        raw["negotiation_rounds"] = []

    return orders, raw, companion_order_ids


def _belongs_to_order(
    table_name: str,
    dependency: dict[str, Any],
    order_id: UUID,
    raw: dict[str, list[dict[str, Any]]],
) -> bool:
    order_text = str(order_id)
    if table_name in {"trades", "match_suggestions", "negotiations"}:
        return order_text in {
            str(dependency.get("bid_order_id")),
            str(dependency.get("ask_order_id")),
        }
    if table_name == "watchlist_targets":
        return str(dependency.get("order_id")) == order_text
    if table_name == "commissions":
        related_trade_ids = {
            str(row["id"])
            for row in raw["trades"]
            if _belongs_to_order("trades", row, order_id, raw)
        }
        return str(dependency.get("trade_id")) in related_trade_ids
    if table_name == "watchlist_events":
        related_target_ids = {
            str(row["id"])
            for row in raw["watchlist_targets"]
            if _belongs_to_order("watchlist_targets", row, order_id, raw)
        }
        return str(dependency.get("watchlist_target_id")) in related_target_ids
    if table_name == "negotiation_rounds":
        related_negotiation_ids = {
            str(row["id"])
            for row in raw["negotiations"]
            if _belongs_to_order("negotiations", row, order_id, raw)
        }
        return str(dependency.get("negotiation_id")) in related_negotiation_ids
    return False


async def inspect_order_quarantine(
    connection: AsyncConnection,
    order_ids: Iterable[str | UUID],
) -> tuple[OrderQuarantineReport, ...]:
    validated_ids = validate_order_ids(order_ids)
    orders, raw, companion_order_ids = await _dependency_rows(
        connection,
        order_ids=validated_ids,
    )
    archived = set(
        UUID(str(value))
        for value in (
            await connection.execute(
                text(
                    "SELECT source_id FROM market_row_quarantines "
                    "WHERE source_table = 'orderbook_orders' "
                    "AND source_id = ANY(CAST(:order_ids AS uuid[]))"
                ),
                {"order_ids": [str(value) for value in validated_ids]},
            )
        ).scalars().all()
    )
    selected = set(validated_ids)
    external = tuple(sorted(companion_order_ids - selected, key=str))
    reports: list[OrderQuarantineReport] = []
    for order_id in validated_ids:
        order = orders.get(order_id)
        dependencies = {
            table_name: [
                dict(item["row"])
                for item in rows
                if _belongs_to_order(table_name, item, order_id, raw)
            ]
            for table_name, rows in raw.items()
        }
        reports.append(
            OrderQuarantineReport(
                order_id=order_id,
                found=order is not None,
                already_quarantined=order_id in archived,
                original_row=dict(order["original_row"]) if order else None,
                dependencies=dependencies,
                external_order_ids=external,
                inventory_item_id=(
                    UUID(str(order["inventory_item_id"]))
                    if order and order["inventory_item_id"] is not None
                    else None
                ),
            )
        )
    return tuple(reports)


async def inspect_accepted_rfq_quarantine(
    connection: AsyncConnection,
    rfq_ids: Iterable[str | UUID],
    *,
    trade_bindings: dict[UUID, UUID] | None = None,
) -> tuple[AcceptedRFQQuarantineReport, ...]:
    """Discover exact accepted RFQ graphs without taking write locks."""
    selected_ids = validate_rfq_ids(rfq_ids)
    bindings = trade_bindings or {}
    unknown_bindings = set(bindings).difference(selected_ids)
    if unknown_bindings:
        raise ValueError(
            "trade bindings contain RFQs outside the explicit set: "
            + ", ".join(map(str, sorted(unknown_bindings, key=str)))
        )
    params = {"rfq_ids": [str(value) for value in selected_ids]}
    rfq_rows = (
        await connection.execute(
            text(
                "SELECT id, buyer_org_id, product_id, delivery_point_id, "
                "quantity_mt, availability_window, status, created_at, "
                "to_jsonb(r) AS row FROM rfqs AS r "
                "WHERE id = ANY(CAST(:rfq_ids AS uuid[])) ORDER BY id"
            ),
            params,
        )
    ).mappings().all()
    rfqs = {UUID(str(row["id"])): dict(row) for row in rfq_rows}
    quote_rows = (
        await connection.execute(
            text(
                "SELECT id, rfq_id, seller_org_id, price_per_mt_usd, status, "
                "created_at, to_jsonb(q) AS row FROM rfq_quotes AS q "
                "WHERE rfq_id = ANY(CAST(:rfq_ids AS uuid[])) ORDER BY rfq_id, id"
            ),
            params,
        )
    ).mappings().all()
    quotes_by_rfq: dict[UUID, list[dict[str, Any]]] = {
        rfq_id: [] for rfq_id in selected_ids
    }
    for row in quote_rows:
        quotes_by_rfq[UUID(str(row["rfq_id"]))].append(dict(row))

    archived = {
        UUID(str(value))
        for value in (
            await connection.execute(
                text(
                    "SELECT source_id FROM market_row_quarantines "
                    "WHERE source_table = 'rfqs' "
                    "AND source_id = ANY(CAST(:rfq_ids AS uuid[]))"
                ),
                params,
            )
        ).scalars()
    }

    reports: list[AcceptedRFQQuarantineReport] = []
    for rfq_id in selected_ids:
        rfq = rfqs.get(rfq_id)
        quotes = quotes_by_rfq[rfq_id]
        accepted = [row for row in quotes if row["status"] == "ACCEPTED"]
        accepted_quote = accepted[0] if len(accepted) == 1 else None
        candidates: list[dict[str, Any]] = []
        selected_trade_id = bindings.get(rfq_id)
        commissions: list[dict[str, Any]] = []
        blockers: dict[str, list[dict[str, Any]]] = {}
        if rfq is not None and accepted_quote is not None:
            candidate_rows = (
                await connection.execute(
                    text(
                        "SELECT id, bid_order_id, ask_order_id, buyer_id, seller_id, "
                        "quantity_mt, price_per_mt_usd, status, created_at, "
                        "to_jsonb(t) AS row FROM trades AS t "
                        "WHERE buyer_id = :buyer_id AND seller_id = :seller_id "
                        "AND quantity_mt = :quantity_mt AND price_per_mt_usd = :price "
                        "AND bid_order_id IS NULL AND ask_order_id IS NULL "
                        "AND created_at >= :quote_created_at ORDER BY created_at, id"
                    ),
                    {
                        "buyer_id": rfq["buyer_org_id"],
                        "seller_id": accepted_quote["seller_org_id"],
                        "quantity_mt": rfq["quantity_mt"],
                        "price": accepted_quote["price_per_mt_usd"],
                        "quote_created_at": accepted_quote["created_at"],
                    },
                )
            ).mappings().all()
            candidates = [dict(row) for row in candidate_rows]
            if selected_trade_id is not None:
                selected = next(
                    (
                        row
                        for row in candidates
                        if UUID(str(row["id"])) == selected_trade_id
                    ),
                    None,
                )
                if selected is None:
                    blockers["trade_binding"] = [
                        {
                            "trade_id": str(selected_trade_id),
                            "reason": "explicit trade is not an exact RFQ candidate",
                        }
                    ]
                else:
                    commission_result = await connection.execute(
                        text(
                            "SELECT id, trade_id, to_jsonb(c) AS row "
                            "FROM commissions AS c WHERE trade_id = :trade_id ORDER BY id"
                        ),
                        {"trade_id": selected_trade_id},
                    )
                    commissions = [dict(row) for row in commission_result.mappings()]
                    negotiation_rows = (
                        await connection.execute(
                            text(
                                "SELECT id, trade_id, to_jsonb(n) AS row "
                                "FROM negotiations AS n WHERE trade_id = :trade_id ORDER BY id"
                            ),
                            {"trade_id": selected_trade_id},
                        )
                    ).mappings().all()
                    if negotiation_rows:
                        blockers["negotiations"] = [
                            dict(row["row"]) for row in negotiation_rows
                        ]

        reports.append(
            AcceptedRFQQuarantineReport(
                rfq_id=rfq_id,
                found=rfq is not None,
                already_quarantined=rfq_id in archived,
                original_row=(dict(rfq["row"]) if rfq else None),
                quote_rows=tuple(dict(row["row"]) for row in quotes),
                accepted_quote_id=(
                    UUID(str(accepted_quote["id"])) if accepted_quote else None
                ),
                trade_candidates=tuple(dict(row["row"]) for row in candidates),
                selected_trade_id=selected_trade_id,
                commission_rows=tuple(dict(row["row"]) for row in commissions),
                blocking_dependencies=blockers,
            )
        )
    return tuple(reports)


_INSERT_ANY_QUARANTINE = text(
    """
    INSERT INTO market_row_quarantines (
        id, source_table, source_id, original_row, dependencies,
        environment, database_name, reason, operator, reference
    ) VALUES (
        :id, :source_table, :source_id, :original_row, :dependencies,
        :environment, :database_name, :reason, :operator, :reference
    )
    ON CONFLICT (source_table, source_id) DO NOTHING
    """
).bindparams(
    bindparam("original_row", type_=JSON),
    bindparam("dependencies", type_=JSON),
)


async def _archive_market_row(
    connection: AsyncConnection,
    *,
    source_table: str,
    source_id: UUID,
    original_row: dict[str, Any],
    dependencies: dict[str, Any],
    context: OperatorContext,
) -> None:
    await connection.execute(
        _INSERT_ANY_QUARANTINE,
        {
            "id": uuid4(),
            "source_table": source_table,
            "source_id": source_id,
            "original_row": original_row,
            "dependencies": dependencies,
            "environment": context.environment,
            "database_name": context.database_name,
            "reason": context.reason,
            "operator": context.operator,
            "reference": context.reference,
        },
    )


async def quarantine_accepted_rfqs(
    connection: AsyncConnection,
    rfq_ids: Iterable[str | UUID],
    *,
    trade_bindings: dict[UUID, UUID] | None,
    no_trade_rfqs: set[UUID],
    context: OperatorContext,
    approval_reference: str | None,
    apply: bool = False,
) -> tuple[AcceptedRFQQuarantineReport, ...]:
    """Archive exact accepted RFQ/quote/demo-trade graphs, then remove them."""
    selected_ids = validate_rfq_ids(rfq_ids)
    selected_id_set = set(selected_ids)
    trade_binding_ids = set((trade_bindings or {}).keys())
    if not no_trade_rfqs <= selected_id_set:
        raise ValueError("explicit no-trade RFQ IDs must be included in --rfq-id")
    if no_trade_rfqs & trade_binding_ids:
        raise ValueError("an RFQ cannot have both trade and explicit no-trade bindings")
    reports = await inspect_accepted_rfq_quarantine(
        connection, selected_ids, trade_bindings=trade_bindings
    )
    if not apply:
        return reports
    if not approval_reference or approval_reference != context.reference:
        raise ValueError(
            "accepted RFQ quarantine requires an explicit product approval "
            "reference identical to --reference"
        )
    missing = [
        report.rfq_id
        for report in reports
        if not report.found and not report.already_quarantined
    ]
    if missing:
        raise ValueError(
            "explicit accepted RFQ IDs were not found: "
            + ", ".join(map(str, missing))
        )
    active = [report for report in reports if report.found]
    invalid = []
    for report in active:
        no_trade_attested = report.rfq_id in no_trade_rfqs
        trade_binding_valid = (
            report.selected_trade_id is not None and not no_trade_attested
        ) or (
            report.selected_trade_id is None
            and no_trade_attested
            and not report.trade_candidates
        )
        if (
            report.accepted_quote_id is None
            or not trade_binding_valid
            or (report.original_row or {}).get("status") != "ACCEPTED"
            or report.blocking_dependencies
        ):
            invalid.append(report.rfq_id)
    if invalid:
        raise ValueError(
            "accepted RFQ graph is ambiguous, unbound, or has unsupported dependencies; "
            "provide an exact trade binding or explicit no-trade attestation: "
            + ", ".join(map(str, invalid))
        )
    demo_ids = set(DEMO_MARKET_ORG_IDS)
    for report in active:
        rfq = report.original_row or {}
        accepted_quote = next(
            row for row in report.quote_rows if UUID(str(row["id"])) == report.accepted_quote_id
        )
        selected_trade = next(
            (
                row
                for row in report.trade_candidates
                if UUID(str(row["id"])) == report.selected_trade_id
            ),
            None,
        )
        if UUID(str(rfq["buyer_org_id"])) not in demo_ids or UUID(
            str(accepted_quote["seller_org_id"])
        ) not in demo_ids:
            raise ValueError(
                f"RFQ {report.rfq_id} is not an exact deterministic DEMO pair"
            )
        if selected_trade is not None and (
            UUID(str(selected_trade["buyer_id"]))
            != UUID(str(rfq["buyer_org_id"]))
            or UUID(str(selected_trade["seller_id"]))
            != UUID(str(accepted_quote["seller_org_id"]))
        ):
            raise ValueError(f"RFQ {report.rfq_id} trade parties do not match")

    slices = [
        (
            UUID(str(report.original_row["product_id"])),
            (
                UUID(str(report.original_row["delivery_point_id"]))
                if report.original_row.get("delivery_point_id")
                else None
            ),
            str(report.original_row["availability_window"]),
        )
        for report in active
        if report.original_row is not None
    ]
    await _acquire_quarantine_locks(
        connection,
        operation="accepted-rfq-quarantine",
        source_ids=selected_ids,
        slices=slices,
    )

    trade_ids = [
        str(report.selected_trade_id)
        for report in active
        if report.selected_trade_id is not None
    ]
    if trade_ids:
        await connection.execute(
            text(
                "SELECT id FROM trades WHERE id = ANY(CAST(:ids AS uuid[])) "
                "ORDER BY id FOR UPDATE"
            ),
            {"ids": trade_ids},
        )
    await connection.execute(
        text(
            "SELECT id FROM rfqs WHERE id = ANY(CAST(:ids AS uuid[])) "
            "ORDER BY id FOR UPDATE"
        ),
        {"ids": [str(value) for value in selected_ids]},
    )
    quote_ids = [
        str(row["id"]) for report in active for row in report.quote_rows
    ]
    if quote_ids:
        await connection.execute(
            text(
                "SELECT id FROM rfq_quotes WHERE id = ANY(CAST(:ids AS uuid[])) "
                "ORDER BY id FOR UPDATE"
            ),
            {"ids": quote_ids},
        )
    commission_ids = [
        str(row["id"]) for report in active for row in report.commission_rows
    ]
    if commission_ids:
        await connection.execute(
            text(
                "SELECT id FROM commissions WHERE id = ANY(CAST(:ids AS uuid[])) "
                "ORDER BY id FOR UPDATE"
            ),
            {"ids": commission_ids},
        )

    locked_reports = await inspect_accepted_rfq_quarantine(
        connection, selected_ids, trade_bindings=trade_bindings
    )
    if [report.as_json() for report in locked_reports] != [
        report.as_json() for report in reports
    ]:
        raise ValueError("accepted RFQ dependency graph changed while acquiring locks")

    for report in active:
        for commission in report.commission_rows:
            await _archive_market_row(
                connection,
                source_table="commissions",
                source_id=UUID(str(commission["id"])),
                original_row=commission,
                dependencies={"rfq_id": str(report.rfq_id)},
                context=context,
            )
        if report.selected_trade_id is not None:
            selected_trade = next(
                row for row in report.trade_candidates
                if UUID(str(row["id"])) == report.selected_trade_id
            )
            await _archive_market_row(
                connection,
                source_table="trades",
                source_id=report.selected_trade_id,
                original_row=selected_trade,
                dependencies={"rfq_id": str(report.rfq_id)},
                context=context,
            )
        for quote in report.quote_rows:
            await _archive_market_row(
                connection,
                source_table="rfq_quotes",
                source_id=UUID(str(quote["id"])),
                original_row=quote,
                dependencies={"rfq_id": str(report.rfq_id)},
                context=context,
            )
        await _archive_market_row(
            connection,
            source_table="rfqs",
            source_id=report.rfq_id,
            original_row=report.original_row or {},
            dependencies={
                "quote_ids": [str(row["id"]) for row in report.quote_rows],
                "trade_id": (
                    str(report.selected_trade_id)
                    if report.selected_trade_id is not None
                    else None
                ),
                "commission_ids": [
                    str(row["id"]) for row in report.commission_rows
                ],
            },
            context=context,
        )

    for table_name, ids in (
        ("commissions", commission_ids),
        ("trades", trade_ids),
        ("rfq_quotes", quote_ids),
        ("rfqs", [str(report.rfq_id) for report in active]),
    ):
        if ids:
            await connection.execute(
                text(
                    f"DELETE FROM {table_name} "
                    "WHERE id = ANY(CAST(:ids AS uuid[]))"
                ),
                {"ids": ids},
            )
    return reports


_INSERT_QUARANTINE = text(
    """
    INSERT INTO market_row_quarantines (
        id, source_table, source_id, original_row, dependencies,
        environment, database_name, reason, operator, reference
    ) VALUES (
        :id, 'orderbook_orders', :source_id, :original_row, :dependencies,
        :environment, :database_name, :reason, :operator, :reference
    )
    ON CONFLICT (source_table, source_id) DO NOTHING
    """
).bindparams(
    bindparam("original_row", type_=JSON),
    bindparam("dependencies", type_=JSON),
)


async def quarantine_orders(
    connection: AsyncConnection,
    order_ids: Iterable[str | UUID],
    *,
    context: OperatorContext,
    apply: bool = False,
) -> tuple[OrderQuarantineReport, ...]:
    """Archive and remove only the explicitly supplied order IDs."""
    validated_ids = validate_order_ids(order_ids)
    reports = await inspect_order_quarantine(connection, validated_ids)
    if not apply:
        return reports

    missing = [
        report.order_id
        for report in reports
        if not report.found and not report.already_quarantined
    ]
    if missing:
        raise ValueError(f"explicit order IDs were not found: {', '.join(map(str, missing))}")
    external = sorted(
        {item for report in reports for item in report.external_order_ids},
        key=str,
    )
    if external:
        raise ValueError(
            "dependent trades/negotiations reference companion orders not in "
            f"the explicit set: {', '.join(map(str, external))}"
        )
    linked_inventory = [
        report.order_id for report in reports if report.inventory_item_id is not None
    ]
    if linked_inventory:
        raise ValueError(
            "inventory-linked orders require lifecycle-specific remediation; "
            f"refusing IDs: {', '.join(map(str, linked_inventory))}"
        )

    active_reports = [report for report in reports if report.found]
    slices = [
        (
            UUID(str(report.original_row["product_id"])),
            (
                UUID(str(report.original_row["delivery_point_id"]))
                if report.original_row.get("delivery_point_id")
                else None
            ),
            str(report.original_row["availability_window"]),
        )
        for report in active_reports
        if report.original_row is not None
    ]
    await _acquire_quarantine_locks(
        connection,
        operation="order-quarantine",
        source_ids=validated_ids,
        slices=slices,
    )
    dependency_ids: dict[str, list[str]] = {
        table_name: sorted(
            {
                str(row["id"])
                for report in active_reports
                for row in report.dependencies[table_name]
            }
        )
        for table_name in (
            "watchlist_events",
            "watchlist_targets",
            "match_suggestions",
            "commissions",
            "negotiation_rounds",
            "negotiations",
            "trades",
        )
    }
    # Canonical market-row order: trades, orders, negotiations, rounds.
    for table_name, ids in (
        ("trades", dependency_ids["trades"]),
        ("orderbook_orders", [str(report.order_id) for report in active_reports]),
        ("negotiations", dependency_ids["negotiations"]),
        ("negotiation_rounds", dependency_ids["negotiation_rounds"]),
    ):
        if ids:
            await connection.execute(
                text(
                    f"SELECT id FROM {table_name} "
                    "WHERE id = ANY(CAST(:ids AS uuid[])) ORDER BY id FOR UPDATE"
                ),
                {"ids": ids},
            )
    locked_reports = await inspect_order_quarantine(connection, validated_ids)
    if [report.as_json() for report in locked_reports] != [
        report.as_json() for report in reports
    ]:
        raise ValueError("order dependency graph changed while acquiring locks")

    for report in active_reports:
        await connection.execute(
            _INSERT_QUARANTINE,
            {
                "id": uuid4(),
                "source_id": report.order_id,
                "original_row": report.original_row,
                "dependencies": report.dependencies,
                "environment": context.environment,
                "database_name": context.database_name,
                "reason": context.reason,
                "operator": context.operator,
                "reference": context.reference,
            },
        )

    for table_name in (
        "watchlist_events",
        "watchlist_targets",
        "match_suggestions",
        "commissions",
        "negotiation_rounds",
        "negotiations",
        "trades",
    ):
        ids = dependency_ids[table_name]
        if ids:
            await connection.execute(
                text(
                    f"DELETE FROM {table_name} "
                    "WHERE id = ANY(CAST(:dependency_ids AS uuid[]))"
                ),
                {"dependency_ids": ids},
            )
    if active_reports:
        await connection.execute(
            text(
                "DELETE FROM orderbook_orders "
                "WHERE id = ANY(CAST(:order_ids AS uuid[]))"
            ),
            {"order_ids": [str(report.order_id) for report in active_reports]},
        )
    return reports


async def rename_known_demo_organizations(
    connection: AsyncConnection,
    *,
    context: OperatorContext,
    apply: bool = False,
) -> list[dict[str, Any]]:
    """Rename only exact deterministic demo IDs; never match organization names."""
    has_provenance = await _has_column(
        connection,
        table_name="organizations",
        column_name="provenance",
    )
    ids = [str(value) for value in CANONICAL_DEMO_ORG_NAMES]
    select_columns = "id, name" + (", provenance" if has_provenance else "")
    rows = (
        await connection.execute(
            text(
                f"SELECT {select_columns} FROM organizations "
                "WHERE id = ANY(CAST(:organization_ids AS uuid[])) ORDER BY id FOR UPDATE"
            ),
            {"organization_ids": ids},
        )
    ).mappings().all()
    report = [
        {
            "id": str(row["id"]),
            "before": row["name"],
            "after": CANONICAL_DEMO_ORG_NAMES[UUID(str(row["id"]))],
            "provenance": row.get("provenance"),
        }
        for row in rows
    ]
    if not apply:
        return report
    non_demo = [
        row["id"]
        for row in report
        if has_provenance and row["provenance"] != "DEMO"
    ]
    if non_demo:
        raise ValueError(
            "exact demo IDs have non-DEMO provenance; refusing rename: "
            + ", ".join(non_demo)
        )
    for row in report:
        await connection.execute(
            text("UPDATE organizations SET name = :name WHERE id = :organization_id"),
            {"name": row["after"], "organization_id": row["id"]},
        )

    action_hash = sha256(context.reference.encode("utf-8")).hexdigest()[:16]
    marker = text(
        """
        INSERT INTO seed_runs (id, seed_name, environment, metadata)
        VALUES (:id, :seed_name, :environment, :metadata)
        ON CONFLICT (seed_name, environment) DO NOTHING
        """
    ).bindparams(bindparam("metadata", type_=JSON))
    await connection.execute(
        marker,
        {
            "id": uuid4(),
            "seed_name": f"operator:demo-rename:{action_hash}",
            "environment": context.environment,
            "metadata": {
                "database": context.database_name,
                "operator": context.operator,
                "reason": context.reason,
                "reference": context.reference,
                "changes": report,
            },
        },
    )
    return report
