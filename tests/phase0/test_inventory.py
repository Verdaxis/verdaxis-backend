"""Pinned, synthetic PostgreSQL source inventory proofs."""

import pytest
from sqlalchemy import text

from scripts.phase0.manifest import inspect_catalog, load_manifest, require_complete_inventory, digest
from tests.phase0.synthetic import uid
import json
import os
from pathlib import Path
import socket


def test_no_external_network():
    with pytest.raises(PermissionError, match='Phase 0'):
        socket.getaddrinfo('example.invalid', 443)
    with socket.socket() as client:
        with pytest.raises(PermissionError, match='Phase 0'):
            client.connect(('192.0.2.1', 443))


def test_target_catalog_and_mapping():
    manifest = load_manifest()
    actual = json.loads((Path(os.environ['PHASE0_WORK']) / 'union-catalog.json').read_text())
    require_complete_inventory(actual, manifest['target_catalog'])
    assert actual['revision'] == 'p0_20261016_union'
    assert len(actual['tables']) == 62
    assert sum(len(item['target_additions']) for item in manifest['tables'].values()) == 15
    for name, item in manifest['tables'].items():
        # The mapping names every copied source column; legacy_columns names the
        # live-only source columns the target has no column for.
        assert set(item['columns']) == (
            {column['name'] for column in manifest['source_catalog']['tables'][name]['columns']}
            - set(item.get('legacy_columns', ()))
        )
        if name in actual['tables']:
            assert set(item['columns']) | set(item['target_additions']) == {column['name'] for column in actual['tables'][name]['columns']}


def test_provisional_union():
    union = json.loads(Path('deploy/phase0/provisional-union.json').read_text())
    expected = union.pop('provisional_digest')
    assert digest(union) == expected
    assert len(union['products']) == 8
    assert len({product['id'] for product in union['products'].values()}) == 8
    assert union['kyc_execution_gate_enabled'] is False
    assert union['database_timezone'] == 'UTC'


@pytest.mark.asyncio
async def test_postgres_versions(source_connection):
    row = (await source_connection.execute(text("SELECT current_setting('server_version_num')::int, postgis_lib_version()"))).one()
    assert 170000 <= row[0] < 180000
    assert row[1].startswith('3.6.')


@pytest.mark.asyncio
async def test_historical_snapshots_and_states(source_connection):
    rows = (await source_connection.execute(text('SELECT status, market_snapshot_version FROM trades'))).all()
    assert {row[0] for row in rows if row[1] is None} == {'PENDING_CONFIRMATION','CONFIRMED','DELIVERED','PAID','CANCELLED','DECLINED'}
    assert all(row[1] in (None, 1) for row in rows)
    row = (await source_connection.execute(text("SELECT t.buyer_provenance, o.verification_status FROM trades t JOIN organizations o ON o.id=t.buyer_id WHERE t.market_snapshot_version=1 AND t.market_product='BIO_METHANOL'"))).one()
    assert tuple(row) == ('REAL', 'REJECTED')
    transaction = await source_connection.begin_nested()
    try:
        from sqlalchemy.exc import DBAPIError
        with pytest.raises(DBAPIError):
            await source_connection.execute(text('UPDATE trades SET market_snapshot_version=0 WHERE market_snapshot_version IS NULL'))
    finally:
        await transaction.rollback()


@pytest.mark.asyncio
async def test_kyc_membership_and_notice_history(source_connection):
    async def scalar(sql):
        return (await source_connection.execute(text(sql))).scalar_one()
    assert await scalar("SELECT count(*) FROM organization_join_requests WHERE status='PENDING'") == 1
    assert await scalar("SELECT count(*) FROM users WHERE kyc_status='REJECTED'") == 1
    assert await scalar("SELECT count(*) FROM users WHERE kyc_organization_id <> organization_id") == 1
    assert await scalar("SELECT count(*) FROM kyc_execution_grace WHERE expires_at < '2026-10-01' AND expires_at=enrolled_at+interval '336 hours'") == 1
    assert await scalar("SELECT count(*) FROM audit_logs WHERE action='kyc.grace_notice_sent'") == 1


@pytest.mark.asyncio
async def test_cycles_receipts_and_sparse_sequences(source_connection):
    row = (await source_connection.execute(text("SELECT count(*) FROM rfqs r JOIN rfq_quotes q ON q.id=r.accepted_quote_id AND q.rfq_id=r.id JOIN trades t ON t.id=r.trade_id"))).scalar_one()
    assert row == 1
    assert (await source_connection.execute(text("SELECT count(*) FROM orderbook_orders o JOIN market_support_authorizations a ON a.id=o.support_authorization_id WHERE a.status='CONSUMED'"))).scalar_one() == 1
    assert (await source_connection.execute(text("SELECT count(*) FROM market_command_results WHERE support_context_id IS NOT NULL AND response_body->>'_private'='preserve'"))).scalar_one() == 1
    assert list((await source_connection.execute(text('SELECT stream_seq FROM market_event_outbox ORDER BY stream_seq'))).scalars()) == [7,91]
    assert (await source_connection.execute(text('SELECT last_value FROM market_event_stream_seq'))).scalar_one() == 150
    assert (await source_connection.execute(text('SELECT last_value FROM orderbook_acceptance_ordinal_seq'))).scalar_one() == 200


@pytest.mark.asyncio
async def test_json_precision_nulls_and_private_notification(source_connection):
    values = {}
    for name in ('sql-null', 'json-null', 'precision'):
        values[name] = (await source_connection.execute(text('SELECT data::text FROM notifications WHERE id=:id'), {'id': uid(name)})).scalar_one()
    assert values == {'sql-null': None, 'json-null': 'null', 'precision': '{"value":1234567890.12345678901234567890}'}
    value = (await source_connection.execute(text('SELECT data::text FROM notifications WHERE id=:id'), {'id': uid('notification')})).scalar_one()
    assert json.loads(value)['_email_only'] is True
    assert '_email_payload' in json.loads(value)


@pytest.mark.asyncio
async def test_historical_catalog_values(source_connection):
    row = (await source_connection.execute(text('SELECT spec_description, is_active, min_lot_size FROM products WHERE id=:id'), {'id': uid('historical-product')})).one()
    assert tuple(row) == ('Preserve this source description', False, 17)
    assert (await source_connection.execute(text('SELECT timezone FROM delivery_points WHERE id=:id'), {'id': uid('historical-point')})).scalar_one() == 'Asia/Singapore'


def test_declared_inventory():
    manifest = load_manifest()
    classes = [table["classification"] for table in manifest["tables"].values()]
    assert len(classes) == 61
    assert classes.count("copy") == 55
    assert classes.count("regenerate") == 2
    assert classes.count("skip") == 4


@pytest.mark.asyncio
async def test_source_catalog(source_connection):
    actual = await source_connection.run_sync(inspect_catalog)
    require_complete_inventory(actual, load_manifest()["source_catalog"])
    assert actual["revision"] == "kycg_20261014_prod"


@pytest.mark.parametrize("ddl", [
    "CREATE TABLE public.unclassified (id integer)",
    "ALTER TABLE public.users ADD COLUMN unclassified text",
    "CREATE TABLE public.unclassified_partition (id integer) PARTITION BY RANGE (id)",
    "ALTER TABLE public.users ALTER COLUMN first_name TYPE varchar(199)",
])
@pytest.mark.asyncio
async def test_unclassified_schema_refused(source_connection, ddl):
    transaction = await source_connection.begin_nested()
    try:
        await source_connection.execute(text(ddl))
        with pytest.raises(ValueError, match="Unclassified source schema"):
            require_complete_inventory(await source_connection.run_sync(inspect_catalog), load_manifest()["source_catalog"])
    finally:
        await transaction.rollback()


@pytest.mark.asyncio
async def test_every_copy_table_is_populated(source_connection):
    for name, table in load_manifest()["tables"].items():
        if table["classification"] == "copy":
            assert (await source_connection.execute(text(f'SELECT count(*) FROM "{name}"'))).scalar_one() > 0, name
