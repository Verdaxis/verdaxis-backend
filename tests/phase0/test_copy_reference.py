"""Reference verification includes the complete target-only baseline."""
import pytest
from sqlalchemy import text

from tests.phase0.test_copy import copy_case as copy_case


@pytest.mark.asyncio
@pytest.mark.parametrize('table', ['products', 'delivery_points'])
@pytest.mark.parametrize('damage', ['missing', 'changed'])
async def test_verification_requires_exact_extra_baseline(copy_case, table, damage):
    from scripts.phase0.copy import copy_database
    from scripts.phase0.verify import verify_database
    manifest, source, target, _ = copy_case
    if table == 'products':
        identity = 'e561e43f-d9b2-598e-981c-f1d28d515ddc'  # Target-only UCOME.
    else:
        # This provisional migration has no delivery baseline. Supply a bounded
        # target-only fixture to exercise the same final-union verification path.
        from scripts.phase0.manifest import inspect_catalog
        identity = '00000000-0000-4000-8000-000000000099'
        await target.execute(text("""
            INSERT INTO delivery_points (id,name,region,timezone,is_active)
            VALUES (CAST(:id AS uuid),'Synthetic baseline','Asia','Asia/Singapore',true)
        """), {'id': identity})
        baseline = await target.run_sync(inspect_catalog)
        manifest['execution']['baseline_reference_rows'] = baseline['reference_rows']
        await target.commit()
    await copy_database(manifest, source, target)
    report = await verify_database(manifest, source, target)
    assert report.new_reference_rows[table]['count'] >= 1
    if damage == 'missing':
        sql = f'DELETE FROM {table} WHERE id=CAST(:id AS uuid)'
    else:
        sql = f'UPDATE {table} SET is_active=false WHERE id=CAST(:id AS uuid)'
    await target.execute(text(sql), {'id': identity})
    with pytest.raises(ValueError, match='reference'):
        await verify_database(manifest, source, target)
