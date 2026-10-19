"""Both real referral writers share one database, then Python dispatch drains."""
import json
import subprocess
from dataclasses import replace

from scripts.rust_contract.phase2_scenarios import Action
from scripts.rust_contract.referral_allocation import auth
from scripts.rust_contract.referral_faults import scenario as fault_scenario

A='20000000-0000-4000-8000-000000000202'


def mixed(context):
    return [(0,Action('python-allocates','R01','POST','/api/referrals/my-code',200,headers=auth(A),actor_id=A)),
            (1,Action('rust-reuses','R01','POST','/api/referrals/my-code',200,headers=auth(A),actor_id=A)),
            (1,Action('rust-invites','R04','POST','/api/referrals/invite',200,body={'email':'mixed@example.com'},headers=auth(A),actor_id=A)),
            (0,Action('python-invites','R04','POST','/api/referrals/invite',200,body={'email':'mixed@example.com'},headers=auth(A),actor_id=A))]


def dispatch(context):
    result=subprocess.run([context.python_bin,'-m','scripts.rust_contract.referral_dispatch'],cwd=context.root,
        env={**context.environment,'PHASE2_DATABASE_URL':context.database_url},check=True,capture_output=True,text=True,timeout=60)
    return json.loads(result.stdout.strip().splitlines()[-1])


def assert_mixed(state,result):
    rows={row['id']:row for row in state['users']['rows'].values()}
    assert rows[A]['referral_code']=='VDX-AFTER1'
    assert not state['referrals']['rows']
    assert not state['market_event_outbox']['rows']


def scenario():
    return replace(fault_scenario(),name='referral-mixed',mixed=mixed,dispatch=dispatch,assert_mixed=assert_mixed)
