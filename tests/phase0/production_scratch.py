"""Test-only exact production names in the disposable Phase 0 runner cluster.

Never accepts a URL, live credential or caller-selected database name. The
source is copied from the runner's independently migrated synthetic template.
No production authority check is patched or bypassed by this fixture.
"""
from __future__ import annotations

import os
import secrets
from contextlib import asynccontextmanager
from dataclasses import dataclass

import asyncpg

ROLES = ('verdaxis_app', 'verdaxis_migrator', 'verdaxis_backup')
DATABASES = ('verdaxis', 'verdaxis_prod')


@dataclass
class ProductionScratch:
    port: int
    passwords: dict
    source: dict
    target: dict

    def url(self, database, role):
        return f'postgresql+asyncpg://{role}:{self.passwords[role]}@127.0.0.1:{self.port}/{database}'

    @property
    def source_url(self):
        return self.url('verdaxis', 'verdaxis_backup')

    @property
    def target_url(self):
        return self.url('verdaxis_prod', 'verdaxis_migrator')

    @property
    def app_url(self):
        return self.url('verdaxis_prod', 'verdaxis_app')

    @asynccontextmanager
    async def admin_connection(self, database='postgres'):
        if database not in ('postgres', *DATABASES):
            raise ValueError('Unapproved scratch administrator database')
        connection = await asyncpg.connect(
            host='127.0.0.1', port=self.port, user='postgres',
            password=os.environ['PHASE0_PASSWORD'], database=database,
        )
        try:
            cluster = await connection.fetchval('SELECT system_identifier::text FROM pg_control_system()')
            if cluster != os.environ['PHASE0_CLUSTER_ID']:
                raise ValueError('Disposable cluster identity differs')
            yield connection
        finally:
            await connection.close()


@asynccontextmanager
async def production_scratch():
    port = int(os.environ['PHASE0_PORT'])
    if not 1024 < port < 65536 or port in (5432, 5433):
        raise ValueError('Runner ephemeral port required')
    scratch = ProductionScratch(port, {role: secrets.token_hex(32) for role in ROLES}, {}, {})
    created_databases, created_roles = [], []
    async with scratch.admin_connection() as admin:
        # This check precedes every mutation, including role creation. Never reuse
        # an existing exact-name database or alter its roles to fit the fixture.
        collisions = await admin.fetchval(
            'SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname=ANY($1::text[])) '
            'OR EXISTS (SELECT 1 FROM pg_roles WHERE rolname=ANY($2::text[]))',
            list(DATABASES), list(ROLES),
        )
        if collisions:
            raise ValueError('Production-name scratch collision')
        templates = await admin.fetchval(
            "SELECT count(*) FROM pg_database WHERE datname IN ('phase0_prod_test','phase0_union_test')"
        )
        if templates != 2:
            raise ValueError('Runner synthetic templates required')
        try:
            for role in ROLES:
                # Names are constants and passwords are locally generated hex.
                await admin.execute(f"CREATE ROLE {role} LOGIN NOINHERIT PASSWORD '{scratch.passwords[role]}'")
                created_roles.append(role)
            await admin.execute('CREATE DATABASE verdaxis OWNER verdaxis_migrator TEMPLATE phase0_prod_test')
            created_databases.append('verdaxis')
            await admin.execute('CREATE DATABASE verdaxis_prod OWNER verdaxis_migrator TEMPLATE template1')
            created_databases.append('verdaxis_prod')
            await admin.execute('REVOKE CONNECT ON DATABASE verdaxis_prod FROM PUBLIC')
            async with scratch.admin_connection('verdaxis') as source:
                # Only the new clone changes ownership; never alter the template
                # or use REASSIGN OWNED, which can include shared database objects.
                objects = await source.fetch("""
                    SELECT n.nspname, c.relname, c.relkind FROM pg_class c
                    JOIN pg_namespace n ON n.oid=c.relnamespace
                    WHERE n.nspname='public' AND c.relkind IN ('r','p','S','v','m')
                      AND c.relowner=(SELECT oid FROM pg_roles WHERE rolname='phase0_owner')
                    ORDER BY CASE WHEN c.relkind='S' THEN 1 ELSE 0 END, c.relname
                """)
                for row in objects:
                    kind = {'S': 'SEQUENCE', 'v': 'VIEW', 'm': 'MATERIALIZED VIEW'}.get(row['relkind'], 'TABLE')
                    name = '"' + row['relname'].replace('"', '""') + '"'
                    await source.execute(f'ALTER {kind} public.{name} OWNER TO verdaxis_migrator')
                functions = await source.fetch("""
                    SELECT p.oid::regprocedure::text AS name FROM pg_proc p
                    JOIN pg_namespace n ON n.oid=p.pronamespace
                    WHERE n.nspname='public'
                      AND p.proowner=(SELECT oid FROM pg_roles WHERE rolname='phase0_owner')
                """)
                for row in functions:
                    await source.execute(f'ALTER FUNCTION {row["name"]} OWNER TO verdaxis_migrator')
                await source.execute('GRANT USAGE ON SCHEMA public TO verdaxis_backup')
                await source.execute('GRANT SELECT ON ALL TABLES IN SCHEMA public TO verdaxis_backup')
                await source.execute('GRANT SELECT ON ALL SEQUENCES IN SCHEMA public TO verdaxis_backup')
            for side, database, role in (
                    ('source', 'verdaxis', 'verdaxis_backup'),
                    ('target', 'verdaxis_prod', 'verdaxis_migrator')):
                async with scratch.admin_connection(database) as connection:
                    row = await connection.fetchrow("""
                        SELECT d.oid, inet_server_addr()::text AS server_host,
                               inet_server_port() AS server_port
                        FROM pg_database d WHERE datname=current_database()
                    """)
                    identity = dict(host='127.0.0.1', port=port, database=database,
                                    role=role, cluster_system_identifier=os.environ['PHASE0_CLUSTER_ID'],
                                    **dict(row))
                    setattr(scratch, side, identity)
            yield scratch
        finally:
            for database in reversed(created_databases):
                await admin.execute(f'DROP DATABASE {database} WITH (FORCE)')
            for role in reversed(created_roles):
                await admin.execute(f'DROP ROLE {role}')
