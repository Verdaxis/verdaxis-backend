"""Add transaction-owned public-read cache invalidation metadata.

No cache activation: external authority and connection fencing are separate gates.
"""
import sqlalchemy as sa

from alembic import op

revision = "prc_20261018_cache"
down_revision = "p0_20261016_union"
branch_labels = None
depends_on = None

DEPENDENCIES = (
    "orderbook_orders", "trades", "products", "delivery_points", "users",
    "organizations", "kyc_execution_grace", "market_row_quarantines",
    "organization_market_approvals", "market_indications", "fair_price_bands",
    "physical_stems", "market_signal_ingestion_runs", "benchmarks",
    "live_slice_benchmarks", "market_event_outbox",
)


def upgrade():
    connection = op.get_bind()
    # CREATE TRIGGER needs SHARE ROW EXCLUSIVE. Taking every lock up front without
    # waiting fails fast behind an active writer, rather than queueing behind it
    # and blocking every later writer. Readers are not blocked.
    connection.execute(sa.text(
        "LOCK TABLE " + ", ".join(f'public."{table}"' for table in DEPENDENCIES)
        + " IN SHARE ROW EXCLUSIVE MODE NOWAIT"))
    op.create_table("public_read_cache_generations",
        sa.Column("domain", sa.Text(), primary_key=True),
        sa.Column("generation", sa.BigInteger(), nullable=False, server_default="0"),
        sa.CheckConstraint("generation >= 0", name="ck_public_cache_generation_nonnegative"))
    op.create_table("public_read_cache_events",
        sa.Column("domain", sa.Text(), primary_key=True),
        sa.Column("generation", sa.BigInteger(), primary_key=True),
        sa.CheckConstraint("generation > 0", name="ck_public_cache_event_positive"))
    op.create_table("public_read_cache_incarnation",
        sa.Column("singleton", sa.Boolean(), primary_key=True),
        sa.Column("database_epoch", sa.Text(), nullable=False),
        sa.Column("primary_identity", sa.Text(), nullable=False),
        sa.Column("authority_revision", sa.BigInteger(), nullable=False, server_default="0"),
        sa.Column("active", sa.Boolean(), nullable=False, server_default=sa.false()),
        sa.CheckConstraint("singleton", name="ck_public_cache_singleton"))
    op.execute("INSERT INTO public.public_read_cache_generations(domain,generation) VALUES ('trading',0)")
    # The migrator owns this metadata. A copied epoch alone never proves primary
    # authority; activation remains false until an external fencing owner attests it.
    op.execute("INSERT INTO public.public_read_cache_incarnation(singleton,database_epoch,primary_identity) VALUES (true,gen_random_uuid()::text,gen_random_uuid()::text)")
    op.execute("""
        CREATE FUNCTION public.bump_public_read_cache_generation() RETURNS trigger
        LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, public AS $$
        DECLARE next_generation bigint;
        BEGIN
            UPDATE public.public_read_cache_generations
            SET generation = generation + 1 WHERE domain = 'trading'
            RETURNING generation INTO STRICT next_generation;
            INSERT INTO public.public_read_cache_events(domain,generation)
            VALUES ('trading',next_generation);
            PERFORM pg_notify('verdaxis_public_read_cache','trading');
            RETURN NULL;
        END $$;
    """)
    op.execute("REVOKE ALL ON FUNCTION public.bump_public_read_cache_generation() FROM PUBLIC")
    for table in DEPENDENCIES:
        if connection.execute(sa.text("SELECT to_regclass(:name)"), {"name": "public." + table}).scalar() is None:
            raise RuntimeError("cache dependency table is absent: " + table)
        op.execute(f'CREATE TRIGGER public_read_cache_changed AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON public."{table}" FOR EACH STATEMENT EXECUTE FUNCTION public.bump_public_read_cache_generation()')
        # ALWAYS: freshness must not depend on a session's replication role, which
        # tooling and restores change to skip ordinary triggers.
        op.execute(f'ALTER TABLE public."{table}" ENABLE ALWAYS TRIGGER public_read_cache_changed')


def downgrade():
    # This additive checkpoint must be forward-reverted. Dropping its metadata
    # could make a still-running cache reader accept an old identity/generation.
    raise RuntimeError("public read cache checkpoint requires a reviewed forward revert")
