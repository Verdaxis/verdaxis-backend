"""Read-only observations of the operator-owned Phase 0 database fence.

No grants, settings, HBA edits or session termination. Call at each operation
boundary; source repeatable-read alone is not the post-stop writer gate.
"""
from sqlalchemy import text

from scripts.phase0.refusal import Refusal


async def check_source_quiesced(connection, identity):
    await _require_only_session(connection, identity, 'PHASE0_SOURCE_SESSIONS')


async def _require_only_session(connection, identity, session_code):
    # Avoid a cached statistics snapshot hiding a newly connected writer.
    await connection.execute(text('SELECT pg_stat_clear_snapshot()'))
    row = (await connection.execute(text('''
        SELECT current_database() AS database, d.oid,
               (SELECT count(*) FROM pg_stat_activity a
                WHERE a.datid=d.oid AND a.pid<>pg_backend_pid()) AS others
        FROM pg_database d WHERE d.datname=current_database()
    '''))).mappings().one()
    if row['database'] != identity['database'] or row['oid'] != identity['oid']:
        raise Refusal('PHASE0_DB_IDENTITY')
    if row['others'] != 0:
        raise Refusal(session_code)


async def check_target_fence(connection, identity, expected_datacl):
    if identity['database'] != 'verdaxis_prod' or identity['role'] != 'verdaxis_migrator':
        raise Refusal('PHASE0_TARGET_FENCE')
    if not isinstance(expected_datacl, str) or not expected_datacl:
        raise Refusal('PHASE0_TARGET_FENCE')
    await _require_only_session(connection, identity, 'PHASE0_TARGET_FENCE')
    actual = (await connection.execute(text('''
        SELECT datacl::text FROM pg_database WHERE datname=current_database()
    '''))).scalar_one()
    if actual != expected_datacl:
        raise Refusal('PHASE0_TARGET_FENCE')
    # Matching an operator-provided ACL is insufficient if it permits a runtime
    # role or PUBLIC. The migrator is the dedicated import connection role.
    grants = (await connection.execute(text('''
        SELECT a.grantee, r.rolname, a.privilege_type, a.is_grantable
        FROM pg_database d CROSS JOIN LATERAL aclexplode(d.datacl) a
        LEFT JOIN pg_roles r ON r.oid=a.grantee
        WHERE d.datname=current_database() AND a.privilege_type='CONNECT'
    '''))).mappings().all()
    if not grants or any(row['rolname'] != 'verdaxis_migrator' or row['is_grantable']
                         for row in grants):
        raise Refusal('PHASE0_TARGET_FENCE')
    owner = (await connection.execute(text('''
        SELECT r.rolname FROM pg_database d JOIN pg_roles r ON r.oid=d.datdba
        WHERE d.datname=current_database()
    '''))).scalar_one()
    if owner != 'verdaxis_migrator':
        raise Refusal('PHASE0_TARGET_FENCE')
