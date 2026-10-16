"""Host authority is private, exact, and cannot be enabled by a mode flag."""
import os

import pytest


def authority_module():
    from scripts.phase0 import host_authority
    return host_authority


def approved_identity():
    return {
        'host': '127.0.0.1', 'port': 55432, 'database': 'verdaxis',
        'oid': 18000, 'role': 'verdaxis_backup',
        'server_host': '127.0.0.1/32', 'server_port': 55432,
        'cluster_system_identifier': '7696961287240179806',
    }


def approved_manifest():
    source = approved_identity()
    target = dict(source, database='verdaxis_prod', oid=18001, role='verdaxis_migrator')
    return dict(format_version=1, mode='production', tool='copy', approved=True,
                hostname='vmi3623757', release_sha='a' * 40,
                source_sha='c08266bd6969143812dec7c5033338428cd5f159',
                source_revision='kycg_20261014_prod', target_revision='p0_20261016_union',
                source=source, target=target, tool_tree_sha256='b' * 64,
                source_catalog_sha256='c' * 64, frozen_source_sha256='d' * 64,
                isolation_sha256='e' * 64, bootstrap_receipt_sha256='f' * 64)


def test_exact_host_authority_contract():
    authority_module().validate_contract(approved_manifest(), tool='copy')


@pytest.mark.parametrize('field,value', [
    ('mode', 'synthetic'), ('approved', 1), ('format_version', True),
    ('tool', 'verify'), ('hostname', ''), ('release_sha', 'head'),
    ('source_sha', 'a' * 40), ('source_revision', 'head'),
    ('target_revision', 'kycd_20261015_staging'), ('tool_tree_sha256', ''),
    ('source_catalog_sha256', 'x' * 64), ('frozen_source_sha256', None),
    ('isolation_sha256', True), ('bootstrap_receipt_sha256', ''),
])
def test_host_authority_rejects_changed_contract(field, value):
    manifest = approved_manifest()
    manifest[field] = value
    with pytest.raises(ValueError):
        authority_module().validate_contract(manifest, tool='copy')


@pytest.mark.parametrize('side,field,value', [
    ('source', 'database', 'verdaxis_prod'), ('target', 'database', 'verdaxis'),
    ('source', 'role', 'verdaxis_migrator'), ('target', 'role', 'verdaxis_app'),
    ('source', 'host', 'localhost'), ('target', 'host', '169.58.37.164'),
    ('source', 'port', True), ('target', 'port', 0),
    ('source', 'oid', True), ('target', 'oid', 18000),
    ('source', 'cluster_system_identifier', '0'),
    ('target', 'cluster_system_identifier', '1234'),
    ('source', 'server_host', 'localhost'), ('target', 'server_port', False),
])
def test_host_authority_rejects_wrong_database_binding(side, field, value):
    manifest = approved_manifest()
    manifest[side][field] = value
    with pytest.raises(ValueError):
        authority_module().validate_contract(manifest, tool='copy')


def test_host_authority_rejects_unknown_fields():
    manifest = approved_manifest()
    manifest['override'] = True
    with pytest.raises(ValueError):
        authority_module().validate_contract(manifest, tool='copy')


def test_private_authority_rejects_untrusted_owner(tmp_path):
    path = tmp_path / 'authority.json'
    path.write_text('{}')
    path.chmod(0o600)
    if os.geteuid() != 0:
        with pytest.raises(ValueError):
            authority_module().read_root_private(path)


def test_private_authority_rejects_symlink(tmp_path):
    path = tmp_path / 'authority.json'
    path.write_text('{}')
    path.chmod(0o600)
    link = tmp_path / 'link.json'
    link.symlink_to(path)
    with pytest.raises(ValueError):
        authority_module().read_root_private(link)


def test_private_authority_rejects_hardlink(tmp_path):
    path = tmp_path / 'authority.json'
    path.write_text('{}')
    path.chmod(0o600)
    os.link(path, tmp_path / 'second.json')
    with pytest.raises(ValueError):
        authority_module().read_root_private(path)


def test_authority_json_rejects_duplicate_keys():
    with pytest.raises(ValueError):
        authority_module().parse_json('{"approved":false,"approved":true}')


def test_authority_json_rejects_nonfinite_values():
    with pytest.raises(ValueError):
        authority_module().parse_json('{"approved":NaN}')


def test_release_tree_digest_detects_untracked_python(tmp_path):
    module = authority_module()
    (tmp_path / 'scripts/phase0').mkdir(parents=True)
    (tmp_path / 'scripts/phase0/copy.py').write_text('pass\n')
    before = module.tree_digest(tmp_path)
    (tmp_path / 'scripts/phase0/sitecustomize.py').write_text('pass\n')
    assert module.tree_digest(tmp_path) != before


def test_release_tree_digest_rejects_symlink(tmp_path):
    module = authority_module()
    (tmp_path / 'app').mkdir()
    (tmp_path / 'app/config.py').symlink_to('/etc/passwd')
    with pytest.raises(ValueError):
        module.tree_digest(tmp_path)
