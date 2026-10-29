"""Advance public cache generations for changed rows and retain a bounded window."""
import sqlalchemy as sa

from alembic import op

revision = "prc_20261026_changes"
down_revision = "prc_20261018_cache"
branch_labels = None
depends_on = None

DEPENDENCIES = (
    "orderbook_orders", "trades", "products", "delivery_points", "users",
    "organizations", "kyc_execution_grace", "market_row_quarantines",
    "organization_market_approvals", "market_indications", "fair_price_bands",
    "physical_stems", "market_signal_ingestion_runs", "benchmarks",
    "live_slice_benchmarks", "market_event_outbox",
)

# Sixteen listener batches of 256 generations allow bounded catch-up. A reader
# behind this window clears all trading entries and reads the current generation.
EVENT_RETENTION_GENERATIONS = 4096


def upgrade():
    connection = op.get_bind()
    connection.execute(sa.text(
        "LOCK TABLE " + ", ".join(f'public."{table}"' for table in DEPENDENCIES)
        + " IN SHARE ROW EXCLUSIVE MODE NOWAIT"))
    # Readers are compatible with the held lock. Refuse an existing reader
    # explicitly before DDL; a reader arriving later remains unblocked because
    # no dependency statement requests a stronger lock.
    active_reader = connection.execute(sa.text("""
        SELECT EXISTS (
            SELECT 1 FROM pg_catalog.pg_locks l
            WHERE l.locktype = 'relation'
                AND l.database = (SELECT oid FROM pg_catalog.pg_database
                                  WHERE datname = current_database())
                AND l.relation = ANY (
                    SELECT to_regclass('public.' || name)::oid
                    FROM unnest(CAST(:dependencies AS text[])) AS dependency(name)
                )
                AND l.mode = 'AccessShareLock' AND l.granted
                AND l.pid IS DISTINCT FROM pg_backend_pid()
        )
    """), {"dependencies": list(DEPENDENCIES)}).scalar()
    if active_reader:
        raise RuntimeError("cache dependency has an active reader; retry the upgrade")
    for table in DEPENDENCIES:
        ordinary = connection.execute(sa.text("""
            SELECT c.relkind = 'r' AND NOT c.relispartition
                AND NOT EXISTS (
                    SELECT 1 FROM pg_catalog.pg_inherits
                    WHERE inhrelid = c.oid OR inhparent = c.oid
                )
            FROM pg_catalog.pg_class c
            WHERE c.oid = to_regclass(:name)
        """), {"name": "public." + table}).scalar()
        if ordinary is not True:
            raise RuntimeError("cache transition tables require an ordinary standalone table: " + table)

    # CREATE OR REPLACE preserves the migrator owner and existing private ACL.
    # TRUNCATE has no transition relation and always advances the generation.
    op.execute(f"""
        CREATE OR REPLACE FUNCTION public.bump_public_read_cache_generation() RETURNS trigger
        LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, public AS $$
        DECLARE next_generation bigint;
        BEGIN
            IF TG_OP <> 'TRUNCATE' THEN
                IF NOT EXISTS (SELECT 1 FROM cache_changed_rows) THEN
                    RETURN NULL;
                END IF;
            END IF;
            UPDATE public.public_read_cache_generations
            SET generation = generation + 1 WHERE domain = 'trading'
            RETURNING generation INTO STRICT next_generation;
            INSERT INTO public.public_read_cache_events(domain,generation)
            VALUES ('trading',next_generation);
            DELETE FROM public.public_read_cache_events
            WHERE domain = 'trading'
                AND generation <= next_generation - {EVENT_RETENTION_GENERATIONS};
            PERFORM pg_notify('verdaxis_public_read_cache','trading');
            RETURN NULL;
        END $$;
    """)
    op.execute("REVOKE ALL ON FUNCTION public.bump_public_read_cache_generation() FROM PUBLIC")
    for table in DEPENDENCIES:
        # Keep the original trigger disabled: DROP TRIGGER takes a stronger
        # dependency lock than this checkpoint permits.
        op.execute(f'ALTER TABLE public."{table}" DISABLE TRIGGER public_read_cache_changed')
        for event, referencing in (
            ("INSERT", "REFERENCING NEW TABLE AS cache_changed_rows"),
            ("UPDATE", "REFERENCING NEW TABLE AS cache_changed_rows"),
            ("DELETE", "REFERENCING OLD TABLE AS cache_changed_rows"),
            ("TRUNCATE", ""),
        ):
            name = "public_read_cache_changed_" + event.lower()
            op.execute(f'CREATE TRIGGER {name} AFTER {event} ON public."{table}" '
                       f'{referencing} FOR EACH STATEMENT '
                       'EXECUTE FUNCTION public.bump_public_read_cache_generation()')
            op.execute(f'ALTER TABLE public."{table}" ENABLE ALWAYS TRIGGER {name}')

    # Bound an existing installation without advancing or resetting its generation.
    op.execute(f"""
        DELETE FROM public.public_read_cache_events
        WHERE domain = 'trading' AND generation <= (
            SELECT generation - {EVENT_RETENTION_GENERATIONS}
            FROM public.public_read_cache_generations WHERE domain = 'trading'
        )
    """)


def downgrade():
    raise RuntimeError("public read cache checkpoint requires a reviewed forward revert")
