"""Run explicitly inside the isolated root-owned Linux container only."""
import asyncio
import hashlib
import json
from pathlib import Path
from uuid import uuid4

import asyncpg
import pytest

from scripts.phase0.bootstrap import BOOTSTRAP_LOCK
from scripts.phase0.copy import IMPORT_LOCK
from tests.phase0.production_cli_case import (
    bump_source_sequence,
    finish_tool,
    grant_target_connect,
    install_authority,
    invoke,
    late_source_write,
    make_case,
    require_committed_chain,
    start_tool,
    wait_for_advisory_lock,
    wait_for_second_source_transaction,
)
from tests.phase0.production_scratch import production_scratch


async def run_tool_with_lock_fault(scratch, tool, directory, common, environment, lock, inject):
    """Start a tool, inject one fault once it holds `lock`, and collect the result.

    The tool takes that lock inside its own transaction, after the entry
    boundary, so a fault injected here can only be refused by a later boundary.
    Every injector closes before the next fence observation, so the source and
    target session gates stay honest.
    """
    process = await start_tool(tool, directory, common, environment)
    try:
        await wait_for_advisory_lock(scratch, process, lock)
        # The check that follows the lock immediately is the pre-import fence.
        # Let it pass so the injection is proved at a later precommit fence.
        await asyncio.sleep(0.5)
        await inject()
        return await finish_tool(process)
    finally:
        if process.returncode is None:
            process.kill()
            await process.wait()


async def run_tool_with_target_session(scratch, tool, directory, common, environment, lock, *, suffix=''):
    """Start a tool, then open one late target session once it holds `lock`.

    The session opens after the fence that immediately follows the lock, and it
    stays open until the tool exits, so only a later in-transaction fence can
    refuse it. This proves the session gate rather than a transient gap.
    """
    process = await start_tool(tool, directory, common, environment, suffix=suffix)
    extra = None
    try:
        await wait_for_advisory_lock(scratch, process, lock)
        await asyncio.sleep(0.5)
        extra = await asyncpg.connect(
            scratch.target_url.replace('postgresql+asyncpg:', 'postgresql:'))
        return await finish_tool(process)
    finally:
        if extra is not None:
            await extra.close()
        if process.returncode is None:
            process.kill()
            await process.wait()


async def run_verify_with_late_fault(scratch, directory, common, environment, inject, *, suffix=''):
    """Start verify, inject one fault once it holds its long source transaction.

    verify owns no advisory lock, so the entry observation is the first source
    transaction and the long repeatable-read read is the second. Injecting after
    that snapshot exists proves the fault can only be refused by a later,
    in-transaction precommit fence or the final fresh observation.
    """
    process = await start_tool('verify', directory, common, environment, suffix=suffix)
    try:
        await wait_for_second_source_transaction(scratch, process)
        await inject()
        return await finish_tool(process)
    finally:
        if process.returncode is None:
            process.kill()
            await process.wait()


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
        # A second copy must not replay. Its own receipt still exists, so the
        # publication gate refuses before any import; committed state is kept.
        replay = await invoke('copy', directory, common, environment)
        assert replay.returncode == 1
        assert 'PHASE0_REPORT_PATH' in replay.stderr
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval('SELECT count(*) FROM trades') > 0
        # Independent recovery must not need the post-commit copy receipt.
        (directory / 'copy-report.json').unlink()
        recovered = await invoke('verify', directory, common, environment, suffix='-recovery')
        assert recovered.returncode == 0, recovered.stderr
        assert (directory / 'verify-recovery-report.json').is_file()


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


# Observed-state faults: manifest and installed authority agree, so the refusal
# must come from the disposable database/cluster state or installed bytes.
# Claim faults: the manifest and the installed authority agree, and the state
# they claim differs from what the disposable database reports. Only bootstrap
# is used, because its report path is its authority receipt path and no prior
# receipt exists to invalidate. Pair faults are refused before any connection.
CLAIM_FAULTS = {
    'source_oid': ('source', 'oid', 1, 'PHASE0_DB_IDENTITY'),
    'source_server_host': ('source', 'server_host', '127.0.0.2/32', 'PHASE0_DB_PAIR'),
    'target_server_port': ('target', 'server_port', 55433, 'PHASE0_DB_PAIR'),
    'target_cluster': ('target', 'cluster_system_identifier', '1', 'PHASE0_DB_PAIR'),
    'target_acl': (None, 'target_datacl', '{}', 'PHASE0_TARGET_FENCE'),
    'frozen_source': (None, 'frozen_source', None, 'PHASE0_SOURCE_STATE'),
    'installed_bytes': (None, 'tool_tree_sha256', '0' * 64, 'PHASE0_INSTALLED_BYTES'),
}


@pytest.mark.asyncio
@pytest.mark.parametrize('fault', sorted(CLAIM_FAULTS))
async def test_bootstrap_claim_fault_refuses_before_ddl(fault):
    """R2 entry boundary: a claim that disagrees with reality refuses."""
    side, key, value, code = CLAIM_FAULTS[fault]
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
        result = await invoke('bootstrap', directory, common, environment)
        assert result.returncode == 1, (fault, result.stdout)
        assert code in result.stderr, (fault, result.stderr)
        assert not Path(common['bootstrap_receipt']).exists()
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval("SELECT to_regclass('public.users')") is None


@pytest.mark.asyncio
@pytest.mark.parametrize('tool', ['bootstrap', 'copy', 'verify'])
async def test_late_target_session_refuses(tool):
    """R2: an extra target session refuses with the authority unchanged."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        if tool != 'bootstrap':
            assert (await invoke('bootstrap', directory, common, environment)).returncode == 0
            common['bootstrap_receipt_sha256'] = hashlib.sha256(
                Path(common['bootstrap_receipt']).read_bytes()).hexdigest()
        if tool == 'verify':
            assert (await invoke('copy', directory, common, environment)).returncode == 0
        suffix = '' if tool == 'bootstrap' else '-session'
        extra = await asyncpg.connect(scratch.target_url.replace('postgresql+asyncpg:', 'postgresql:'))
        try:
            result = await invoke(tool, directory, common, environment, suffix=suffix, apply=True)
        finally:
            await extra.close()
        assert result.returncode == 1, (tool, result.stdout)
        assert 'PHASE0_TARGET_FENCE' in result.stderr, (tool, result.stderr)
        assert not (directory / (f'{tool}{suffix}-report.json')).exists()


@pytest.mark.asyncio
@pytest.mark.parametrize('tool', ['bootstrap', 'copy', 'verify'])
async def test_late_target_acl_change_refuses(tool):
    """R2: a target CONNECT grant after approval refuses with the authority fixed."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        if tool != 'bootstrap':
            assert (await invoke('bootstrap', directory, common, environment)).returncode == 0
            common['bootstrap_receipt_sha256'] = hashlib.sha256(
                Path(common['bootstrap_receipt']).read_bytes()).hexdigest()
        if tool == 'verify':
            assert (await invoke('copy', directory, common, environment)).returncode == 0
        suffix = '' if tool == 'bootstrap' else '-acl'
        async with scratch.admin_connection() as admin:
            await admin.execute('GRANT CONNECT ON DATABASE verdaxis_prod TO verdaxis_app')
        result = await invoke(tool, directory, common, environment, suffix=suffix, apply=True)
        assert result.returncode == 1, (tool, result.stdout)
        assert 'PHASE0_TARGET_FENCE' in result.stderr, (tool, result.stderr)
        assert not (directory / (f'{tool}{suffix}-report.json')).exists()


@pytest.mark.asyncio
@pytest.mark.parametrize('tool', ['copy', 'verify'])
async def test_late_source_change_refuses(tool):
    """R2: a source row changed after approval refuses, authority unchanged."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        assert (await invoke('bootstrap', directory, common, environment)).returncode == 0
        common['bootstrap_receipt_sha256'] = hashlib.sha256(
            Path(common['bootstrap_receipt']).read_bytes()).hexdigest()
        if tool == 'verify':
            assert (await invoke('copy', directory, common, environment)).returncode == 0
        suffix = '-' + tool
        # The approved source role is read-only, so the change is made by the
        # owning administrator, exactly as a late external writer would.
        async with scratch.admin_connection('verdaxis') as admin:
            await admin.execute("UPDATE notifications SET title='late write' WHERE title IS NOT NULL")
        result = await invoke(tool, directory, common, environment, suffix=suffix, apply=True)
        assert result.returncode == 1, (tool, result.stdout)
        assert 'PHASE0_SOURCE_STATE' in result.stderr, (tool, result.stderr)
        assert not (directory / (f'{tool}{suffix}-report.json')).exists()


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
        destination = Path(common['bootstrap_receipt']) if tool == 'bootstrap' else (
            directory / f'{tool}-occupied-report.json')
        destination.write_text('existing evidence')
        suffix = '' if tool == 'bootstrap' else '-occupied'
        result = await invoke(tool, directory, common, environment, suffix=suffix, apply=True)
        assert result.returncode == 1, (tool, result.stdout)
        assert 'PHASE0_REPORT_PATH' in result.stderr, (tool, result.stderr)
        assert destination.read_text() == 'existing evidence'


@pytest.mark.asyncio
@pytest.mark.parametrize('key,value,code', [
    ('hostname', 'wrong-host', 'PHASE0_AUTHORITY'),
    ('release_sha', '0' * 40, 'PHASE0_AUTHORITY'),
    ('source_sha', '1' * 40, 'PHASE0_AUTHORITY'),
    ('source_catalog_sha256', '0' * 64, 'PHASE0_AUTHORITY'),
    ('approved', False, 'PHASE0_AUTHORITY'),
    ('target_datacl', '{}', 'PHASE0_AUTHORITY'),
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
        assert result.returncode == 1, (key, result.stdout)
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


# Boundary faults: a source/target change arrives after the approved authority is
# fixed. The tests prove which boundary names the refusal and whether the
# mutation before that boundary committed or rolled back. Each refusal must leave
# no success report.
@pytest.mark.asyncio
async def test_bootstrap_late_source_change_refuses_at_entry():
    """R2 entry boundary: a source change before the run refuses before DDL."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        await late_source_write(scratch)
        result = await invoke('bootstrap', directory, common, environment)
        assert result.returncode == 1, result.stdout
        assert 'PHASE0_SOURCE_STATE' in result.stderr, result.stderr
        assert not Path(common['bootstrap_receipt']).exists()
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval("SELECT to_regclass('public.users')") is None


@pytest.mark.asyncio
async def test_bootstrap_in_flight_source_sequence_change_refuses_before_commit():
    """R2 precommit boundary: a sequence change inside DDL rolls the DDL back."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        result = await run_tool_with_lock_fault(
            scratch, 'bootstrap', directory, common, environment,
            BOOTSTRAP_LOCK, lambda: bump_source_sequence(scratch))
        assert result.returncode == 1, result.stdout
        assert 'PHASE0_SOURCE_STATE' in result.stderr, result.stderr
        assert not Path(common['bootstrap_receipt']).exists()
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval("SELECT to_regclass('public.users')") is None


@pytest.mark.asyncio
async def test_bootstrap_in_flight_target_acl_change_refuses_before_commit():
    """R2 precommit boundary: a target CONNECT grant rolls the DDL back."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        result = await run_tool_with_lock_fault(
            scratch, 'bootstrap', directory, common, environment,
            BOOTSTRAP_LOCK, lambda: grant_target_connect(scratch))
        assert result.returncode == 1, result.stdout
        assert 'PHASE0_TARGET_FENCE' in result.stderr, result.stderr
        assert not Path(common['bootstrap_receipt']).exists()
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval("SELECT to_regclass('public.users')") is None


@pytest.mark.asyncio
async def test_bootstrap_in_flight_target_session_refuses_before_commit():
    """R2 precommit boundary: a target session opened inside DDL rolls it back."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        result = await run_tool_with_target_session(
            scratch, 'bootstrap', directory, common, environment, BOOTSTRAP_LOCK)
        assert result.returncode == 1, result.stdout
        assert 'PHASE0_TARGET_FENCE' in result.stderr, result.stderr
        assert not Path(common['bootstrap_receipt']).exists()
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval("SELECT to_regclass('public.users')") is None


@pytest.mark.asyncio
async def test_bootstrap_late_source_row_change_refuses_at_final_boundary():
    """R2 final boundary: a row change after commit keeps the DDL, no receipt."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        result = await run_tool_with_lock_fault(
            scratch, 'bootstrap', directory, common, environment,
            BOOTSTRAP_LOCK, lambda: late_source_write(scratch))
        assert result.returncode == 1, result.stdout
        assert 'PHASE0_SOURCE_STATE' in result.stderr, result.stderr
        assert not Path(common['bootstrap_receipt']).exists()
        # The DDL transaction committed before the final observation refused.
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval("SELECT to_regclass('public.users')") is not None


@pytest.mark.asyncio
async def test_endpoint_claim_fault_refuses_before_connection_for_copy_and_verify():
    """R2 entry boundary: a claimed endpoint that differs from its pair refuses."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        faults = (('copy', 'source', 'server_host', '127.0.0.2/32'),
                  ('verify', 'target', 'server_port', 55433))
        for tool, side, key, value in faults:
            claimed = {**common, side: {**common[side], key: value}}
            install_authority(claimed)
            result = await invoke(tool, directory, claimed, environment, suffix='-endpoint')
            assert result.returncode == 1, (tool, result.stdout)
            assert 'PHASE0_DB_PAIR' in result.stderr, (tool, result.stderr)
            assert not (directory / f'{tool}-endpoint-report.json').exists()
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval("SELECT to_regclass('public.users')") is None


@pytest.mark.asyncio
async def test_copy_in_flight_source_sequence_change_refuses_before_commit():
    """R2 precommit boundary: a sequence change rolls the import back."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        await require_committed_chain(directory, common, environment, copy=False)
        result = await run_tool_with_lock_fault(
            scratch, 'copy', directory, common, environment,
            IMPORT_LOCK, lambda: bump_source_sequence(scratch))
        assert result.returncode == 1, result.stdout
        assert 'PHASE0_SOURCE_STATE' in result.stderr, result.stderr
        assert not (directory / 'copy-report.json').exists()
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval('SELECT count(*) FROM trades') == 0


@pytest.mark.asyncio
async def test_copy_in_flight_target_acl_change_refuses_before_commit():
    """R2 precommit boundary: a target CONNECT grant rolls the import back."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        await require_committed_chain(directory, common, environment, copy=False)
        result = await run_tool_with_lock_fault(
            scratch, 'copy', directory, common, environment,
            IMPORT_LOCK, lambda: grant_target_connect(scratch))
        assert result.returncode == 1, result.stdout
        assert 'PHASE0_TARGET_FENCE' in result.stderr, result.stderr
        assert not (directory / 'copy-report.json').exists()
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval('SELECT count(*) FROM trades') == 0


@pytest.mark.asyncio
async def test_copy_in_flight_target_session_refuses_before_commit():
    """R2 precommit boundary: a target session opened inside import rolls it back."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        await require_committed_chain(directory, common, environment, copy=False)
        result = await run_tool_with_target_session(
            scratch, 'copy', directory, common, environment, IMPORT_LOCK)
        assert result.returncode == 1, result.stdout
        assert 'PHASE0_TARGET_FENCE' in result.stderr, result.stderr
        assert not (directory / 'copy-report.json').exists()
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval('SELECT count(*) FROM trades') == 0


@pytest.mark.asyncio
async def test_copy_late_source_row_change_refuses_at_final_boundary():
    """R2 final boundary: a row change after commit keeps the import, no receipt."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        await require_committed_chain(directory, common, environment, copy=False)
        result = await run_tool_with_lock_fault(
            scratch, 'copy', directory, common, environment,
            IMPORT_LOCK, lambda: late_source_write(scratch))
        assert result.returncode == 1, result.stdout
        assert 'PHASE0_SOURCE_STATE' in result.stderr, result.stderr
        assert not (directory / 'copy-report.json').exists()
        # The import transaction committed before the final observation refused.
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval('SELECT count(*) FROM trades') > 0


@pytest.mark.asyncio
async def test_verify_in_flight_target_session_refuses():
    """R2 precommit boundary: a target session opened after entry refuses."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        await require_committed_chain(directory, common, environment)
        process = await start_tool('verify', directory, common, environment)
        extra = None
        try:
            await wait_for_second_source_transaction(scratch, process)
            extra = await asyncpg.connect(
                scratch.target_url.replace('postgresql+asyncpg:', 'postgresql:'))
            result = await finish_tool(process)
        finally:
            if extra is not None:
                await extra.close()
            if process.returncode is None:
                process.kill()
                await process.wait()
        assert result.returncode == 1, result.stdout
        assert 'PHASE0_TARGET_FENCE' in result.stderr, result.stderr
        assert not (directory / 'verify-report.json').exists()
        async with scratch.admin_connection('verdaxis_prod') as target:
            assert await target.fetchval('SELECT count(*) FROM trades') > 0


@pytest.mark.asyncio
async def test_verify_late_target_acl_change_refuses_at_final_boundary():
    """R2 final boundary: a target CONNECT grant is unseen by the frozen snapshot."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        await require_committed_chain(directory, common, environment)
        result = await run_verify_with_late_fault(
            scratch, directory, common, environment, lambda: grant_target_connect(scratch))
        assert result.returncode == 1, result.stdout
        assert 'PHASE0_TARGET_FENCE' in result.stderr, result.stderr
        assert not (directory / 'verify-report.json').exists()


@pytest.mark.asyncio
async def test_verify_late_source_row_change_refuses_at_final_boundary():
    """R2 final boundary: a row change is unseen by the frozen snapshot."""
    async with production_scratch() as scratch:
        directory = Path('/root') / ('authority-' + uuid4().hex)
        common, environment = await make_case(scratch, directory)
        await require_committed_chain(directory, common, environment)
        result = await run_verify_with_late_fault(
            scratch, directory, common, environment, lambda: late_source_write(scratch))
        assert result.returncode == 1, result.stdout
        assert 'PHASE0_SOURCE_STATE' in result.stderr, result.stderr
        assert not (directory / 'verify-report.json').exists()
