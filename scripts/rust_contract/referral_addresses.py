"""EmailStr behaviour through the real invite route on disposable twins.

Rejected addresses answer 422 before the hourly invite quota counts them, so
the invalid list can be long. Accepted addresses count, so this scenario keeps
them under the per-runtime limit of ten. The exhaustive grammar comparison is
the Python-captured vector corpus; this scenario proves the same behaviour end
to end: response body, retained state, sent recipient and self/existing-user
comparison after normalization.

A lone surrogate escape is not in this list: Python answers it with HTTP 500, so
it is an approved difference, recorded in `referral-email-approved-differences.json`.
"""
import copy
import json
from dataclasses import replace

from scripts.rust_contract.phase2_scenarios import Action, Scenario
from scripts.rust_contract.phase2_state import expect_cache_invalidations
from scripts.rust_contract.referral_allocation import auth
from scripts.rust_contract.referral_allocation import servers as allocation_servers

SENDER = '20000000-0000-4000-8000-000000000501'
EXISTING = '20000000-0000-4000-8000-000000000502'
SENDER_EMAIL = 'sender@example.com'
EXISTING_EMAIL = 'existing@example.com'
CODE = 'VDX-ADDR01'


def setup(connection):
    for user, email in ((SENDER, SENDER_EMAIL), (EXISTING, EXISTING_EMAIL)):
        connection.execute(
            "INSERT INTO users(id,email,password_hash,role,status,email_verified,must_change_password,first_name,last_name) "
            "VALUES(%s,%s,'not-login','BUYER','APPROVED',true,false,'Ada','Lovelace')", (user, email))


def servers(context):
    return tuple(replace(server, environment={**server.environment, 'REFERRAL_CANDIDATE_MAP': json.dumps({SENDER: [CODE]})})
                 for server in allocation_servers(context))


def rejected(before, after, response, mail):
    assert before == after and not mail, 'a rejected address changed state or sent mail'


def code_only(before, after):
    expected = copy.deepcopy(before)
    allocated = False
    for row in expected['users']['rows'].values():
        if row['id'] == SENDER and not row['referral_code']:
            row['referral_code'] = CODE
            allocated = True
    expect_cache_invalidations(expected, int(allocated))
    assert expected == after, 'an invite changed more than the sender code'


def sent_to(address, count):
    def check(before, after, response, mail):
        code_only(before, after)
        assert len(mail) == count, f'expected {count} sends, saw {len(mail)}'
        assert mail[-1]['payload']['to'] == [address]
        assert json.loads(response['body'])['email'] == address
    return check


def no_send(count):
    def check(before, after, response, mail):
        assert before == after and len(mail) == count
    return check


def invite(name, value, status, check, **options):
    return Action(name, 'R04', 'POST', '/api/referrals/invite', status, body={'email': value}, headers=auth(SENDER),
                  actor_id=SENDER, assert_after=check, **options)


REJECTED = [
    ('quoted-local-part', '"quoted"@example.com'),
    ('ip-literal', 'a@[127.0.0.1]'),
    ('ipv6-literal', 'a@[IPv6:::1]'),
    ('special-use-top-level-domain', 'a@example.test'),
    ('reserved-name', 'a@host.localhost'),
    ('missing-period', 'a@localhost'),
    ('digits-only-top-level-domain', 'a@example.123'),
    ('control-character', 'a\u0001b@example.com'),
    ('unsafe-right-to-left-mark', 'a‏b@example.com'),
    ('local-part-leading-period', '.a@example.com'),
    ('mixed-script-label-with-joiner', 'a@xn--ab-m1t.com'),
    ('display-name-with-period', 'A.B <a@example.com>'),
    ('unclosed-bracket', 'Ada <a@example.com'),
    ('text-after-bracket', 'Ada <a@example.com> x'),
    ('ascii-address-too-long', 'a' * 250 + '@example.com'),
    ('multibyte-address-too-long', 'é' * 130 + '@example.com'),
    ('label-too-long', 'a@' + 'b' * 64 + '.com'),
    ('longer-than-pydantic-limit', 'a' * 2050 + '@example.com'),
    ('fullwidth-at-sign', 'a＠example.com'),
    ('empty', ''),
]


def actions(context):
    for name, value in REJECTED:
        yield invite(f'rejects-{name}', value, 422, rejected)
    for name, value in [('array', ['a@example.com']), ('object', {'a': 1}), ('number', 12), ('boolean', True), ('null', None)]:
        yield invite(f'rejects-non-string-{name}', value, 422, rejected)
    yield Action('rejects-missing-field', 'R04', 'POST', '/api/referrals/invite', 422, body={}, headers=auth(SENDER),
                 actor_id=SENDER, assert_after=rejected)
    yield invite('accepts-reserved-mailbox-name-lower-cased', 'Postmaster@Example.COM', 200, sent_to('postmaster@example.com', 1))
    yield invite('accepts-decomposed-letter-composed', 'é@Example.org', 200, sent_to('é@example.org', 2))
    yield invite('accepts-display-name-form', 'Ada Lovelace <ada@Example.com>', 200, sent_to('ada@example.com', 3))
    yield invite('accepts-quoted-display-name-with-quotes', '"A" "B" <q@example.com>', 200, sent_to('q@example.com', 4))
    yield invite('accepts-internationalized-domain', 'User@例子.公司', 200, sent_to('User@例子.公司', 5))
    yield invite('accepts-punycode-domain-in-unicode-form', 'u@XN--FSQU00A.xn--55qx5d', 200, sent_to('u@例子.公司', 6))
    yield invite('self-invite-after-domain-normalization', 'sender@EXAMPLE.COM', 400, no_send(6))
    yield invite('existing-user-after-normalization', ' existing@EXAMPLE.COM ', 200, no_send(6), recipient_exists=True)


def scenario():
    return Scenario('referral-invite-addresses', 'referrals', setup, servers, actions,
                    build=('cargo', 'build', '--manifest-path', 'rs/Cargo.toml', '--locked', '-p', 'verdaxis-read',
                           '--features', 'contract-tests', '--example', 'referral_contract'))
