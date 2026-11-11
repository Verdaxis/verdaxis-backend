-- Both environments adopted the sqlx baseline (sqlx_1) on 7 Nov 2026 and the
-- pre-cutover rollback window is closed, so the retired Alembic history table
-- has no reader left. A database built from the SQL baseline never had it;
-- this removes it from the databases that were adopted from Alembic.
DROP TABLE IF EXISTS public.alembic_version;
