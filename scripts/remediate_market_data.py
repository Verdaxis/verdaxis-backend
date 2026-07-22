#!/usr/bin/env python3
"""Dry-run-first operator CLI for exact synthetic market remediation."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
from pathlib import Path
from typing import Any

from sqlalchemy.ext.asyncio import create_async_engine

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from app.services.market_quarantine import (
    MAX_QUARANTINE_IDS,
    OperatorContext,
    approve_real_organizations,
    connected_database_name,
    discover_order_ids,
    quarantine_accepted_rfqs,
    quarantine_orders,
    rename_known_demo_organizations,
    validate_write_authorization,
)
from app.environment_database import validate_database_target


def _database_url(value: str) -> str:
    if value.startswith("postgresql://"):
        return value.replace("postgresql://", "postgresql+asyncpg://", 1)
    if not value.startswith("postgresql+asyncpg://"):
        raise argparse.ArgumentTypeError("only PostgreSQL asyncpg URLs are supported")
    return value


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Inspect or remediate exact synthetic market rows. Mutations require "
            "--apply and exact environment/database attestation."
        )
    )
    parser.add_argument(
        "--database-url",
        type=_database_url,
        default=os.environ.get("MARKET_REMEDIATION_DATABASE_URL"),
        required=os.environ.get("MARKET_REMEDIATION_DATABASE_URL") is None,
    )
    parser.add_argument("--environment", required=True)
    parser.add_argument(
        "--attestation",
        required=True,
        help="Exact '<environment>:<database>' attestation",
    )
    parser.add_argument("--operator", default="")
    parser.add_argument("--reason", default="")
    parser.add_argument("--reference", default="")
    parser.add_argument("--apply", action="store_true", help="Commit the requested mutation")
    parser.add_argument(
        "--production-approval-reference",
        help="Required for production writes and must exactly equal --reference",
    )

    subparsers = parser.add_subparsers(dest="command", required=True)
    discover = subparsers.add_parser(
        "discover",
        help="Report bounded candidate IDs; this command never mutates",
    )
    discover.add_argument(
        "--scope",
        choices=("sentinel", "stale-demo", "test"),
        required=True,
    )
    discover.add_argument("--limit", type=int, default=MAX_QUARANTINE_IDS)

    quarantine = subparsers.add_parser(
        "quarantine",
        help="Archive and remove only explicitly repeated --order-id values",
    )
    quarantine.add_argument("--order-id", action="append", required=True)

    accepted_rfqs = subparsers.add_parser(
        "quarantine-accepted-rfqs",
        help=(
            "Archive exact accepted RFQs, all linked quotes, and explicitly "
            "bound orderless DEMO trades"
        ),
    )
    accepted_rfqs.add_argument("--rfq-id", action="append", required=True)
    accepted_rfqs.add_argument(
        "--rfq-no-trade",
        action="append",
        default=[],
        metavar="RFQ_ID",
        help="Explicitly attest that an accepted RFQ has no dependent trade",
    )
    accepted_rfqs.add_argument(
        "--rfq-trade",
        action="append",
        default=[],
        metavar="RFQ_ID=TRADE_ID",
        help="Exact accepted-RFQ to dependent orderless trade binding",
    )

    real_organizations = subparsers.add_parser(
        "approve-real-organizations",
        help=(
            "Approve exact organizations with eligible traders and record "
            "operator authority for REAL market provenance"
        ),
    )
    real_organizations.add_argument(
        "--organization-id",
        action="append",
        required=True,
    )
    real_organizations.add_argument(
        "--expected-snapshot",
        action="append",
        default=[],
        metavar="ORGANIZATION_ID=SHA256",
        help="Required on apply and copied exactly from the reviewed dry-run",
    )
    accepted_rfqs.add_argument(
        "--accepted-rfq-approval-reference",
        help="Required for apply and must exactly equal --reference",
    )

    subparsers.add_parser(
        "rename-demo-organizations",
        help="Rename only the compiled exact deterministic demo organization IDs",
    )
    return parser.parse_args(argv)


def _json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, default=str)


def _rfq_trade_bindings(values: list[str]) -> dict:
    from uuid import UUID

    bindings: dict[UUID, UUID] = {}
    for value in values:
        try:
            rfq_value, trade_value = value.split("=", 1)
            rfq_id, trade_id = UUID(rfq_value), UUID(trade_value)
        except (ValueError, AttributeError) as exc:
            raise ValueError(
                f"invalid --rfq-trade {value!r}; expected RFQ_UUID=TRADE_UUID"
            ) from exc
        if rfq_id in bindings and bindings[rfq_id] != trade_id:
            raise ValueError(f"conflicting trade bindings for RFQ {rfq_id}")
        bindings[rfq_id] = trade_id
    return bindings


def _rfq_id_set(values: list[str]) -> set:
    from uuid import UUID

    try:
        return {UUID(value) for value in values}
    except (ValueError, AttributeError) as exc:
        raise ValueError("invalid --rfq-no-trade value; expected RFQ_UUID") from exc


def _organization_snapshot_bindings(values: list[str]) -> dict:
    from uuid import UUID

    bindings: dict[UUID, str] = {}
    for value in values:
        try:
            organization_value, snapshot_hash = value.split("=", 1)
            organization_id = UUID(organization_value)
        except (ValueError, AttributeError) as exc:
            raise ValueError(
                f"invalid --expected-snapshot {value!r}; expected ORGANIZATION_UUID=SHA256"
            ) from exc
        if organization_id in bindings and bindings[organization_id] != snapshot_hash:
            raise ValueError(
                f"conflicting expected snapshots for organization {organization_id}"
            )
        bindings[organization_id] = snapshot_hash
    return bindings


async def run(args: argparse.Namespace) -> dict[str, Any]:
    if args.command == "discover" and args.apply:
        raise ValueError("discover is always read-only; remove --apply")

    environment = args.environment.strip().lower()

    engine = create_async_engine(args.database_url, pool_pre_ping=True)
    try:
        connection_context = engine.begin() if args.apply else engine.connect()
        async with connection_context as connection:
            actual_database = await connected_database_name(connection)
            database_name = validate_database_target(
                environment=environment,
                database_url=args.database_url,
                current_database=actual_database,
                supplied=args.attestation,
            )
            context = OperatorContext(
                environment=environment,
                database_name=database_name,
                operator=args.operator,
                reason=args.reason,
                reference=args.reference,
            )
            validate_write_authorization(
                context,
                apply=args.apply,
                production_approval_reference=args.production_approval_reference,
            )

            if args.command == "discover":
                report = await discover_order_ids(
                    connection,
                    scope=args.scope,
                    limit=args.limit,
                )
                return {"dry_run": True, "database": actual_database, **report.as_json()}

            if args.command == "quarantine":
                reports = await quarantine_orders(
                    connection,
                    args.order_id,
                    context=context,
                    apply=args.apply,
                )
                return {
                    "dry_run": not args.apply,
                    "database": actual_database,
                    "orders": [report.as_json() for report in reports],
                }

            if args.command == "quarantine-accepted-rfqs":
                reports = await quarantine_accepted_rfqs(
                    connection,
                    args.rfq_id,
                    trade_bindings=_rfq_trade_bindings(args.rfq_trade),
                    no_trade_rfqs=_rfq_id_set(args.rfq_no_trade),
                    context=context,
                    approval_reference=args.accepted_rfq_approval_reference,
                    apply=args.apply,
                )
                return {
                    "dry_run": not args.apply,
                    "database": actual_database,
                    "accepted_rfqs": [report.as_json() for report in reports],
                }

            if args.command == "approve-real-organizations":
                reports = await approve_real_organizations(
                    connection,
                    args.organization_id,
                    context=context,
                    apply=args.apply,
                    expected_snapshots=_organization_snapshot_bindings(
                        args.expected_snapshot
                    ),
                )
                return {
                    "dry_run": not args.apply,
                    "database": actual_database,
                    "organizations": [report.as_json() for report in reports],
                }

            if args.command == "rename-demo-organizations":
                changes = await rename_known_demo_organizations(
                    connection,
                    context=context,
                    apply=args.apply,
                )
                return {
                    "dry_run": not args.apply,
                    "database": actual_database,
                    "changes": changes,
                }

            raise ValueError(f"unsupported command {args.command!r}")
    finally:
        await engine.dispose()


def main() -> None:
    args = parse_args()
    try:
        result = asyncio.run(run(args))
    except (ValueError, RuntimeError) as exc:
        raise SystemExit(f"refused: {exc}") from exc
    print(_json(result))


if __name__ == "__main__":
    main()
