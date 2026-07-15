"""Canonical audit action registry."""

ADMIN_USER_APPROVED = "admin.user_approved"
ADMIN_USER_REJECTED = "admin.user_rejected"
COMMISSION_UPDATED = "commission.updated"
INVENTORY_PUBLISHED = "inventory.published"
KYC_APPROVED = "kyc.approved"
KYC_REJECTED = "kyc.rejected"
KYC_SUBMITTED = "kyc.submitted"
NEGOTIATION_ACCEPTED = "negotiation.accepted"
NEGOTIATION_COUNTERED = "negotiation.countered"
NEGOTIATION_CREATED = "negotiation.created"
NEGOTIATION_DECLINED = "negotiation.declined"
ORDER_CANCELLED = "order.cancelled"
ORDER_CREATED = "order.created"
ORDER_UPDATED = "order.updated"
RFQ_ACCEPTED = "rfq.accepted"
RFQ_CANCELLED = "rfq.cancelled"
RFQ_CREATED = "rfq.created"
RFQ_QUOTE_SUBMITTED = "rfq.quote_submitted"
SUBSCRIPTION_UPDATED = "subscription.updated"
TRADE_AUTO_MATCHED = "trade.auto_matched"
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
        ADMIN_USER_REJECTED,
        COMMISSION_UPDATED,
        INVENTORY_PUBLISHED,
        KYC_APPROVED,
        KYC_REJECTED,
        KYC_SUBMITTED,
        NEGOTIATION_ACCEPTED,
        NEGOTIATION_COUNTERED,
        NEGOTIATION_CREATED,
        NEGOTIATION_DECLINED,
        ORDER_CANCELLED,
        ORDER_CREATED,
        ORDER_UPDATED,
        RFQ_ACCEPTED,
        RFQ_CANCELLED,
        RFQ_CREATED,
        RFQ_QUOTE_SUBMITTED,
        SUBSCRIPTION_UPDATED,
        TRADE_AUTO_MATCHED,
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
