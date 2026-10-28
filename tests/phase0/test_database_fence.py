"""Production database fence checks never mutate source authority or HBA."""
import asyncpg
import pytest
from sqlalchemy.ext.asyncio import create_async_engine
from sqlalchemy.pool import NullPool

from tests.phase0.production_scratch import production_scratch


async def target_acl(scratch):
    async with scratch.admin_connection() as admin:
        return await admin.fetchval("SELECT datacl::text FROM pg_database WHERE datname='verdaxis_prod'")


@pytest.mark.asyncio
async def test_target_fence_denies_runtime_connection():
    async with production_scratch() as scratch:
        with pytest.raises(asyncpg.InsufficientPrivilegeError):
            connection = await asyncpg.connect(scratch.app_url.replace('postgresql+asyncpg:', 'postgresql:'))
            await connection.close()


@pytest.mark.asyncio
@pytest.mark.parametrize('fault', ['none', 'extra_session', 'changed_acl', 'wrong_oid'])
async def test_exact_target_fence_observation(fault):
    from scripts.phase0.database_fence import check_target_fence

    async with production_scratch() as scratch:
        expected = await target_acl(scratch)
        engine = create_async_engine(scratch.target_url, poolclass=NullPool)
        extra = None
        try:
            async with engine.connect() as target:
                identity = dict(scratch.target)
                if fault == 'extra_session':
                    extra = await asyncpg.connect(scratch.target_url.replace('postgresql+asyncpg:', 'postgresql:'))
                elif fault == 'changed_acl':
                    async with scratch.admin_connection() as admin:
                        await admin.execute('GRANT CONNECT ON DATABASE verdaxis_prod TO verdaxis_app')
                elif fault == 'wrong_oid':
                    identity['oid'] += 1
                if fault == 'none':
                    await check_target_fence(target, identity, expected)
                else:
                    with pytest.raises(ValueError):
                        await check_target_fence(target, identity, expected)
        finally:
            if extra:
                await extra.close()
            await engine.dispose()


@pytest.mark.asyncio
async def test_target_fence_rejects_unsafe_acl_even_when_expected():
    from scripts.phase0.database_fence import check_target_fence

    async with production_scratch() as scratch:
        async with scratch.admin_connection() as admin:
            await admin.execute('GRANT CONNECT ON DATABASE verdaxis_prod TO PUBLIC')
        engine = create_async_engine(scratch.target_url, poolclass=NullPool)
        try:
            async with engine.connect() as target:
                with pytest.raises(ValueError):
                    await check_target_fence(target, scratch.target, await target_acl(scratch))
        finally:
            await engine.dispose()


@pytest.mark.asyncio
async def test_source_writer_gate_refuses_every_other_session():
    from scripts.phase0.database_fence import check_source_quiesced

    async with production_scratch() as scratch:
        engine = create_async_engine(scratch.source_url, poolclass=NullPool)
        try:
            async with engine.connect() as source:
                await check_source_quiesced(source, scratch.source)
                extra = await asyncpg.connect(scratch.source_url.replace('postgresql+asyncpg:', 'postgresql:'))
                try:
                    with pytest.raises(ValueError):
                        await check_source_quiesced(source, scratch.source)
                finally:
                    await extra.close()
        finally:
            await engine.dispose()
