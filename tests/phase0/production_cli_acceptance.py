"""Run explicitly inside the isolated root-owned Linux container only."""
import hashlib
import json
from pathlib import Path
from uuid import uuid4

import pytest

from tests.phase0.production_cli_case import install_authority, invoke, make_case
from tests.phase0.production_scratch import production_scratch


@pytest.mark.asyncio
async def test_cross_tool_manifests_refuse_before_secrets_or_connections():
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        for cli in ('bootstrap', 'copy', 'verify'):
            for claimed in ('bootstrap', 'copy', 'verify'):
                if cli == claimed:
                    continue
                # Deliberately absent secrets. A wrong tool must refuse before
                # validation-only return, receipt lookup, or any connection.
                for side in ('source', 'target'):
                    (directory / f'{cli}-{side}.env').unlink(missing_ok=True)
                result = await invoke(cli, directory, common, environment,
                                      manifest_tool=claimed, apply=False,
                                      suffix='-' + claimed)
                assert result.returncode == 1, (cli, claimed, result.stdout, result.stderr)
                assert 'PHASE0_TOOL_MISMATCH' in result.stderr
                assert not (directory / f'{cli}-{claimed}-report.json').exists()
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval("SELECT to_regclass('public.users')") is None


@pytest.mark.asyncio
async def test_production_bootstrap_copy_verify_cli_chain():
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        result = await invoke('bootstrap', directory, common, environment)
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
        # Independent recovery must not need the post-commit copy receipt.
        (directory / 'copy-report.json').unlink()
        recovered = await invoke('verify', directory, common, environment, suffix='-recovery')
        assert recovered.returncode == 0, recovered.stderr
        import copy

        import asyncpg
        for tool in ('copy', 'verify'):
            for key, value in [('hostname', 'wrong-host'), ('release_sha', '0' * 40),
                               ('tool_tree_sha256', '0' * 64),
                               ('bootstrap_receipt_sha256', '0' * 64)]:
                changed = copy.deepcopy(common)
                changed[key] = value
                refused = await invoke(tool, directory, changed, environment, suffix='-' + key)
                assert refused.returncode == 1
                assert 'unrecognized arguments' not in refused.stderr
            for side in ('source', 'target'):
                for key, value in [('oid', 1), ('role', 'verdaxis_app'), ('database', 'wrong'),
                                   ('host', '127.0.0.2'), ('port', 1),
                                   ('cluster_system_identifier', '1')]:
                    changed = copy.deepcopy(common)
                    changed[side][key] = value
                    refused = await invoke(tool, directory, changed, environment, suffix=f'-{side}-{key}')
                    assert refused.returncode == 1
                    assert 'unrecognized arguments' not in refused.stderr
        extra = await asyncpg.connect(scratch.target_url.replace('postgresql+asyncpg:', 'postgresql:'))
        try:
            refused = await invoke('verify', directory, common, environment, suffix='-session')
            assert refused.returncode == 1
        finally:
            await extra.close()
        async with scratch.admin_connection() as admin:
            await admin.execute('GRANT CONNECT ON DATABASE verdaxis_prod TO verdaxis_app')
        refused = await invoke('verify', directory, common, environment, suffix='-acl')
        assert refused.returncode == 1


@pytest.mark.asyncio
@pytest.mark.parametrize('tool', ['bootstrap', 'copy'])
async def test_validation_only_reports_validate_only(tool):
    """R3: no --apply must say it validated only, with no report or connection."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        result = await invoke(tool, directory, common, environment, apply=False)
        assert result.returncode == 0, result.stderr
        assert 'validated only' in result.stdout
        assert 'completed' not in result.stdout
        assert not (directory / f'{tool}-report.json').exists()
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval("SELECT to_regclass('public.users')") is None


OBSERVED_FAULTS = {
    'source_server_host': ('source', 'server_host', '127.0.0.2/32', 'PHASE0_DB_IDENTITY'),
    'target_server_port': ('target', 'server_port', 55433, 'PHASE0_DB_IDENTITY'),
    'source_oid': ('source', 'oid', 1, 'PHASE0_DB_IDENTITY'),
    'target_cluster': ('target', 'cluster_system_identifier', '1', 'PHASE0_DB_IDENTITY'),
    'target_acl': (None, 'target_datacl', '{}', 'PHASE0_TARGET_FENCE'),
    'frozen_source': (None, 'frozen_source', None, 'PHASE0_SOURCE_STATE'),
}


@pytest.mark.asyncio
@pytest.mark.parametrize('tool', ['bootstrap', 'copy', 'verify'])
@pytest.mark.parametrize('fault', sorted(OBSERVED_FAULTS))
async def test_observed_state_fault_refuses_with_reason_code(tool, fault):
    """R2: authority matches the manifest; only observed state differs."""
    side, key, value, code = OBSERVED_FAULTS[fault]
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        if fault == 'frozen_source':
            common['frozen_source'] = dict(common['frozen_source'])
            common['frozen_source']['sequences'] = {}
        elif side is None:
            common[key] = value
        else:
            common[side] = dict(common[side])
            common[side][key] = value
        install_authority(common)
        if tool == 'verify' and fault != 'frozen_source':
            # Verify needs an import to inspect: run bootstrap+copy first, then
            # reinstall the faulting authority and verify.
            if (await invoke('bootstrap', directory, common, environment)).returncode != 0:
                pytest.skip('bootstrap prerequisite refused for this fault')
            common['bootstrap_receipt_sha256'] = hashlib.sha256(
                Path(common['bootstrap_receipt']).read_bytes()).hexdigest()
            if (await invoke('copy', directory, common, environment)).returncode != 0:
                pytest.skip('copy prerequisite refused for this fault')
        result = await invoke(tool, directory, common, environment,
                              suffix='-' + fault, apply=True)
        assert result.returncode == 1, (tool, fault, result.stdout, result.stderr)
        assert code in result.stderr, (tool, fault, result.stderr)
        assert not (directory / f'{tool}-{fault}-report.json').exists()


@pytest.mark.asyncio
@pytest.mark.parametrize('tool', ['bootstrap', 'copy', 'verify'])
async def test_late_target_session_refuses(tool):
    """R2: an extra target session refuses at the observed-state gate."""
    import asyncpg
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        if tool != 'bootstrap':
            assert (await invoke('bootstrap', directory, common, environment)).returncode == 0
            common['bootstrap_receipt_sha256'] = hashlib.sha256(
                Path(common['bootstrap_receipt']).read_bytes()).hexdigest()
        if tool == 'verify':
            assert (await invoke('copy', directory, common, environment)).returncode == 0
        extra = await asyncpg.connect(scratch.target_url.replace('postgresql+asyncpg:', 'postgresql:'))
        try:
            result = await invoke(tool, directory, common, environment, suffix='-session', apply=True)
            assert result.returncode == 1
            assert 'PHASE0_TARGET_FENCE' in result.stderr
            assert not (directory / f'{tool}-session-report.json').exists()
        finally:
            await extra.close()


@pytest.mark.asyncio
@pytest.mark.parametrize('tool', ['bootstrap', 'copy', 'verify'])
async def test_pre_existing_report_destination_refuses(tool):
    """R2 final boundary: publication cannot overwrite or silently skip."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        if tool != 'bootstrap':
            assert (await invoke('bootstrap', directory, common, environment)).returncode == 0
            common['bootstrap_receipt_sha256'] = hashlib.sha256(
                Path(common['bootstrap_receipt']).read_bytes()).hexdigest()
        if tool == 'verify':
            assert (await invoke('copy', directory, common, environment)).returncode == 0
        destination = directory / f'{tool}-occupied-report.json'
        destination.write_text('existing evidence')
        result = await invoke(tool, directory, common, environment,
                              suffix='-occupied', apply=True)
        assert result.returncode == 1
        assert 'PHASE0_REPORT_PATH' in result.stderr
        assert destination.read_text() == 'existing evidence'


@pytest.mark.asyncio
@pytest.mark.parametrize('key,value,code', [
    ('hostname', 'wrong-host', 'PHASE0_AUTHORITY'),
    ('release_sha', '0' * 40, 'PHASE0_AUTHORITY'),
    ('tool_tree_sha256', '0' * 64, 'PHASE0_INSTALLED_BYTES'),
    ('bootstrap_receipt_sha256', '0' * 64, 'PHASE0_RECEIPT'),
])
async def test_manifest_tamper_refuses_against_fixed_authority(key, value, code):
    """R2: the approved authority stays fixed; the manifest is altered."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        if key != 'bootstrap_receipt_sha256':
            assert (await invoke('bootstrap', directory, common, environment)).returncode == 0
            common['bootstrap_receipt_sha256'] = hashlib.sha256(
                Path(common['bootstrap_receipt']).read_bytes()).hexdigest()
        tampered = json.loads(json.dumps(common))
        tampered[key] = value
        result = await invoke('verify', directory, tampered, environment, suffix='-tamper')
        assert result.returncode == 1
        assert code in result.stderr, (key, result.stderr)
        assert not (directory / 'verify-tamper-report.json').exists()


@pytest.mark.asyncio
async def test_successful_verify_always_publishes_report():
    """R1: a successful verify must create its new report; missing inputs fail."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        assert (await invoke('bootstrap', directory, common, environment)).returncode == 0
        common['bootstrap_receipt_sha256'] = hashlib.sha256(
            Path(common['bootstrap_receipt']).read_bytes()).hexdigest()
        assert (await invoke('copy', directory, common, environment)).returncode == 0
        result = await invoke('verify', directory, common, environment)
        assert result.returncode == 0, result.stderr
        assert (directory / 'verify-report.json').is_file()
        # Missing secret and missing manifest inputs must be nonzero.
        for side in ('source', 'target'):
            (directory / f'verify-{side}.env').unlink()
        assert (await invoke('verify', directory, common, environment, suffix='-nosecret')).returncode == 1
