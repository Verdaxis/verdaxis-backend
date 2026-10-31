"""Real referral routes on separate application-role cells; no live mail."""
import time

from scripts.rust_contract.phase2_scenarios import Action, Race, Scenario, Server

ACTOR = '20000000-0000-4000-8000-000000000001'
SECRET = 'test-secret-key-for-testing-minimum-32-chars'


def setup(connection):
    connection.execute("UPDATE users SET first_name='Ada',last_name='Lovelace',referral_code='VDX-ABC123' WHERE id=%s", (ACTOR,))


def headers():
    import jwt
    now = int(time.time())
    token = jwt.encode({'sub': ACTOR, 'type':'access', 'iat':now, 'exp':now+7200, 'iss':'verdaxis', 'aud':'verdaxis-api'}, SECRET, algorithm='HS256')
    return {'authorization':f'Bearer {token}', 'x-request-id':'referral-contract-fixed'}


def servers(context):
    environment = {'JWT_ISSUER':'verdaxis','JWT_AUDIENCE':'verdaxis-api', 'FRONTEND_URL':'https://app.example///', 'EMAIL_FROM':'Verdaxis <sender@example.com>'}
    return (Server((context.python_bin,'-m','scripts.rust_contract.referral_oracle','--port','{port}'), environment),
            Server((str(context.rust_example("referral_contract")),), environment))


def unchanged(before, after, response, mail):
    assert before == after, 'read/refusal changed committed state'


def actions(context):
    auth = headers()
    for name, route, path, status, bearer in [
        ('stats','R02','/api/referrals/my-referrals',200,auth),
        ('leaderboard','R03','/api/referrals/leaderboard',200,auth),
        ('public','R05','/api/referrals/resolve/VDX-ABC123',200,{}),
        ('public-invalid-bearer','R05','/api/referrals/resolve/VDX-ABC123',200,{'authorization':'Bearer invalid'}),
        ('unknown','R05','/api/referrals/resolve/VDX-ZZZ999',200,{}),
        ('newline','R05','/api/referrals/resolve/VDX-ABC123%0A',200,{}),
        ('unauthenticated','R02','/api/referrals/my-referrals',401,{}),
    ]:
        yield Action(name,route,'GET',path,status,headers={'x-request-id':'referral-contract-fixed',**bearer},actor_id=ACTOR,assert_after=unchanged)
    yield Action('existing-code','R01','POST','/api/referrals/my-code',200,headers=auth,actor_id=ACTOR,assert_after=unchanged)
    for index, status in enumerate([200,201,202,400,500]):
        def check(before, after, response, mail, count=index+1):
            assert before == after
            assert len(mail) == count
            payload = mail[-1]['payload']
            assert payload['to'] == ['new@example.com']
            assert payload['from'] == 'Verdaxis <sender@example.com>'
            assert payload['reply_to'] == 'admin@verdaxis.exchange'
            assert payload['subject'] == 'Ada Lovelace invited you to Verdaxis'
            assert 'https://app.example/invite/VDX-ABC123' in payload['html']
        yield Action(f'invite-{status}','R04','POST','/api/referrals/invite',200 if status in (200,201) else 502,
                     body={'email':'new@example.com'},headers=auth,actor_id=ACTOR,mail_status=status,assert_after=check)
    yield Action('invalid-email','R04','POST','/api/referrals/invite',422,body={'email':'not-email'},headers=auth,actor_id=ACTOR,assert_after=unchanged)


def races(context):
    # Existing code exercises serialized same-user reads without random output.
    def invariant(before, after, results):
        assert before == after
        assert all(response['status'] == 200 and error is None for response,error in results.values())
    return [Race(tuple(Action(f'code-{i}','R01','POST','/api/referrals/my-code',200,headers=headers(),actor_id=ACTOR) for i in range(2)), invariant)]


def scenario():
    return Scenario('referral-contract','referrals',setup,servers,actions,
                    build=('cargo','build','--manifest-path','rs/Cargo.toml','--locked','-p','verdaxis-read','--example','referral_contract'),races=races)
