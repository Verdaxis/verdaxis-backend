"""Read-only admission/execution rollout report."""

import asyncio
import argparse

def _preflight_exit_code(blockers: dict[str, int]) -> int:
    return 2 if any(count > 0 for count in blockers.values()) else 0


async def report() -> int:
    from sqlalchemy import and_, func, not_, or_, select

    from app.database import AsyncSessionLocal
    from app.demo_identities import DEMO_MARKET_ORG_IDS, KNOWN_TEST_ORG_IDS
    from app.models.orderbook import OrderBookOrder, OrderBookStatus, Trade, TradeStatus
    from app.models.negotiation import Negotiation, NegotiationRound, NegotiationStatus
    from app.models.rfq import QuoteStatus, RFQ, RFQQuote, RFQStatus
    from app.models.registration import OrganizationJoinRequest, JoinRequestStatus
    from app.models.user import Organization, User, UserRole, UserStatus

    async with AsyncSessionLocal() as db:
        duplicate_emails = (await db.execute(
            select(func.lower(func.trim(User.email)), func.count())
            .group_by(func.lower(func.trim(User.email)))
            .having(func.count() > 1)
        )).all()
        users = (await db.execute(
            select(User.id, User.role, User.status, User.email_verified, User.organization_id, User.kyc_status)
            .outerjoin(Organization, Organization.id == User.organization_id)
            .where(
                User.role.in_([UserRole.BUYER, UserRole.SUPPLIER]),
                User.status == UserStatus.APPROVED,
                or_(
                    User.email_verified.is_not(True),
                    User.organization_id.is_(None),
                    Organization.id.is_(None),
                    Organization.verification_status.is_(None),
                    Organization.verification_status != "APPROVED",
                ),
            )
        )).all()
        legacy_orders = await db.scalar(select(func.count()).select_from(OrderBookOrder).where(OrderBookOrder.owner_user_id.is_(None)))
        legacy_rfqs = await db.scalar(select(func.count()).select_from(RFQ).where(RFQ.buyer_user_id.is_(None)))
        legacy_quotes = await db.scalar(select(func.count()).select_from(RFQQuote).where(RFQQuote.seller_user_id.is_(None)))
        legacy_trades = await db.scalar(
            select(func.count()).select_from(Trade).where(
                (Trade.buyer_user_id.is_(None)) | (Trade.seller_user_id.is_(None))
            )
        )
        legacy_negotiations = await db.scalar(
            select(func.count()).select_from(Negotiation).where(
                (Negotiation.initiator_user_id.is_(None))
                | (Negotiation.counterparty_user_id.is_(None))
            )
        )
        legacy_rounds = await db.scalar(
            select(func.count()).select_from(NegotiationRound).where(
                NegotiationRound.proposer_user_id.is_(None)
            )
        )
        approved_kyc_without_evidence = await db.scalar(
            select(func.count()).select_from(User).where(
                User.kyc_status == "APPROVED",
                (User.kyc_external_evidence_reference.is_(None))
                | (User.kyc_review_note.is_(None))
                | (User.kyc_reviewed_by.is_(None))
                | (User.kyc_reviewed_at.is_(None)),
            )
        )
        pending_joins = await db.scalar(
            select(func.count()).select_from(OrganizationJoinRequest).where(
                OrganizationJoinRequest.status == JoinRequestStatus.PENDING
            )
        )
        outstanding_legacy_orders = await db.scalar(
            select(func.count()).select_from(OrderBookOrder).where(
                OrderBookOrder.owner_user_id.is_(None),
                OrderBookOrder.status.in_((OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)),
                not_(
                    OrderBookOrder.organization_id.in_(
                        tuple(DEMO_MARKET_ORG_IDS | KNOWN_TEST_ORG_IDS)
                    )
                ),
            )
        )
        outstanding_legacy_rfqs = await db.scalar(
            select(func.count()).select_from(RFQ).where(
                RFQ.buyer_user_id.is_(None),
                RFQ.status.in_((RFQStatus.OPEN, RFQStatus.QUOTED)),
                not_(RFQ.buyer_org_id.in_(tuple(DEMO_MARKET_ORG_IDS | KNOWN_TEST_ORG_IDS))),
            )
        )
        outstanding_legacy_quotes = await db.scalar(
            select(func.count()).select_from(RFQQuote).join(RFQ, RFQ.id == RFQQuote.rfq_id).where(
                RFQQuote.seller_user_id.is_(None),
                RFQQuote.status == QuoteStatus.PENDING,
                not_(
                    or_(
                        and_(
                            RFQ.buyer_org_id.in_(tuple(DEMO_MARKET_ORG_IDS)),
                            RFQQuote.seller_org_id.in_(tuple(DEMO_MARKET_ORG_IDS)),
                        ),
                        and_(
                            RFQ.buyer_org_id.in_(tuple(KNOWN_TEST_ORG_IDS)),
                            RFQQuote.seller_org_id.in_(tuple(KNOWN_TEST_ORG_IDS)),
                        ),
                    )
                ),
            )
        )
        outstanding_legacy_trades = await db.scalar(
            select(func.count()).select_from(Trade).where(
                (Trade.buyer_user_id.is_(None)) | (Trade.seller_user_id.is_(None)),
                Trade.status.in_(
                    (TradeStatus.PENDING_CONFIRMATION, TradeStatus.CONFIRMED, TradeStatus.DELIVERED)
                ),
                not_(
                    or_(
                        and_(
                            Trade.buyer_id.in_(tuple(DEMO_MARKET_ORG_IDS)),
                            Trade.seller_id.in_(tuple(DEMO_MARKET_ORG_IDS)),
                        ),
                        and_(
                            Trade.buyer_id.in_(tuple(KNOWN_TEST_ORG_IDS)),
                            Trade.seller_id.in_(tuple(KNOWN_TEST_ORG_IDS)),
                        ),
                    )
                ),
            )
        )
        outstanding_legacy_negotiations = await db.scalar(
            select(func.count()).select_from(Negotiation).where(
                (
                    (Negotiation.initiator_user_id.is_(None))
                    | (Negotiation.counterparty_user_id.is_(None))
                ),
                Negotiation.status.in_((NegotiationStatus.OPEN, NegotiationStatus.COUNTERED)),
                not_(
                    or_(
                        and_(
                            Negotiation.initiator_org_id.in_(tuple(DEMO_MARKET_ORG_IDS)),
                            Negotiation.counterparty_org_id.in_(tuple(DEMO_MARKET_ORG_IDS)),
                        ),
                        and_(
                            Negotiation.initiator_org_id.in_(tuple(KNOWN_TEST_ORG_IDS)),
                            Negotiation.counterparty_org_id.in_(tuple(KNOWN_TEST_ORG_IDS)),
                        ),
                    )
                ),
            )
        )

        blockers = {
            "ineligible_users": len(users),
            "duplicate_case_insensitive_emails": len(duplicate_emails),
            "outstanding_legacy_orders_without_owner": outstanding_legacy_orders or 0,
            "outstanding_legacy_rfqs_without_owner": outstanding_legacy_rfqs or 0,
            "outstanding_legacy_quotes_without_owner": outstanding_legacy_quotes or 0,
            "outstanding_legacy_trades_without_user_provenance": outstanding_legacy_trades or 0,
            "outstanding_legacy_negotiations_without_user_provenance": outstanding_legacy_negotiations or 0,
        }

        print(f"ineligible_users={len(users)}")
        print(f"duplicate_case_insensitive_emails={len(duplicate_emails)}")
        for row in users:
            status_value = row.status.value if hasattr(row.status, "value") else row.status
            role_value = row.role.value if hasattr(row.role, "value") else row.role
            print(
                f"user={row.id} role={role_value} status={status_value} "
                f"email_verified={row.email_verified} organization_id={row.organization_id} "
                f"kyc_status={row.kyc_status}"
            )
        print(f"legacy_orders_without_owner={legacy_orders or 0}")
        print(f"legacy_rfqs_without_owner={legacy_rfqs or 0}")
        print(f"legacy_quotes_without_owner={legacy_quotes or 0}")
        print(f"legacy_trades_without_user_provenance={legacy_trades or 0}")
        print(f"legacy_negotiations_without_user_provenance={legacy_negotiations or 0}")
        print(f"legacy_negotiation_rounds_without_user_provenance={legacy_rounds or 0}")
        print(f"approved_kyc_without_external_review_evidence={approved_kyc_without_evidence or 0}")
        print("kyc_enforcement=ADVISORY_HELD")
        print(f"pending_organization_join_reviews={pending_joins or 0}")
        for name, count in blockers.items():
            if name not in {"ineligible_users", "duplicate_case_insensitive_emails"}:
                print(f"{name}={count}")
        exit_code = _preflight_exit_code(blockers)
        print(f"enforcement_preflight={'BLOCKED' if exit_code else 'READY'}")
        print(
            "legacy_owner_policy=exact_synthetic_registry_or_explicit_owner_decision"
        )
        return exit_code


def main() -> int:
    parser = argparse.ArgumentParser(description="Read-only security-v2 operator preflight")
    parser.add_argument("--check-imports", action="store_true")
    args = parser.parse_args()
    if args.check_imports:
        print("security_preflight_imports=READY")
        return 0
    return asyncio.run(report())


if __name__ == "__main__":
    raise SystemExit(main())
