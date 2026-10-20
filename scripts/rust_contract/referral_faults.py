"""Real R01 rollback and lost-response schedules on disposable twins."""
import copy
import json
from dataclasses import replace

from scripts.rust_contract.phase2_scenarios import Action, Scenario
from scripts.rust_contract.referral_allocation import auth
from scripts.rust_contract.referral_allocation import servers as allocation_servers

BEFORE = '20000000-0000-4000-8000-000000000201'
AFTER = '20000000-0000-4000-8000-000000000202'


def setup(connection):
    for actor in (BEFORE, AFTER):
        connection.execute("INSERT INTO users(id,email,password_hash,role,status,email_verified,must_change_password) VALUES(%s,%s,'not-login','BUYER','APPROVED',true,false)",(actor,actor+'@example.com'))
    connection.execute("""CREATE FUNCTION referral_test_rollback() RETURNS trigger LANGUAGE plpgsql AS $$
    BEGIN
      IF NEW.id::text = '20000000-0000-4000-8000-000000000201' THEN
        RAISE EXCEPTION 'disposable before-commit failure' USING ERRCODE='23514';
      END IF;
      RETURN NEW;
    END $$""")
    connection.execute('CREATE TRIGGER referral_test_rollback BEFORE UPDATE OF referral_code ON users FOR EACH ROW EXECUTE FUNCTION referral_test_rollback()')


def servers(context):
    values = {BEFORE:['VDX-BEFORE'], AFTER:['VDX-AFTER1']}
    return tuple(replace(server,environment={**server.environment,'REFERRAL_CANDIDATE_MAP':json.dumps(values)}) for server in allocation_servers(context))


def unchanged(before,after,response,mail):
    assert before == after
    assert not mail


def retained(before,after,response,mail):
    expected=copy.deepcopy(before)
    rows=expected['users']['rows']
    for row in rows.values():
        if row['id']==AFTER: row['referral_code']='VDX-AFTER1'
    assert expected==after
    assert not mail


def actions(context):
    yield Action('before-commit-rollback','R01','POST','/api/referrals/my-code',500,headers=auth(BEFORE),actor_id=BEFORE,outcome='rolled-back',assert_after=unchanged)
    yield Action('lost-response-after-commit','R01','POST','/api/referrals/my-code',503,headers={**auth(AFTER),'x-referral-test-response-loss':'true'},actor_id=AFTER,outcome='ambiguous',assert_after=retained)
    yield Action('explicit-call-after-loss','R01','POST','/api/referrals/my-code',200,headers=auth(AFTER),actor_id=AFTER,assert_after=unchanged)


def scenario():
    return Scenario('referral-faults','referrals',setup,servers,actions,build=('cargo','build','--manifest-path','rs/Cargo.toml','--locked','-p','verdaxis-read','--features','contract-tests','--example','referral_contract'))
