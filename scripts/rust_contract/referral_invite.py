"""Missing-code invite schedules on disposable twins: provider failure, timeout, no key.

An invite for an account without a code commits the allocated code before the
provider request. These schedules prove, against Python, which state survives
each provider outcome and that every request is sent once and never replayed.
"""
import copy
import json
from dataclasses import replace

from scripts.rust_contract.phase2_scenarios import Action, Scenario, Server
from scripts.rust_contract.phase2_state import expect_cache_invalidations
from scripts.rust_contract.referral_allocation import auth
from scripts.rust_contract.referral_allocation import servers as allocation_servers

BUILD = ('cargo', 'build', '--manifest-path', 'rs/Cargo.toml', '--locked', '-p', 'verdaxis-read',
         '--features', 'contract-tests', '--example', 'referral_contract')
SENDERS = [f'20000000-0000-4000-8000-{number:012d}' for number in range(301, 311)]
FIRST, REJECTED, BAD_REQUEST, SERVER_ERROR, MALFORMED, ROLLBACK, LOST, EXISTING = SENDERS[:8]
CODES = {sender: [f'VDX-INV{index:03d}'] for index, sender in enumerate(SENDERS, start=1)}
TIMEOUT_SENDER, NO_KEY_SENDER = SENDERS[8], SENDERS[9]


def setup(connection):
    for number, sender in enumerate(SENDERS, start=1):
        connection.execute(
            "INSERT INTO users(id,email,password_hash,role,status,email_verified,must_change_password,first_name,last_name) "
            "VALUES(%s,%s,'not-login','BUYER','APPROVED',true,false,'Inviter',%s)",
            (sender, f'{sender}@example.com', f'Number{number}'))
    connection.execute("""CREATE FUNCTION referral_invite_test_rollback() RETURNS trigger LANGUAGE plpgsql AS $$
    BEGIN
      IF NEW.id::text = '20000000-0000-4000-8000-000000000306' THEN
        RAISE EXCEPTION 'disposable before-commit failure' USING ERRCODE='23514';
      END IF;
      RETURN NEW;
    END $$""")
    connection.execute('CREATE TRIGGER referral_invite_test_rollback BEFORE UPDATE OF referral_code ON users '
                       'FOR EACH ROW EXECUTE FUNCTION referral_invite_test_rollback()')


def candidate_servers(context):
    return tuple(replace(server, environment={**server.environment, 'REFERRAL_CANDIDATE_MAP': json.dumps(CODES)})
                 for server in allocation_servers(context))


def recipient(sender):
    return f'invitee-{sender[-3:]}@example.com'


def keeps_only_code(sender, before, after):
    """The sender's new code is the single business change."""
    expected = copy.deepcopy(before)
    for row in expected['users']['rows'].values():
        if row['id'] == sender:
            assert not row['referral_code'], 'fixture sender already had a code'
            row['referral_code'] = CODES[sender][0]
    expect_cache_invalidations(expected, 1)
    assert expected == after, 'invite changed more than the sender code'


def attempted(sender, count, *, retained=True, sends=True):
    """Assert the cumulative provider ledger and the retained pre-send code."""
    def check(before, after, response, mail):
        if retained:
            if any(row['id'] == sender and row['referral_code'] for row in before['users']['rows'].values()):
                assert before == after, 'a retry changed committed state'
            else:
                keeps_only_code(sender, before, after)
        else:
            assert before == after, 'a refused invite changed committed state'
        assert len(mail) == count, f'expected {count} provider attempts, saw {len(mail)}'
        if sends:
            payload = mail[-1]['payload']
            assert payload['to'] == [recipient(sender)]
            assert f'https://app.example/invite/{CODES[sender][0]}' in payload['html']
            assert payload['subject'] == f'Inviter Number{SENDERS.index(sender) + 1} invited you to Verdaxis'
    return check


def invite(name, sender, status, check, *, expected_status=None, outcome='committed', headers=None, recipient_exists=False,
           mail_status=200, mail_body=b'{"id":"disposable-send"}', to=None):
    return Action(name, 'R04', 'POST', '/api/referrals/invite', expected_status or status,
                  body={'email': to or recipient(sender)}, headers={**auth(sender), **(headers or {})}, actor_id=sender,
                  outcome=outcome, recipient_exists=recipient_exists, mail_status=mail_status, mail_body=mail_body,
                  assert_after=check)


def failure_actions(context):
    # Nine requests: the per-client hourly invite quota is ten in each runtime.
    yield invite('missing-code-accepted', FIRST, 200, attempted(FIRST, 1))
    yield invite('missing-code-provider-202', REJECTED, 502, attempted(REJECTED, 2), mail_status=202)
    yield invite('missing-code-provider-400', BAD_REQUEST, 502, attempted(BAD_REQUEST, 3), mail_status=400)
    yield invite('missing-code-provider-500', SERVER_ERROR, 502, attempted(SERVER_ERROR, 4), mail_status=500)
    yield invite('missing-code-malformed-accepted-body', MALFORMED, 200, attempted(MALFORMED, 5), mail_body=b'not-json')
    yield invite('missing-code-before-commit-rollback', ROLLBACK, 500, attempted(ROLLBACK, 5, retained=False, sends=False),
                 outcome='rolled-back')
    yield invite('missing-code-lost-response-after-send', LOST, 503, attempted(LOST, 6), outcome='ambiguous',
                 headers={'x-referral-test-response-loss': 'true'})
    yield invite('explicit-second-call-sends-again', LOST, 200, attempted(LOST, 7))

    def silent(before, after, response, mail):
        assert before == after and len(mail) == 7, 'an existing recipient allocated a code or sent mail'
    yield invite('existing-recipient-has-no-code-or-send', EXISTING, 200, silent, recipient_exists=True,
                 to=f'{FIRST}@example.com')


def failure_scenario():
    return Scenario('referral-invite-failures', 'referrals', setup, candidate_servers, failure_actions, build=BUILD)


def relay_servers(context):
    """Both runtimes call the loopback relay, which stalls after the provider status line."""
    environment = {'REFERRAL_CANDIDATE_MAP': json.dumps(CODES)}
    base = allocation_servers(context)
    command = (context.python_bin, '-m', 'scripts.rust_contract.referral_timeout_server')
    return (Server((*command, 'python', '--port', '{port}'), {**base[0].environment, **environment}),
            Server((*command, 'rust'), {**base[1].environment, **environment}))


def timeout_actions(context):
    # Each provider call waits for its ten-second deadline in each runtime.
    yield invite('missing-code-provider-timeout', TIMEOUT_SENDER, 502, attempted(TIMEOUT_SENDER, 1))
    yield invite('explicit-second-call-times-out-again', TIMEOUT_SENDER, 502, attempted(TIMEOUT_SENDER, 2))


def timeout_scenario():
    return Scenario('referral-invite-timeout', 'referrals', setup, relay_servers, timeout_actions, build=BUILD)


def no_key_servers(context):
    environment = {'REFERRAL_CANDIDATE_MAP': json.dumps(CODES), 'REFERRAL_TEST_MAIL_KEY_ABSENT': 'true'}
    return tuple(replace(server, environment={**server.environment, **environment}) for server in allocation_servers(context))


def no_key_actions(context):
    yield invite('missing-code-without-provider-key', NO_KEY_SENDER, 502, attempted(NO_KEY_SENDER, 0, sends=False))
    yield invite('explicit-second-call-without-provider-key', NO_KEY_SENDER, 502, attempted(NO_KEY_SENDER, 0, sends=False))


def no_key_scenario():
    return Scenario('referral-invite-no-key', 'referrals', setup, no_key_servers, no_key_actions, build=BUILD)
