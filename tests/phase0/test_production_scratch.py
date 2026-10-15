"""Production identities on synthetic storage in the runner-owned cluster only."""
import os

import asyncpg
import pytest


@pytest.fixture
def scratch_fixture_module():
    from tests.phase0 import production_scratch
    return production_scratch


@pytest.mark.asyncio
async def test_production_named_scratch_identity_and_credentials(scratch_fixture_module):
    async with scratch_fixture_module.production_scratch() as scratch:
        assert scratch.source['database'] == 'verdaxis'
        assert scratch.target['database'] == 'verdaxis_prod'
        assert scratch.source['oid'] != scratch.target['oid']
        assert scratch.source['role'] == 'verdaxis_backup'
        assert scratch.target['role'] == 'verdaxis_migrator'
        assert scratch.source['cluster_system_identifier'] == os.environ['PHASE0_CLUSTER_ID']
        assert len(set(scratch.passwords.values())) == 3
        assert os.environ['PHASE0_PASSWORD'] not in scratch.passwords.values()
        source = await asyncpg.connect(scratch.source_url.replace('postgresql+asyncpg:', 'postgresql:'))
        target = await asyncpg.connect(scratch.target_url.replace('postgresql+asyncpg:', 'postgresql:'))
        try:
            assert await source.fetchval('SELECT version_num FROM alembic_version') == 'kycg_20261014_prod'
            assert await source.fetchval('SELECT count(*) FROM trades') > 0
            with pytest.raises(asyncpg.InsufficientPrivilegeError):
                await source.execute('UPDATE notifications SET title=title WHERE false')
            assert await target.fetchval("SELECT to_regclass('public.users')") is None
            assert await target.fetchval('SELECT current_user') == 'verdaxis_migrator'
            assert await target.fetchval('SELECT system_identifier::text FROM pg_control_system()') == scratch.target['cluster_system_identifier']
        finally:
            await source.close()
            await target.close()


@pytest.mark.asyncio
async def test_production_settings_accept_exact_scratch_split(scratch_fixture_module):
    async with scratch_fixture_module.production_scratch() as scratch:
        from app.config import Settings
        settings = Settings(_env_file=None, ENVIRONMENT='production',
                            RELEASE_SHA='a' * 40,
                            JWT_SECRET='synthetic-production-boundary-secret-123456789',
                            DATABASE_URL=scratch.app_url,
                            MIGRATOR_DATABASE_URL=scratch.target_url,
                            KYC_EXECUTION_GATE_ENABLED=False)
        assert settings.ENVIRONMENT == 'production'
        assert settings.DATABASE_URL == scratch.app_url
        assert settings.MIGRATOR_DATABASE_URL == scratch.target_url
        with pytest.raises(ValueError):
            Settings(_env_file=None, ENVIRONMENT='production', RELEASE_SHA='a' * 40,
                     JWT_SECRET='synthetic-production-boundary-secret-123456789',
                     DATABASE_URL=scratch.source_url, MIGRATOR_DATABASE_URL=scratch.target_url)


@pytest.mark.asyncio
async def test_scratch_name_collision_refuses_without_reuse(scratch_fixture_module):
    async with scratch_fixture_module.production_scratch() as first:
        async with first.admin_connection() as admin:
            before = await admin.fetchval("SELECT oid FROM pg_database WHERE datname='verdaxis'")
        with pytest.raises(ValueError, match='collision'):
            async with scratch_fixture_module.production_scratch():
                pytest.fail('existing database was reused')
        async with first.admin_connection() as admin:
            assert await admin.fetchval("SELECT oid FROM pg_database WHERE datname='verdaxis'") == before


@pytest.mark.asyncio
async def test_scratch_wrong_cluster_refused_before_creation(scratch_fixture_module, monkeypatch):
    monkeypatch.setenv('PHASE0_CLUSTER_ID', '1')
    with pytest.raises(ValueError, match='cluster'):
        async with scratch_fixture_module.production_scratch():
            pytest.fail('wrong cluster accepted')
