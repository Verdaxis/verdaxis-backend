"""Canonical audit action registry."""

ADMIN_USER_APPROVED = "admin.user_approved"
ADMIN_ORGANIZATION_APPROVED = "admin.organization_approved"
ADMIN_ORGANIZATION_REJECTED = "admin.organization_rejected"
ORGANIZATION_JOIN_REQUESTED = "organization.join_requested"
ORGANIZATION_JOIN_APPROVED = "organization.join_approved"
ORGANIZATION_JOIN_REJECTED = "organization.join_rejected"
ADMIN_USER_REJECTED = "admin.user_rejected"
COMMISSION_UPDATED = "commission.updated"
INVENTORY_PUBLISHED = "inventory.published"
MARKET_ACCESS_INVALIDATED = "market.access_invalidated"
KYC_APPROVED = "kyc.approved"
KYC_REJECTED = "kyc.rejected"
KYC_SUBMITTED = "kyc.submitted"
KYC_MEMBERSHIP_INVALIDATED = "kyc.membership_invalidated"
# negotiation.accepted / rfq.accepted are intentionally absent: negotiation
# and RFQ execution are disabled in this release (market-owned decision), and
# the registry never carries actions without a live call site.
NEGOTIATION_COUNTERED = "negotiation.countered"
NEGOTIATION_CREATED = "negotiation.created"
NEGOTIATION_DECLINED = "negotiation.declined"
ORDER_CANCELLED = "order.cancelled"
ORDER_CREATED = "order.created"
ORDER_EXPIRED = "order.expired"
ORDER_UPDATED = "order.updated"
RFQ_CANCELLED = "rfq.cancelled"
RFQ_CREATED = "rfq.created"
RFQ_QUOTE_SUBMITTED = "rfq.quote_submitted"
RFQ_QUOTE_WITHDRAWN = "rfq.quote_withdrawn"
SUBSCRIPTION_UPDATED = "subscription.updated"
TRADE_AUTO_MATCHED = "trade.auto_matched"
TRADE_CANCELLED = "trade.cancelled"
TRADE_CONFIRMED = "trade.confirmed"
TRADE_CREATED = "trade.created"
TRADE_DECLINED = "trade.declined"
TRADE_DELIVERED = "trade.delivered"
TRADE_PAID = "trade.paid"
USER_PASSWORD_CHANGED = "user.password_changed"
USER_PASSWORD_RESET_COMPLETED = "user.password_reset_completed"
USER_PASSWORD_RESET_REQUESTED = "user.password_reset_requested"
USER_REGISTERED = "user.registered"

# Login success/failure is deliberately excluded: the event volume would drown
# financially and compliance-relevant audit trail entries.
AUDIT_ACTIONS = frozenset(
    {
        ADMIN_USER_APPROVED,
        ADMIN_ORGANIZATION_APPROVED,
        ADMIN_ORGANIZATION_REJECTED,
        ORGANIZATION_JOIN_REQUESTED,
        ORGANIZATION_JOIN_APPROVED,
        ORGANIZATION_JOIN_REJECTED,
        ADMIN_USER_REJECTED,
        COMMISSION_UPDATED,
        INVENTORY_PUBLISHED,
        MARKET_ACCESS_INVALIDATED,
        KYC_APPROVED,
        KYC_REJECTED,
        KYC_SUBMITTED,
        KYC_MEMBERSHIP_INVALIDATED,
        NEGOTIATION_COUNTERED,
        NEGOTIATION_CREATED,
        NEGOTIATION_DECLINED,
        ORDER_CANCELLED,
        ORDER_CREATED,
        ORDER_EXPIRED,
        ORDER_UPDATED,
        RFQ_CANCELLED,
        RFQ_CREATED,
        RFQ_QUOTE_SUBMITTED,
        RFQ_QUOTE_WITHDRAWN,
        SUBSCRIPTION_UPDATED,
        TRADE_AUTO_MATCHED,
        TRADE_CANCELLED,
        TRADE_CONFIRMED,
        TRADE_CREATED,
        TRADE_DECLINED,
        TRADE_DELIVERED,
        TRADE_PAID,
        USER_PASSWORD_CHANGED,
        USER_PASSWORD_RESET_COMPLETED,
        USER_PASSWORD_RESET_REQUESTED,
        USER_REGISTERED,
    }
)
