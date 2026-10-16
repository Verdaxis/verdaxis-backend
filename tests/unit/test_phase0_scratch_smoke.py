"""Scratch smoke adapter guards: the base URL can never be a live service."""
import os

import pytest

from scripts.phase0.scratch_business_smoke import (
    read_fixture_url,
    require_scratch_base_url,
)


@pytest.mark.parametrize('value', [
    'http://example.com:8100',          # public host name
    'http://api.verdaxis.exchange:8100',  # production API name
    'http://169.58.37.164:8100',        # production host address
    'http://194.233.68.86:8100',        # staging host address
    'http://10.0.0.5:8100',             # non-loopback private address
    'http://127.0.0.1:8000',            # live production listener
    'http://127.0.0.1:8001',            # live staging listener
    'https://127.0.0.1:8100',           # not plain http
    'http://127.0.0.1:8100/api',        # path smuggles a route
    'http://127.0.0.1:8100/?x=1',       # query smuggles parameters
    'http://verdaxis:secret@127.0.0.1:8100',  # embedded credentials
    'http://127.0.0.1:80',              # privileged port
    'http://localhost:8100',            # not a literal address
    'http://127.0.0.1:not-a-port',      # non-numeric port
])
def test_base_url_refuses_anything_that_is_not_a_scratch_listener(value):
    with pytest.raises(ValueError):
        require_scratch_base_url(value)


@pytest.mark.parametrize('value', ['http://127.0.0.1:8100', 'http://127.0.0.1:5434'])
def test_base_url_accepts_a_scratch_loopback_listener(value):
    assert require_scratch_base_url(value) == value


def write_private(tmp_path, contents):
    path = tmp_path / '.env'
    path.write_text(contents)
    os.chmod(path, 0o600)
    return path


@pytest.mark.parametrize('contents', [
    'postgresql+asyncpg://verdaxis_migrator:secret@127.0.0.1:5432/verdaxis_prod\n',  # live port
    'postgresql+asyncpg://verdaxis_migrator:secret@127.0.0.1:5434/verdaxis\n',      # old source name
    'postgresql+asyncpg://verdaxis_migrator:secret@10.0.0.5:5434/verdaxis_prod\n',  # remote host
    'postgresql://verdaxis_migrator:secret@127.0.0.1:5434/verdaxis_prod\n',         # not asyncpg
    'postgresql+asyncpg://verdaxis_migrator@127.0.0.1:5434/verdaxis_prod\n',        # no password
    'postgresql+asyncpg://verdaxis_migrator:secret@127.0.0.1:5434/verdaxis_prod?sslmode=require\n',
])
def test_fixture_url_refuses_live_or_ambiguous_targets(tmp_path, contents):
    with pytest.raises(ValueError):
        read_fixture_url(write_private(tmp_path, contents))


def test_fixture_url_accepts_only_the_scratch_target(tmp_path):
    path = write_private(tmp_path, 'DATABASE_URL='
                         'postgresql+asyncpg://verdaxis_migrator:secret@127.0.0.1:5434/verdaxis_prod\n')
    url = read_fixture_url(path)
    assert (url.host, url.port, url.database) == ('127.0.0.1', 5434, 'verdaxis_prod')


def test_fixture_file_must_hold_exactly_one_line(tmp_path):
    path = write_private(tmp_path, 'JWT_SECRET=x\n'
                         'DATABASE_URL=postgresql+asyncpg://a:b@127.0.0.1:5434/verdaxis_prod\n')
    with pytest.raises(ValueError):
        read_fixture_url(path)


def test_non_private_fixture_file_refuses(tmp_path):
    path = write_private(tmp_path, 'DATABASE_URL='
                         'postgresql+asyncpg://verdaxis_migrator:secret@127.0.0.1:5434/verdaxis_prod\n')
    os.chmod(path, 0o644)
    with pytest.raises(Exception):
        read_fixture_url(path)
