"""Non-empty referral reads on disposable twins: statistics, a full top ten with ties, resolution.

The leaderboard orders by referral count alone, so rows with equal counts are
ordered by the database. This fixture places ties inside the ranking and across
the tenth place; Rust uses stable user-id order, while comparison permits
Python's undefined order and last-place membership (PD2-L1).
"""
import json
from datetime import datetime, timedelta, timezone

from scripts.rust_contract.phase2_scenarios import Action, Scenario
from scripts.rust_contract.referral_allocation import auth
from scripts.rust_contract.referral_allocation import servers as allocation_servers

ORGANIZATIONS = [
    ('10000000-0000-4000-8000-000000000101', 'Atlas Shipping', 'atlas.invalid', 'SHIPPING_LINE'),
    ('10000000-0000-4000-8000-000000000102', 'Borealis Marine', 'borealis.invalid', 'SHIP_MANAGER'),
    ('10000000-0000-4000-8000-000000000103', 'Cobalt Fuels', 'cobalt.invalid', 'FUEL_SUPPLIER'),
]
# Referral counts per referrer: ties inside the ranking and four accounts tied for the last place.
COUNTS = [9, 7, 7, 5, 4, 4, 4, 3, 3, 2, 2, 2, 2, 1]
# (first_name, last_name, organization index or None) for each referrer.
NAMES = [
    ('Ada', 'Lovelace', 0), ('Grace', 'Hopper', 1), (None, 'Turing', None), ('Edsger', None, 2),
    (None, None, None), ('', '', 0), (' Ann ', ' Lee ', 1), ('Émile', '中文', 2),
    ('Linus', 'Torvalds', None), ('Barbara', 'Liskov', 0), ('Donald', 'Knuth', 1), ('Ken', 'Thompson', None),
    ('Dennis', 'Ritchie', 2), ('Margaret', 'Hamilton', 0),
]
ADMIN = '30000000-0000-4000-8000-000000000099'
FIRST_CLOCK = datetime(2026, 9, 1, 8, 0, 0, tzinfo=timezone.utc)


def referrer_id(number):
    return f'30000000-0000-4000-8000-{number:012d}'


def code(number):
    return f'VDX-REF{number:03d}'


def setup(connection):
    for organization, name, domain, kind in ORGANIZATIONS:
        connection.execute('INSERT INTO organizations(id,name,domain,type) VALUES(%s,%s,%s,%s)', (organization, name, domain, kind))
    insert_user = ("INSERT INTO users(id,email,password_hash,role,status,email_verified,must_change_password,"
                   "first_name,last_name,organization_id,referral_code) VALUES(%s,%s,'not-login',%s,'APPROVED',true,false,%s,%s,%s,%s)")
    for number, ((first, last, organization), _) in enumerate(zip(NAMES, COUNTS), start=1):
        organization_id = None if organization is None else ORGANIZATIONS[organization][0]
        connection.execute(insert_user, (referrer_id(number), f'{referrer_id(number)}@example.com', 'BUYER', first, last, organization_id, code(number)))
    connection.execute(insert_user, (ADMIN, f'{ADMIN}@example.com', 'ADMIN', 'Root', 'Admin', ORGANIZATIONS[0][0], 'VDX-ADMIN1'))
    referred = 0
    for number, count in enumerate(COUNTS, start=1):
        for index in range(count):
            referred += 1
            user = f'40000000-0000-4000-8000-{referred:012d}'
            role = None if referred % 5 == 0 else ('SUPPLIER' if referred % 2 else 'BUYER')
            organization_id = None if referred % 3 == 0 else ORGANIZATIONS[referred % 3 - 1][0]
            connection.execute(insert_user, (user, f'{user}@example.com', role, 'Referred', f'Number{referred}', organization_id, None))
            status = ('SIGNED_UP', 'VERIFIED', 'ACTIVE')[index % 3]
            created = FIRST_CLOCK + timedelta(minutes=7 * referred)
            connection.execute(
                'INSERT INTO referrals(id,referrer_id,referred_user_id,referral_code_used,status,created_at) VALUES(%s,%s,%s,%s,%s,%s)',
                (f'50000000-0000-4000-8000-{referred:012d}', referrer_id(number), user, code(number), status, created))


def servers(context):
    return allocation_servers(context)


def body_of(response):
    return json.loads(response['body'])


def unchanged(before, after, response, mail):
    assert before == after, 'a read changed committed state'
    assert not mail, 'a read sent mail'


def leaderboard_shape(before, after, response, mail):
    unchanged(before, after, response, mail)
    entries = body_of(response)
    assert [entry['rank'] for entry in entries] == list(range(1, 11)), 'ranks are consecutive from one'
    assert [entry['referral_count'] for entry in entries] == [9, 7, 7, 5, 4, 4, 4, 3, 3, 2], 'a full top ten with ties'
    assert entries[0]['user_name'] == 'Ada Lovelace'
    assert all(set(entry) == {'rank', 'user_name', 'organization_name', 'referral_count'} for entry in entries)


def statistics_shape(before, after, response, mail):
    unchanged(before, after, response, mail)
    body = body_of(response)
    assert (body['total'], body['verified'], body['active']) == (9, 6, 3)
    created = [item['signed_up_at'] for item in body['referrals']]
    assert created == sorted(created, reverse=True), 'newest first'
    assert all(set(item) == {'organization_name', 'role', 'status', 'signed_up_at'} for item in body['referrals'])
    assert any(item['role'] is None for item in body['referrals']), 'a referral without a role'
    assert any(item['organization_name'] is None for item in body['referrals']), 'a referral without an organization'


def resolution(valid):
    def check(before, after, response, mail):
        unchanged(before, after, response, mail)
        assert body_of(response)['valid'] is valid
    return check


def actions(context):
    owner = auth(referrer_id(1))
    yield Action('statistics-with-referrals', 'R02', 'GET', '/api/referrals/my-referrals', 200, headers=owner,
                 actor_id=referrer_id(1), assert_after=statistics_shape)
    yield Action('statistics-of-smallest-referrer', 'R02', 'GET', '/api/referrals/my-referrals', 200,
                 headers=auth(referrer_id(14)), actor_id=referrer_id(14), assert_after=unchanged)
    yield Action('top-ten-with-ties-across-the-boundary', 'R03', 'GET', '/api/referrals/leaderboard', 200, headers=owner,
                 actor_id=referrer_id(1), assert_after=leaderboard_shape)
    for name, path, valid in [
        ('resolve-person-with-organization', f'/api/referrals/resolve/{code(1)}', True),
        ('resolve-person-without-organization', f'/api/referrals/resolve/{code(3)}', True),
        ('resolve-person-without-names', f'/api/referrals/resolve/{code(5)}', True),
        ('resolve-blank-names', f'/api/referrals/resolve/{code(6)}', True),
        ('resolve-padded-names', f'/api/referrals/resolve/{code(7)}', True),
        ('resolve-unicode-names', f'/api/referrals/resolve/{code(8)}', True),
        ('resolve-administrator', '/api/referrals/resolve/VDX-ADMIN1', True),
        ('resolve-lower-case-is-invalid', f'/api/referrals/resolve/{code(1).lower()}', False),
        ('resolve-unknown', '/api/referrals/resolve/VDX-NONE00', False),
        ('resolve-with-final-newline', f'/api/referrals/resolve/{code(2)}%0A', False),
        ('resolve-too-long', f'/api/referrals/resolve/{code(2)}X', False),
    ]:
        yield Action(name, 'R05', 'GET', path, 200, headers={'x-request-id': 'referral-contract-fixed'}, actor_id=referrer_id(1),
                     assert_after=resolution(valid))


def scenario():
    return Scenario('referral-reads-populated', 'referrals', setup, servers, actions,
                    build=('cargo', 'build', '--manifest-path', 'rs/Cargo.toml', '--locked', '-p', 'verdaxis-read',
                           '--features', 'contract-tests', '--example', 'referral_contract'))
