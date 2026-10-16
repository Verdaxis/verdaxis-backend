"""Linux database acceptance cannot route to an external database or image."""


def test_linux_runner_uses_pinned_images_and_no_public_network():
    from tests.phase0.linux_database import database_command, runtime_command
    database = database_command('owned-db', 'synthetic-password')
    runtime = runtime_command('owned-runtime', 'owned-db')
    assert database[database.index('--network') + 1] == 'none'
    assert runtime[runtime.index('--network') + 1] == 'container:owned-db'
    assert '-p' not in database and '--publish' not in database
    assert '--mount' not in runtime and '-v' not in runtime
    assert any('@sha256:' in item for item in database)
    assert any(item.startswith('sha256:') for item in runtime)
    assert 'no-new-privileges' in runtime


def test_linux_runner_archives_exact_pins():
    from tests.phase0.linux_database import ARCHIVES
    assert ARCHIVES['prod'] == 'c08266bd6969143812dec7c5033338428cd5f159'
    assert ARCHIVES['staging'] == 'b218ab264b92f99007bacc05d4871413e024b72a'
    assert ARCHIVES['candidate'] == 'HEAD'
