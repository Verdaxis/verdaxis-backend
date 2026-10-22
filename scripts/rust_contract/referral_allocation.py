"""Allocation HTTP cases with database-observed ordering, not timing races."""
import copy
import json
import threading
import time
from urllib.parse import urlsplit

from scripts.rust_contract.phase2_referrals import ACTOR
from scripts.rust_contract.phase2_referrals import headers as base_headers
from scripts.rust_contract.phase2_referrals import servers as base_servers
from scripts.rust_contract.phase2_scenarios import Action, Race, Scenario
from scripts.rust_contract.phase2_seed import connect

IDS = [f'20000000-0000-4000-8000-{i:012d}' for i in range(101, 109)]
CODES = {IDS[0]: ['VDX-TAKEN1'] * 10, IDS[1]: ['VDX-TAKEN1'] * 9 + ['VDX-FREE01'],
         IDS[2]: ['VDX-SAME01'], IDS[3]: ['VDX-RACE01'], IDS[4]: ['VDX-RACE01'],
         IDS[5]: ['VDX-SAME02'], IDS[6]: ['VDX-RACE02'], IDS[7]: ['VDX-RACE02']}


def setup(connection):
    for actor in IDS:
        connection.execute("INSERT INTO users(id,email,password_hash,role,status,email_verified,must_change_password) VALUES(%s,%s,'not-login','BUYER','APPROVED',true,false)", (actor, actor+'@example.com'))
    connection.execute("UPDATE users SET referral_code='VDX-TAKEN1' WHERE id=%s", (ACTOR,))
    # Fixture-only SQL barrier: the probe has completed when UPDATE reaches here.
    connection.execute("""CREATE FUNCTION referral_test_update_barrier() RETURNS trigger LANGUAGE plpgsql AS $$
    BEGIN
      IF NEW.id::text IN ('20000000-0000-4000-8000-000000000104','20000000-0000-4000-8000-000000000105','20000000-0000-4000-8000-000000000107','20000000-0000-4000-8000-000000000108') THEN
        PERFORM pg_advisory_xact_lock(78000 + right(NEW.id::text,3)::integer);
      END IF;
      RETURN NEW;
    END $$""")
    connection.execute('CREATE TRIGGER referral_test_barrier BEFORE UPDATE OF referral_code ON users FOR EACH ROW EXECUTE FUNCTION referral_test_update_barrier()')


def auth(actor):
    import jwt

    from scripts.rust_contract.phase2_referrals import SECRET
    now = int(time.time())
    token = jwt.encode({'sub':actor,'type':'access','iat':now,'exp':now+7200,'iss':'verdaxis','aud':'verdaxis-api'},SECRET,algorithm='HS256')
    return {**base_headers(),'authorization':f'Bearer {token}'}


def servers(context):
    from dataclasses import replace
    return tuple(replace(server, environment={**server.environment,'REFERRAL_CANDIDATE_MAP':json.dumps(CODES)})
                 for server in base_servers(context))


def only_code(before, after, response, mail):
    expected = copy.deepcopy(before)
    changed = 0
    for key, row in expected['users']['rows'].items():
        code = after['users']['rows'][key]['referral_code']
        changed += row['referral_code'] != code
        row['referral_code'] = code
    expect_cache_invalidations(expected, changed)
    assert expected == after
    assert not mail


def actions(context):
    yield Action('ten-collisions','R01','POST','/api/referrals/my-code',500,headers=auth(IDS[0]),actor_id=IDS[0],outcome='rolled-back',assert_after=only_code)
    yield Action('nine-collisions-then-free','R01','POST','/api/referrals/my-code',200,headers=auth(IDS[1]),actor_id=IDS[1],assert_after=only_code)


def wait_for(connection, sql, args, count):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        connection.execute('SELECT pg_stat_clear_snapshot()')
        if connection.execute(sql,args).fetchone()[0] >= count:
            return
        threading.Event().wait(.02)
    raise AssertionError('database barrier was not observed')


def races(context):
    port = urlsplit(context.environment['SEED_DATABASE_URL']).port
    errors = []
    observed = []
    def schedule(first, second, collision):
        ready = threading.Event()
        def control():
            try:
                from contextlib import ExitStack
                with ExitStack() as stack:
                    gates = [(database, stack.enter_context(connect(port,database,autocommit=True)))
                             for database in ('phase2_route_0_python_test','phase2_route_0_rust_test')]
                    keys = [78000+int(actor[-3:]) for actor in (first,second)]
                    for database, gate in gates:
                        if collision:
                            for key in keys: gate.execute('SELECT pg_advisory_lock(%s)',(key,))
                        else:
                            gate.execute('BEGIN')
                            gate.execute('SELECT id FROM users WHERE id=%s FOR UPDATE',(first,))
                    ready.set()
                    for database, gate in gates:
                        wait_for(gate,"SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock'",(),2)
                        print('SQL_GATE_OBSERVED', database, flush=True)
                        if collision:
                            gate.execute('SELECT pg_advisory_unlock(%s)',(keys[0],))
                            wait_for(gate,'SELECT count(*) FROM users WHERE id=%s AND referral_code IS NOT NULL',(first,),1)
                            gate.execute('SELECT pg_advisory_unlock(%s)',(keys[1],))
                        else: gate.execute('COMMIT')
                        observed.append(database)
            except BaseException as error:
                print('SQL_GATE_ERROR', type(error).__name__, str(error), flush=True)
                errors.append(error)
                ready.set()
        thread = threading.Thread(target=control,daemon=True)
        thread.start()
        assert ready.wait(15)
        return thread
    for first, second, collision in [(IDS[2],IDS[2],False),(IDS[3],IDS[4],True),(IDS[5],IDS[5],False),(IDS[6],IDS[7],True)]:
        thread = schedule(first,second,collision)
        def invariant(before,after,results):
            only_code(before, after, None, [])
            assert not errors, errors
        yield Race((Action('first','R01','POST','/api/referrals/my-code',200,headers=auth(first),actor_id=first),
                    Action('second','R01','POST','/api/referrals/my-code',500 if collision else 200,headers=auth(second),actor_id=second)),invariant,repeats=1)
        thread.join(20)
        assert not thread.is_alive() and not errors, errors
    assert len(observed)==8


def scenario():
    return Scenario('referral-allocation','referrals',setup,servers,actions,
        build=('cargo','build','--manifest-path','rs/Cargo.toml','--locked','-p','verdaxis-read','--features','contract-tests','--example','referral_contract'),races=races)
