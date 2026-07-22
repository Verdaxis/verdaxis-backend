"""Separate admission, tenant joins, and durable execution ownership."""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql


revision: str = "sec_20260720_boundaries"
down_revision: Union[str, None] = "sec_20260720_identity"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # The active FuelType enum contains values longer than the historical
    # VARCHAR(8). Widen only; this avoids an unrelated runtime insert failure
    # surfaced by the required full metadata check.
    op.alter_column(
        "inventory_items",
        "fuel_type",
        existing_type=sa.String(8),
        type_=sa.String(20),
        existing_nullable=False,
    )
    # Normalize identity before adding the case-insensitive guard. Abort the
    # migration rather than silently merging two existing accounts.
    duplicate = op.get_bind().execute(
        sa.text(
            "SELECT lower(trim(email)) FROM users "
            "GROUP BY lower(trim(email)) HAVING count(*) > 1 LIMIT 1"
        )
    ).scalar_one_or_none()
    if duplicate is not None:
        raise RuntimeError("Cannot normalize users.email: duplicate case-insensitive identities exist")
    op.execute(sa.text("UPDATE users SET email = lower(trim(email))"))
    op.create_index("uq_users_email_lower", "users", [sa.text("lower(email)")], unique=True)

    # The old VERIFIED organization value is semantically the old approval
    # state. Do not promote any pending users, organizations, or KYC records.
    op.execute(
        sa.text("UPDATE organizations SET verification_status = 'APPROVED' WHERE verification_status = 'VERIFIED'")
    )

    # This revision is intentionally a separate deployment step. The prior
    # revision started an explicit 24-hour compatibility window and hashed
    # every plaintext value. Refuse to remove the fallback while any of those
    # links can still be valid; operators must wait or intentionally expire
    # them before continuing.
    bind = op.get_bind()
    unbound_legacy_tokens = bind.execute(
        sa.text(
            "SELECT count(*) FROM users WHERE email_verification_token IS NOT NULL AND ("
            "email_verification_token_hash IS NULL OR "
            "email_verification_token_expires_at IS NULL OR "
            "email_verification_token_hash <> encode("
            "sha256(convert_to(email_verification_token, 'UTF8')), 'hex'))"
        )
    ).scalar_one()
    if unbound_legacy_tokens:
        raise RuntimeError(
            "Legacy email verification tokens are missing a matching hash or expiry; "
            "repair the compatibility invariant before applying sec_20260720_boundaries"
        )
    active_legacy_tokens = bind.execute(
        sa.text(
            "SELECT count(*) FROM users WHERE email_verification_token IS NOT NULL "
            "AND email_verification_token_expires_at > CURRENT_TIMESTAMP"
        )
    ).scalar_one()
    if active_legacy_tokens:
        raise RuntimeError(
            "Legacy email verification compatibility window is still active; "
            "wait until all migration-time expiries pass before applying sec_20260720_boundaries"
        )
    op.execute(sa.text("DROP TRIGGER trg_users_sync_legacy_verification_token ON users"))
    op.execute(sa.text("DROP FUNCTION verdaxis_sync_legacy_verification_token()"))
    op.execute(sa.text("UPDATE users SET email_verification_token = NULL"))
    op.drop_column("users", "email_verification_token")

    op.add_column("users", sa.Column("kyc_external_evidence_reference", sa.String(200), nullable=True))
    op.add_column("users", sa.Column("kyc_review_note", sa.Text(), nullable=True))
    op.add_column("users", sa.Column("kyc_reviewed_by", postgresql.UUID(as_uuid=True), nullable=True))
    op.add_column("users", sa.Column("kyc_reviewed_at", sa.DateTime(timezone=True), nullable=True))
    op.create_foreign_key(
        "fk_users_kyc_reviewed_by", "users", "users", ["kyc_reviewed_by"], ["id"]
    )

    op.add_column("orderbook_orders", sa.Column("owner_user_id", postgresql.UUID(as_uuid=True), nullable=True))
    op.create_foreign_key(
        "fk_orderbook_orders_owner_user_id", "orderbook_orders", "users", ["owner_user_id"], ["id"]
    )
    op.create_index("ix_orderbook_orders_owner_user_id", "orderbook_orders", ["owner_user_id"])

    op.add_column("rfqs", sa.Column("buyer_user_id", postgresql.UUID(as_uuid=True), nullable=True))
    op.create_foreign_key("fk_rfqs_buyer_user_id", "rfqs", "users", ["buyer_user_id"], ["id"])
    op.create_index("ix_rfqs_buyer_user_id", "rfqs", ["buyer_user_id"])
    op.add_column("rfq_quotes", sa.Column("seller_user_id", postgresql.UUID(as_uuid=True), nullable=True))
    op.create_foreign_key("fk_rfq_quotes_seller_user_id", "rfq_quotes", "users", ["seller_user_id"], ["id"])
    op.create_index("ix_rfq_quotes_seller_user_id", "rfq_quotes", ["seller_user_id"])

    op.add_column("refresh_sessions", sa.Column("rotation_grace_until", sa.DateTime(timezone=True), nullable=True))
    op.add_column("trades", sa.Column("buyer_user_id", postgresql.UUID(as_uuid=True), nullable=True))
    op.create_foreign_key("fk_trades_buyer_user_id", "trades", "users", ["buyer_user_id"], ["id"])
    op.create_index("ix_trades_buyer_user_id", "trades", ["buyer_user_id"])
    op.add_column("trades", sa.Column("seller_user_id", postgresql.UUID(as_uuid=True), nullable=True))
    op.create_foreign_key("fk_trades_seller_user_id", "trades", "users", ["seller_user_id"], ["id"])
    op.create_index("ix_trades_seller_user_id", "trades", ["seller_user_id"])

    op.add_column("negotiations", sa.Column("initiator_user_id", postgresql.UUID(as_uuid=True), nullable=True))
    op.add_column("negotiations", sa.Column("counterparty_user_id", postgresql.UUID(as_uuid=True), nullable=True))
    op.add_column("negotiations", sa.Column("accepted_by_user_id", postgresql.UUID(as_uuid=True), nullable=True))
    op.create_foreign_key(
        "fk_negotiations_initiator_user_id", "negotiations", "users", ["initiator_user_id"], ["id"]
    )
    op.create_foreign_key(
        "fk_negotiations_counterparty_user_id", "negotiations", "users", ["counterparty_user_id"], ["id"]
    )
    op.create_foreign_key(
        "fk_negotiations_accepted_by_user_id", "negotiations", "users", ["accepted_by_user_id"], ["id"]
    )
    op.create_index("ix_negotiations_initiator_user_id", "negotiations", ["initiator_user_id"])
    op.create_index("ix_negotiations_counterparty_user_id", "negotiations", ["counterparty_user_id"])
    op.add_column(
        "negotiation_rounds",
        sa.Column("proposer_user_id", postgresql.UUID(as_uuid=True), nullable=True),
    )
    op.create_foreign_key(
        "fk_negotiation_rounds_proposer_user_id",
        "negotiation_rounds",
        "users",
        ["proposer_user_id"],
        ["id"],
    )

    op.create_table(
        "pending_registrations",
        sa.Column("id", postgresql.UUID(as_uuid=True), primary_key=True),
        sa.Column("token_hash", sa.String(64), nullable=False, unique=True),
        sa.Column("email", sa.String(), nullable=False),
        sa.Column("password_hash", sa.String(), nullable=False),
        sa.Column("first_name", sa.String(), nullable=True),
        sa.Column("last_name", sa.String(), nullable=True),
        # Integration note: width 8 matches the non-native UserRole enum the
        # PendingRegistration model declares (longest value SUPPLIER).
        sa.Column("role", sa.String(8), nullable=True),
        sa.Column("referral_code", sa.String(32), nullable=True),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("used_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
    )
    op.create_index("ix_pending_registrations_expires_at", "pending_registrations", ["expires_at"])
    op.create_index("ix_pending_registrations_email", "pending_registrations", ["email"])
    op.create_index(
        "uq_pending_registrations_email_active",
        "pending_registrations",
        [sa.text("lower(email)")],
        unique=True,
        postgresql_where=sa.text("used_at IS NULL"),
    )

    op.create_table(
        "organization_join_requests",
        sa.Column("id", postgresql.UUID(as_uuid=True), primary_key=True),
        sa.Column("user_id", postgresql.UUID(as_uuid=True), sa.ForeignKey("users.id"), nullable=False),
        sa.Column("organization_id", postgresql.UUID(as_uuid=True), sa.ForeignKey("organizations.id"), nullable=False),
        # Integration note: width 8 matches the non-native JoinRequestStatus
        # enum the OrganizationJoinRequest model declares (APPROVED/REJECTED).
        sa.Column("status", sa.String(8), nullable=False, server_default="PENDING"),
        sa.Column("reviewed_by", postgresql.UUID(as_uuid=True), sa.ForeignKey("users.id"), nullable=True),
        sa.Column("reviewed_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("review_note", sa.Text(), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.UniqueConstraint("user_id", "organization_id", name="uq_org_join_user_org"),
    )
    op.create_index("ix_organization_join_requests_user_id", "organization_join_requests", ["user_id"])
    op.create_index("ix_organization_join_requests_organization_id", "organization_join_requests", ["organization_id"])


def downgrade() -> None:
    op.drop_index("ix_organization_join_requests_organization_id", table_name="organization_join_requests")
    op.drop_index("ix_organization_join_requests_user_id", table_name="organization_join_requests")
    op.drop_table("organization_join_requests")
    op.drop_index("uq_pending_registrations_email_active", table_name="pending_registrations")
    op.drop_index("ix_pending_registrations_email", table_name="pending_registrations")
    op.drop_index("ix_pending_registrations_expires_at", table_name="pending_registrations")
    op.drop_table("pending_registrations")
    op.drop_constraint(
        "fk_negotiation_rounds_proposer_user_id", "negotiation_rounds", type_="foreignkey"
    )
    op.drop_column("negotiation_rounds", "proposer_user_id")
    op.drop_index("ix_negotiations_counterparty_user_id", table_name="negotiations")
    op.drop_index("ix_negotiations_initiator_user_id", table_name="negotiations")
    op.drop_constraint("fk_negotiations_accepted_by_user_id", "negotiations", type_="foreignkey")
    op.drop_constraint("fk_negotiations_counterparty_user_id", "negotiations", type_="foreignkey")
    op.drop_constraint("fk_negotiations_initiator_user_id", "negotiations", type_="foreignkey")
    op.drop_column("negotiations", "accepted_by_user_id")
    op.drop_column("negotiations", "counterparty_user_id")
    op.drop_column("negotiations", "initiator_user_id")
    op.drop_index("uq_users_email_lower", table_name="users")
    op.drop_column("refresh_sessions", "rotation_grace_until")
    op.drop_index("ix_trades_seller_user_id", table_name="trades")
    op.drop_constraint("fk_trades_seller_user_id", "trades", type_="foreignkey")
    op.drop_column("trades", "seller_user_id")
    op.drop_index("ix_trades_buyer_user_id", table_name="trades")
    op.drop_constraint("fk_trades_buyer_user_id", "trades", type_="foreignkey")
    op.drop_column("trades", "buyer_user_id")
    op.drop_index("ix_rfq_quotes_seller_user_id", table_name="rfq_quotes")
    op.drop_constraint("fk_rfq_quotes_seller_user_id", "rfq_quotes", type_="foreignkey")
    op.drop_column("rfq_quotes", "seller_user_id")
    op.drop_index("ix_rfqs_buyer_user_id", table_name="rfqs")
    op.drop_constraint("fk_rfqs_buyer_user_id", "rfqs", type_="foreignkey")
    op.drop_column("rfqs", "buyer_user_id")
    op.drop_index("ix_orderbook_orders_owner_user_id", table_name="orderbook_orders")
    op.drop_constraint("fk_orderbook_orders_owner_user_id", "orderbook_orders", type_="foreignkey")
    op.drop_column("orderbook_orders", "owner_user_id")
    op.drop_constraint("fk_users_kyc_reviewed_by", "users", type_="foreignkey")
    op.drop_column("users", "kyc_reviewed_at")
    op.drop_column("users", "kyc_reviewed_by")
    op.drop_column("users", "kyc_review_note")
    op.drop_column("users", "kyc_external_evidence_reference")
    # Downgrade cannot recover discarded plaintext values. Existing users
    # retain their hash/expiry fields from the identity revision and must be
    # sent a new link by an old-code rollback.
    op.add_column("users", sa.Column("email_verification_token", sa.String(), nullable=True))
    op.execute(
        sa.text(
            """
            CREATE FUNCTION verdaxis_sync_legacy_verification_token()
            RETURNS trigger
            LANGUAGE plpgsql
            AS $$
            BEGIN
                IF TG_OP = 'INSERT'
                   OR NEW.email_verification_token IS DISTINCT FROM OLD.email_verification_token THEN
                    IF NEW.email_verification_token IS NULL THEN
                        NEW.email_verification_token_hash := NULL;
                        NEW.email_verification_token_expires_at := NULL;
                    ELSE
                        NEW.email_verification_token_hash := encode(
                            sha256(convert_to(NEW.email_verification_token, 'UTF8')), 'hex'
                        );
                        NEW.email_verification_token_expires_at :=
                            CURRENT_TIMESTAMP + interval '24 hours';
                    END IF;
                END IF;
                RETURN NEW;
            END
            $$
            """
        )
    )
    op.execute(
        sa.text(
            "CREATE TRIGGER trg_users_sync_legacy_verification_token "
            "BEFORE INSERT OR UPDATE OF email_verification_token ON users "
            "FOR EACH ROW EXECUTE FUNCTION verdaxis_sync_legacy_verification_token()"
        )
    )
    # Preserve VARCHAR(20) on rollback. The widening is backward compatible,
    # while narrowing can fail on valid values written after upgrade.
