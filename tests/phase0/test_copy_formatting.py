"""Copy and verification must not inherit lossy session text formats."""
import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

from tests.phase0.test_copy import copy_case as copy_case


async def set_hostile_formats(connection, date_style):
    await connection.execute(text("SELECT set_config('DateStyle', :style, false)"), {'style': date_style})
    for setting in ("extra_float_digits=0", "IntervalStyle='sql_standard'",
                    "TimeZone='Asia/Singapore'", "bytea_output='escape'"):
        await connection.execute(text(f'SET {setting}'))
    await connection.commit()


@pytest.mark.asyncio
async def test_copy_preserves_float_bytes_and_dates_with_role_overrides(copy_case):
    from scripts.phase0.copy import copy_database
    manifest, source, target, _ = copy_case
    engine = create_async_engine(source.engine.url.set(username='phase0_owner'))
    try:
        async with engine.begin() as owner:
            await owner.execute(text('UPDATE watchlist_targets SET snapshot_price_per_mt_usd=1.2345678901234567'))
            await owner.execute(text("UPDATE user_login_days SET activity_date=DATE '2026-01-02'"))
    finally:
        await engine.dispose()
    await set_hostile_formats(source, 'SQL, DMY')
    await set_hostile_formats(target, 'SQL, MDY')
    await copy_database(manifest, source, target)
    for sql in ("SELECT float8send(snapshot_price_per_mt_usd) FROM watchlist_targets ORDER BY id",
                "SELECT date_send(activity_date) FROM user_login_days ORDER BY id"):
        assert (await source.execute(text(sql))).all() == (await target.execute(text(sql))).all()


@pytest.mark.asyncio
@pytest.mark.parametrize('changed', ['float', 'date'])
async def test_verification_detects_changes_under_lossy_formats(copy_case, changed):
    from scripts.phase0.copy import copy_database
    from scripts.phase0.verify import table_hash
    manifest, source, target, _ = copy_case
    engine = create_async_engine(source.engine.url.set(username='phase0_owner'))
    try:
        async with engine.begin() as owner:
            await owner.execute(text('UPDATE watchlist_targets SET snapshot_price_per_mt_usd=1.2345678901234567'))
            await owner.execute(text("UPDATE user_login_days SET activity_date=DATE '2026-01-02'"))
    finally:
        await engine.dispose()
    await copy_database(manifest, source, target)
    if changed == 'float':
        await target.execute(text('UPDATE watchlist_targets SET snapshot_price_per_mt_usd=1.23456789012346'))
    else:
        await target.execute(text("UPDATE user_login_days SET activity_date=DATE '2026-02-01'"))
    await target.commit()
    await set_hostile_formats(source, 'SQL, DMY')
    await set_hostile_formats(target, 'SQL, MDY')
    table = 'watchlist_targets' if changed == 'float' else 'user_login_days'
    source_hash = await table_hash(source, manifest['source_catalog']['tables'][table])
    target_hash = await table_hash(target, manifest['target_catalog']['tables'][table])
    assert source_hash != target_hash
