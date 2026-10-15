"""Independent recovery must execute the documented CLI, not only a helper."""
import asyncio
import copy
import json
import os
import subprocess
from pathlib import Path

import pytest

from scripts.phase0.manifest import digest
from scripts.phase0.verify import fingerprint_database
from tests.phase0.test_copy import copy_case  # noqa: F401

ROOT = Path(__file__).resolve().parents[2]


async def prepare_cli(case, tmp_path, *, receipt_failure=False):
    from scripts.phase0.copy import CopyRefused, copy_database, private_json

    manifest, source, target, _ = case
    frozen = await fingerprint_database(source, manifest['source_catalog'])
    await source.rollback()
    if receipt_failure:
        with pytest.raises(CopyRefused):
            await copy_database(manifest, source, target, fail_at='before_receipt')
    else:
        await copy_database(manifest, source, target)
    verification = copy.deepcopy(manifest)
    verification['execution']['frozen_source'] = frozen
    verification['execution']['source_catalog_sha256'] = digest(manifest['source_catalog'])
    private_json(tmp_path / 'verify.json', verification)
    for side, connection in (('source', source), ('target', target)):
        path = tmp_path / f'verify-{side}.env'
        path.write_text('DATABASE_URL=' + connection.engine.url.render_as_string(hide_password=False) + '\n')
        path.chmod(0o600)
    # Close both caller connections: verification must use independent sessions.
    await source.close()
    await target.close()
    await source.engine.dispose()
    await target.engine.dispose()
    return verification


async def run_cli(tmp_path, *extra):
    command = [str(ROOT / 'venv/bin/python'), '-m', 'scripts.phase0.verify',
               '--manifest', str(tmp_path / 'verify.json'),
               '--source-environment-file', str(tmp_path / 'verify-source.env'),
               '--target-environment-file', str(tmp_path / 'verify-target.env'),
               '--report', str(tmp_path / 'verified.json'), *extra]
    return await asyncio.to_thread(subprocess.run, command, cwd=ROOT, capture_output=True, text=True)


@pytest.mark.asyncio
async def test_documented_verify_command_recovers_missing_receipt(copy_case, tmp_path):
    manifest = await prepare_cli(copy_case, tmp_path, receipt_failure=True)
    result = await run_cli(tmp_path)
    assert result.returncode == 0, result.stderr
    report_path = tmp_path / 'verified.json'
    assert report_path.is_file(), 'CLI returned success without a verification report'
    report = json.loads(report_path.read_text())
    assert report['source_fingerprint'] == manifest['execution']['frozen_source']
    assert len(report['verification']['tables']) == 57
    assert report['sequences']['market_event_stream_seq']['next_value'] == 151
    assert report_path.stat().st_mode & 0o777 == 0o600
    assert not Path(manifest['execution']['receipt_path']).exists()
    assert (await run_cli(tmp_path)).returncode != 0


@pytest.mark.asyncio
@pytest.mark.parametrize('fault', ['row', 'reference', 'trigger', 'sequence', 'source', 'oid', 'cluster', 'frozen', 'catalog', 'report', 'unknown'])
async def test_verify_cli_refuses_corruption(copy_case, tmp_path, fault):
    manifest = await prepare_cli(copy_case, tmp_path)
    admin = copy_case[3]
    if fault in ('oid', 'cluster', 'frozen', 'catalog'):
        if fault == 'oid':
            manifest['execution']['target']['oid'] += 1
        elif fault == 'cluster':
            manifest['execution']['target']['cluster_system_identifier'] = '1'
        elif fault == 'frozen':
            manifest['execution']['frozen_source']['tables']['users']['count'] += 1
        else:
            manifest['execution']['source_catalog_sha256'] = '0' * 64
        (tmp_path / 'verify.json').write_text(json.dumps(manifest))
    elif fault == 'report':
        (tmp_path / 'verified.json').mkdir()
    elif fault != 'unknown':
        import asyncpg
        name = manifest['execution']['source' if fault == 'source' else 'target']['database']
        connection = await asyncpg.connect(host='127.0.0.1', port=int(os.environ['PHASE0_PORT']),
                                         user='postgres', password=os.environ['PHASE0_PASSWORD'], database=name)
        try:
            statements = {
                'row': "UPDATE notifications SET title='changed'",
                'source': "UPDATE notifications SET title='changed'",
                'reference': "UPDATE products SET spec_description='changed'",
                'trigger': 'ALTER TABLE trades DISABLE TRIGGER USER',
                'sequence': "SELECT nextval('market_event_stream_seq')",
            }
            await connection.execute(statements[fault])
        finally:
            await connection.close()
    result = await run_cli(tmp_path, *(['--unreviewed-option'] if fault == 'unknown' else []))
    assert result.returncode != 0, f'{fault}: false success'
    assert not (tmp_path / 'verified.json').is_file()
    assert os.environ['PHASE0_PASSWORD'] not in result.stdout + result.stderr


def test_verify_cli_requires_inputs(tmp_path):
    result = subprocess.run([str(ROOT / 'venv/bin/python'), '-m', 'scripts.phase0.verify'],
                            cwd=ROOT, capture_output=True, text=True)
    assert result.returncode != 0
