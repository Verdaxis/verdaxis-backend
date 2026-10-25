"""Add trade freeze history and settlement enforcement."""

import sqlalchemy as sa

from alembic import op

revision = "p89_20261022_trade_freeze"
down_revision = "p89_20261022_watchlist_producer"
branch_labels = None
depends_on = None


def upgrade():
    op.execute("""
        CREATE TABLE trade_freezes (
            id uuid PRIMARY KEY,
            trade_id uuid NOT NULL REFERENCES trades(id) ON DELETE RESTRICT,
            status varchar(8) NOT NULL,
            reason_code varchar(32) NOT NULL,
            note varchar(500) NOT NULL,
            frozen_by_user_id uuid NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
            frozen_at timestamptz NOT NULL DEFAULT now(),
            cleared_by_user_id uuid REFERENCES users(id) ON DELETE RESTRICT,
            cleared_at timestamptz,
            clear_note varchar(500),
            version integer NOT NULL DEFAULT 1,
            CONSTRAINT ck_trade_freezes_status CHECK (status IN ('ACTIVE', 'CLEARED')),
            CONSTRAINT ck_trade_freezes_reason CHECK (reason_code IN (
                'PAYMENT_DISPUTE', 'DELIVERY_DISPUTE', 'TERMS_DISPUTE',
                'COMPLIANCE_REVIEW', 'FRAUD_REVIEW', 'COURT_ORDER', 'OTHER')),
            CONSTRAINT ck_trade_freezes_note CHECK (char_length(btrim(note)) BETWEEN 3 AND 500),
            CONSTRAINT ck_trade_freezes_clear CHECK (
                (status = 'ACTIVE' AND cleared_at IS NULL
                    AND cleared_by_user_id IS NULL AND clear_note IS NULL)
                OR (status = 'CLEARED' AND cleared_at IS NOT NULL
                    AND cleared_by_user_id IS NOT NULL AND clear_note IS NOT NULL
                    AND char_length(btrim(clear_note)) BETWEEN 3 AND 500)),
            CONSTRAINT ck_trade_freezes_version CHECK (version >= 1)
        )
    """)
    op.execute("""
        CREATE UNIQUE INDEX uq_trade_freezes_one_active
        ON trade_freezes (trade_id) WHERE status = 'ACTIVE'
    """)
    op.execute("""
        CREATE INDEX ix_trade_freezes_active_frozen_at
        ON trade_freezes (frozen_at DESC, id DESC) WHERE status = 'ACTIVE'
    """)
    op.execute("""
        CREATE FUNCTION trades_freeze_guard() RETURNS trigger
        LANGUAGE plpgsql SET search_path = pg_catalog, public AS $$
        BEGIN
            IF NEW.status IS DISTINCT FROM OLD.status
               AND NEW.status IN ('CONFIRMED', 'DELIVERED', 'PAID', 'DECLINED', 'CANCELLED')
               AND EXISTS (SELECT 1 FROM public.trade_freezes f
                           WHERE f.trade_id = NEW.id AND f.status = 'ACTIVE') THEN
                RAISE EXCEPTION 'trade is frozen' USING ERRCODE = 'ZF001';
            END IF;
            RETURN NEW;
        END $$
    """)
    op.execute("REVOKE ALL ON FUNCTION trades_freeze_guard() FROM PUBLIC")
    op.execute("""
        CREATE TRIGGER trades_freeze_guard BEFORE UPDATE OF status ON trades
        FOR EACH ROW EXECUTE FUNCTION trades_freeze_guard()
    """)
    op.execute("""
        CREATE FUNCTION trade_freezes_history_guard() RETURNS trigger
        LANGUAGE plpgsql SET search_path = pg_catalog, public AS $$
        BEGIN
            IF ROW(NEW.id, NEW.trade_id, NEW.reason_code, NEW.note,
                   NEW.frozen_by_user_id, NEW.frozen_at)
               IS DISTINCT FROM
               ROW(OLD.id, OLD.trade_id, OLD.reason_code, OLD.note,
                   OLD.frozen_by_user_id, OLD.frozen_at) THEN
                RAISE EXCEPTION 'freeze history is immutable' USING ERRCODE = 'ZF002';
            END IF;
            IF NEW IS NOT DISTINCT FROM OLD THEN
                RETURN NEW;
            END IF;
            IF OLD.status = 'CLEARED' THEN
                RAISE EXCEPTION 'cleared freeze is immutable' USING ERRCODE = 'ZF003';
            END IF;
            IF NEW.status <> 'CLEARED' OR NEW.cleared_at IS NULL
               OR NEW.cleared_by_user_id IS NULL OR NEW.clear_note IS NULL
               OR char_length(btrim(NEW.clear_note)) NOT BETWEEN 3 AND 500
               OR NEW.version <= OLD.version THEN
                RAISE EXCEPTION 'complete freeze clearance is required' USING ERRCODE = 'ZF002';
            END IF;
            RETURN NEW;
        END $$
    """)
    op.execute("REVOKE ALL ON FUNCTION trade_freezes_history_guard() FROM PUBLIC")
    op.execute("""
        CREATE TRIGGER trade_freezes_history_guard BEFORE UPDATE ON trade_freezes
        FOR EACH ROW EXECUTE FUNCTION trade_freezes_history_guard()
    """)


def downgrade():
    # Lock before the retained-history check so concurrent inserts cannot pass it.
    op.execute("LOCK TABLE trades, trade_freezes IN ACCESS EXCLUSIVE MODE NOWAIT")
    retained = op.get_bind().execute(
        sa.text("SELECT EXISTS (SELECT 1 FROM trade_freezes)")
    ).scalar_one()
    if retained:
        raise RuntimeError("Retained trade freeze history prevents downgrade")
    op.execute("DROP TRIGGER trades_freeze_guard ON trades")
    op.execute("DROP TRIGGER trade_freezes_history_guard ON trade_freezes")
    op.execute("DROP FUNCTION trades_freeze_guard()")
    op.execute("DROP FUNCTION trade_freezes_history_guard()")
    op.drop_table("trade_freezes")
