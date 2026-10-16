"""Run explicitly inside the isolated root-owned Linux container only."""
import hashlib
import json
from pathlib import Path
from uuid import uuid4

import pytest

from tests.phase0.production_cli_case import invoke, make_case
from tests.phase0.production_scratch import production_scratch


@pytest.mark.asyncio
async def test_production_bootstrap_copy_verify_cli_chain():
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        result = await invoke('bootstrap', directory, common, environment)
        if result.returncode != 0:
            # Synthetic-only diagnostic, not a production CLI logging bypass.
            import os
            from unittest.mock import patch

            from scripts.phase0.production import run
            manifest = json.loads((directory / 'bootstrap.json').read_text())
            with patch.dict(os.environ, environment, clear=True):
                await run(manifest, directory / 'bootstrap-source.env', directory / 'bootstrap-target.env', apply=True)
        assert result.returncode == 0, result.stderr
        receipt = Path(common['bootstrap_receipt'])
        assert receipt.is_file()
        common['bootstrap_receipt_sha256'] = hashlib.sha256(receipt.read_bytes()).hexdigest()
        result = await invoke('copy', directory, common, environment)
        failure_path = directory / 'copy-report.failure.json'
        diagnostic = failure_path.read_text() if failure_path.exists() else result.stderr
        assert result.returncode == 0, diagnostic
        result = await invoke('verify', directory, common, environment)
        assert result.returncode == 0, result.stderr
        report = json.loads((directory / 'verify-report.json').read_text())
        assert report['source_fingerprint'] == common['frozen_source']
        assert len(report['verification']['tables']) == 57
        assert (await invoke('copy', directory, common, environment)).returncode != 0


@pytest.mark.asyncio
@pytest.mark.parametrize('fault', ['host', 'release', 'bytes', 'source_oid', 'target_oid',
                                  'cluster', 'source_role', 'target_role', 'acl', 'freeze'])
async def test_production_bootstrap_wrong_authority_refuses_before_ddl(fault):
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        if fault == 'host':
            common['hostname'] += '-wrong'
        elif fault == 'release':
            common['release_sha'] = '0' * 40
        elif fault == 'bytes':
            common['tool_tree_sha256'] = '0' * 64
        elif fault.endswith('_oid'):
            common[fault.split('_')[0]]['oid'] += 1
        elif fault == 'cluster':
            common['target']['cluster_system_identifier'] = '1'
        elif fault.endswith('_role'):
            common[fault.split('_')[0]]['role'] = 'verdaxis_app'
        elif fault == 'acl':
            common['target_datacl'] = '{}'
        else:
            common['frozen_source']['tables']['users']['count'] += 1
        result = await invoke('bootstrap', directory, common, environment)
        assert result.returncode == 1
        assert 'Bootstrap refused' in result.stderr
        assert 'unrecognized arguments' not in result.stderr
        assert not (directory / 'bootstrap-report.json').exists()
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval("SELECT to_regclass('public.users')") is None
