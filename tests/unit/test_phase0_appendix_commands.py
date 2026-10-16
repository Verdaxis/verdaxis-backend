"""Parse every repository CLI command in the host appendix without I/O.

Use the argument-parser statements from each real main function. Do not import
application modules, load secrets, execute shell blocks or open a database.
"""
import argparse
import ast
import re
import shlex
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
APPENDIX = ROOT / 'docs/runbooks/phase0-host-appendix.md'


def parser_for(module):
    path = ROOT.joinpath(*module.split('.')).with_suffix('.py')
    tree = ast.parse(path.read_text())
    main = next(node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'main')
    statements = []
    for node in main.body:
        if isinstance(node, ast.Assign) and any(
            isinstance(call, ast.Call) and isinstance(call.func, ast.Attribute)
            and call.func.attr == 'ArgumentParser' for call in ast.walk(node)
        ):
            statements.append(node)
        elif isinstance(node, ast.Expr) and isinstance(node.value, ast.Call):
            call = node.value
            if isinstance(call.func, ast.Attribute) and isinstance(call.func.value, ast.Name) and call.func.value.id == 'parser' and call.func.attr == 'add_argument':
                statements.append(node)
    assert statements, f'No real parser found in {path}'
    namespace = {'argparse': argparse, 'Path': Path, '__doc__': 'CLI contract'}
    # The assertion and fence parsers use literal module-level choice lists.
    for node in tree.body:
        if isinstance(node, ast.Assign):
            for target in node.targets:
                if isinstance(target, ast.Name) and target.id in {'CHECKS', 'DATABASES', '_IDENTITIES'}:
                    namespace[target.id] = ast.literal_eval(node.value)
    exec(compile(ast.Module(body=statements, type_ignores=[]), str(path), 'exec'), namespace)
    return namespace['parser']


def documented_commands():
    text = APPENDIX.read_text()
    blocks = re.findall(r'^```(?:bash|sh)\n(.*?)^```', text, re.MULTILINE | re.DOTALL)
    commands = []
    for block in blocks:
        for line in block.replace('\\\n', ' ').splitlines():
            if line.lstrip().startswith('#'):
                continue
            if '-m scripts.' not in line:
                continue
            words = shlex.split(line)
            index = words.index('-m')
            module = words[index + 1]
            arguments = words[index + 2:]
            if '<' in arguments:
                redirect = arguments.index('<')
                assert redirect == len(arguments) - 2, line
                assert arguments[-1].startswith('/root/verdaxis-phase0/'), line
                arguments = arguments[:redirect]
            assert not any(word in {'|', ';', '&&', '>'} for word in arguments), line
            commands.append((module, arguments))
    assert commands, 'Appendix must contain executable tool commands'
    return commands


def test_every_appendix_tool_command_matches_real_parser():
    commands = documented_commands()
    modules = {module for module, _ in commands}
    assert {'scripts.phase0.bootstrap', 'scripts.phase0.copy', 'scripts.phase0.verify',
            'scripts.phase0.assert_host', 'scripts.phase0.start_fence',
            'scripts.phase0.scratch_business_smoke',
            'scripts.validate_health_response'} <= modules
    for module, arguments in commands:
        parser_for(module).parse_args(arguments)
    # Do not let a later edit evade the extractor by changing invocation style.
    text = APPENDIX.read_text()
    assert not re.search(r'^\s*(?:p0run\s+)?\S*python\S*\s+(?:/\S+/)?scripts/\S+\.py', text, re.MULTILINE)


def test_production_tools_have_separate_manifests_and_secrets():
    tools = {}
    for module, arguments in documented_commands():
        if module not in {'scripts.phase0.bootstrap', 'scripts.phase0.copy', 'scripts.phase0.verify'}:
            continue
        values = vars(parser_for(module).parse_args(arguments))
        tool = module.rsplit('.', 1)[1]
        assert values['source_environment_file'] is not None
        assert f'/{tool}-source.env' in str(values['source_environment_file'])
        assert f'/{tool}-target.env' in str(values['target_environment_file'])
        assert f'/{tool}.json' in str(values['manifest'])
        tools[tool] = str(values['manifest'])
    assert len(set(tools.values())) == 3


@pytest.mark.parametrize('module', ['scripts.phase0.bootstrap', 'scripts.phase0.copy', 'scripts.phase0.verify'])
def test_obsolete_or_abbreviated_tool_flags_refuse(module):
    parser = parser_for(module)
    with pytest.raises(SystemExit) as error:
        parser.parse_args(['--environment-file', '/not-read'])
    assert error.value.code == 2


def test_appendix_shell_tools_use_real_argument_parsers_only():
    import subprocess

    text = APPENDIX.read_text()
    blocks = re.findall(r'^```(?:bash|sh)\n(.*?)^```', text, re.MULTILINE | re.DOTALL)
    commands = []
    for block in blocks:
        for line in block.replace('\\\n', ' ').splitlines():
            if '/scripts/' not in line or '.sh' not in line:
                continue
            words = shlex.split(line)
            for index, word in enumerate(words):
                if '/scripts/' in word and word.endswith('.sh'):
                    commands.append((Path(word).name, words[index + 1:]))
    assert {name for name, _ in commands} == {'deploy.sh', 'install_systemd_units.sh'}
    for name, arguments in commands:
        source = (ROOT / 'scripts' / name).read_text()
        arguments = ['a' * 40 if value == '$RELEASE_SHA' else value for value in arguments]
        if name == 'deploy.sh':
            # Stop before cd, Git, environment loading or any deployment action.
            parser = source[source.index('DRY_RUN=0'):source.index('cd "$BACKEND_DIR"')]
        else:
            # Extract mode initialization and parser through SHA validation only.
            parser = source[source.index('MODE="dry-run"'):source.index('case "$DEPLOY_ENVIRONMENT" in')]
        result = subprocess.run(['bash', '-c', 'set -eu\n' + parser, name, *arguments],
                                capture_output=True, text=True, timeout=5)
        assert result.returncode == 0, (name, result.stderr)


def test_pm_p05_scope_and_remaining_conflicts_are_explicit():
    text = APPENDIX.read_text()
    assert '## Open items' in text
    assert '## PM questions' not in text
    assert 'pg_createcluster 17 p0rehearsal --port 5434 --locale en_US.UTF-8 --start-conf manual' in text
    assert 'IPAddressDeny=any' in text and 'IPAddressAllow=localhost' in text
    assert '127.0.0.1:8100' in text
    assert 'not rehearsed' in text
    assert '30-minute target' in text and '60-minute hard limit' in text
    assert 'after reopening' in text
    assert 'public `/health/ready`' in text


def test_p0run_reopens_only_the_private_workspace_for_evidence():
    """ProtectHome=read-only needs the exact evidence path writable."""
    text = APPENDIX.read_text()
    block = next(block for block in re.findall(r'^```bash\n(.*?)^```', text, re.MULTILINE | re.DOTALL)
                 if 'p0run()' in block)
    assert '-p ProtectHome=read-only \\\n' in block
    assert '-p ReadWritePaths=/root/verdaxis-phase0 \\\n' in block
    assert block.count('ReadWritePaths') == 1
    # No other host path may be reopened by the transient unit.
    assert '/home' not in block.split('p0run()', 1)[1].split('```', 1)[0]


def test_target_creation_resets_the_public_schema_in_order():
    """A new database's public schema must reach live's owner and ACL first."""
    text = APPENDIX.read_text()
    block = next(block for block in re.findall(r'^```bash\n(.*?)^```', text, re.MULTILINE | re.DOTALL)
                 if 'CREATE EXTENSION postgis;' in block)
    for line in ('ALTER SCHEMA public OWNER TO verdaxis_migrator;',
                 'REVOKE ALL ON SCHEMA public FROM PUBLIC;',
                 'GRANT USAGE ON SCHEMA public TO verdaxis_app, verdaxis_backup;'):
        assert line in block
    order = [block.index('REVOKE TEMPORARY ON DATABASE verdaxis_prod FROM PUBLIC;'),
             block.index('ALTER SCHEMA public OWNER TO verdaxis_migrator;'),
             block.index('REVOKE ALL ON SCHEMA public FROM PUBLIC;'),
             block.index('GRANT USAGE ON SCHEMA public TO verdaxis_app, verdaxis_backup;')]
    assert order == sorted(order)
    assert ('verdaxis_migrator {verdaxis_migrator=UC/verdaxis_migrator,'
            'verdaxis_app=U/verdaxis_migrator,verdaxis_backup=U/verdaxis_migrator}') in text


def test_appendix_shell_blocks_are_syntactically_valid():
    import subprocess

    text = APPENDIX.read_text()
    blocks = re.findall(r'^```(?:bash|sh)\n(.*?)^```', text, re.MULTILINE | re.DOTALL)
    for index, block in enumerate(blocks):
        result = subprocess.run(['bash', '-n'], input=block, capture_output=True,
                                text=True, timeout=5)
        assert result.returncode == 0, (index, result.stderr)


def test_final_pm_order_and_static_backup_boundary():
    text = APPENDIX.read_text()
    assert '## PM answers — final sequencing decisions' in text
    assert 'candidate_only=true' not in text
    assert text.count('-f candidate_only=false') == 1
    assert 'path /health/ready /health/live' in text
    assert 'method GET' in text
    assert 'dispatched_at IS NULL' in text and 'stream_seq IS NULL' in text
    assert text.index('### B4a. Static backup') < text.index('### B5.')
    assert text.index('### B10. Sign and reopen') < text.index('### B10a. ONE frontend')
    assert 'after API reopening, never automatically return to the old database' in text


def test_inline_operator_python_compiles_without_execution():
    text = APPENDIX.read_text()
    snippets = re.findall(r"<<'PY'\n(.*?)^PY$", text, re.MULTILINE | re.DOTALL)
    assert snippets
    for index, snippet in enumerate(snippets):
        compile(snippet, f'appendix-inline-{index}', 'exec')


def test_current_runbook_does_not_reuse_historical_merge_state():
    text = (ROOT / 'docs/runbooks/phase0-cutover.md').read_text()
    assert '## Current state — 2026-10-16' in text
    assert 'It is not merged into this lane' not in text
    assert 'phase0-host-appendix.md' in text


def test_post_cutover_amendments_are_recorded_and_applied():
    """The 17 October window findings must stay folded into the runbook."""
    text = APPENDIX.read_text()
    assert 'Post-cutover amendments (17 October 2026)' in text
    # B4 target default privileges: validate_roles.sql refuses without them.
    block = next(block for block in re.findall(r'^```bash\n(.*?)^```', text, re.MULTILINE | re.DOTALL)
                 if 'CREATE EXTENSION postgis;' in block)
    for line in ('ALTER DEFAULT PRIVILEGES FOR ROLE verdaxis_migrator IN SCHEMA public '
                 'GRANT SELECT ON TABLES TO verdaxis_backup;',
                 'ALTER DEFAULT PRIVILEGES FOR ROLE verdaxis_migrator IN SCHEMA public '
                 'GRANT SELECT ON SEQUENCES TO verdaxis_backup;'):
        assert line in block
    # B7 curve probe resolves a real product id; the bare path 422s.
    assert '"/api/curves/forward?product_id=$CURVE_PRODUCT"' in text
    assert '/api/curves/forward;' not in text
    assert 'SELECT id FROM products ORDER BY id LIMIT 1' in text
    # B2 proves the frozen dump restores with per-table counts before B3.
    assert 'pg_restore --list` is not proof' in text
    assert 'verdaxis_finalcheck' in text
    assert text.index('verdaxis_finalcheck') < text.index('### B3.')
    # B1 derives the rollback candidate list from the release unit manifest.
    assert 'runtime-units.manifest' in text
    assert 'candidate-only-files.txt' in text


def test_b5_carries_demo_acl_and_requires_key_source_before_deploy():
    text = APPENDIX.read_text()
    b5 = text[text.index('### B5.'):text.index('### B6.')]
    # Demo-user ACL carry, exactly .git and nothing wider.
    assert 'setfacl --set-file=- -p "$root/.git"' in b5
    assert 'setfacl --set-file=- -p "$root/.git/HEAD"' in b5
    assert 'sudo -u verdaxis-demo-production test ! -r "$root/.env"' in b5
    # The key source must exist before the installer preflight.
    assert "test ! -e /etc/verdaxis/kyc-document-keys.json" in b5
    assert "printf '' > /etc/verdaxis/kyc-document-keys.json" in b5
    assert b5.index('kyc-document-keys.json') < b5.index('--dry-run')
    # No root safe.directory anywhere: git runs as the checkout owner.
    assert 'safe.directory' in text
    assert 'sudo -n -u <owner>' in text


def test_b10b_checks_demo_identity_as_demo_user_before_rearm():
    text = APPENDIX.read_text()
    b10b = text[text.index('### B10b.'):text.index('### B11.')]
    assert 'verify_runtime_identity.py' in b10b
    assert 'sudo -u verdaxis-demo-production' in b10b
    assert b10b.index('verify_runtime_identity.py') < b10b.index("timers-restore.txt")
    assert 'Verdaxis runtime identity: ok' in b10b


def test_b11_names_the_placeholder_state():
    text = APPENDIX.read_text()
    b11 = text[text.index('### B11.'):text.index('## Acceptance limits')]
    assert 'empty root:root 0400 placeholder' in b11
    assert 'never be overwritten' in b11
