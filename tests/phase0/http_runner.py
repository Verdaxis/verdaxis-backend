"""Run the guarded HTTP suite with migrator setup and constrained runtime ACLs."""
import os, subprocess, sys, time, uuid, socket, urllib.request
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT))
from tests.phase0.runner import IMAGE
name='verdaxis-task9-http-'+uuid.uuid4().hex[:10]
password=uuid.uuid4().hex
run=ROOT/'work/task9-http'
run.mkdir(exist_ok=True)
server=None
log=None
def cmd(*args,**kwargs):
    return subprocess.run(args,check=True,text=True,**kwargs)
try:
    cmd('docker','run','-d','--platform','linux/amd64','--name',name,'-e',f'POSTGRES_PASSWORD={password}','-p','127.0.0.1::5432',IMAGE)
    for _ in range(240):
        if subprocess.run(['docker','exec',name,'pg_isready','-h','127.0.0.1','-U','postgres'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode==0: break
        time.sleep(1)
    else: raise RuntimeError('database not ready')
    port=cmd('docker','port',name,'5432/tcp',capture_output=True).stdout.strip().split(':')[-1]
    cmd('docker','exec',name,'psql','-U','postgres','-c',f"CREATE ROLE phase0_owner LOGIN NOINHERIT PASSWORD '{password}'")
    database='phase0_http_market_integrity_test'
    cmd('docker','exec',name,'createdb','-U','postgres','-O','phase0_owner',database)
    cmd('docker','exec',name,'psql','-U','postgres','-d',database,'-c','CREATE EXTENSION postgis')
    url=f'postgresql+asyncpg://phase0_owner:{password}@127.0.0.1:{port}/{database}'
    guard=run/'guard'; guard.mkdir(exist_ok=True)
    (guard/'sitecustomize.py').write_bytes((ROOT/'tests/phase0/network_guard.py').read_bytes())
    env={'PATH':os.environ['PATH'],'HOME':str(run),'ENVIRONMENT':'test','RELEASE_SHA':cmd('git','rev-parse','HEAD',capture_output=True).stdout.strip(),'JWT_SECRET':'test-secret-key-for-testing-minimum-32-chars','JWT_ISSUER':'verdaxis-test-api','JWT_AUDIENCE':'verdaxis-test-web','KYC_EXECUTION_GATE_ENABLED':'false','DATABASE_URL':url,'MIGRATOR_DATABASE_URL':url,'PHASE0_PORT':port,'PYTHONPATH':f'{guard}:{ROOT}','DISPOSABLE_ITEST_PASSWORD':'Synthetic-HTTP-Password-42!','DISPOSABLE_TEST_TOKEN':uuid.uuid4().hex}
    python=str(ROOT/'venv/bin/python')
    cmd(python,'-m','alembic','-c',str(ROOT/'alembic.ini'),'upgrade','head',env=env,cwd=ROOT)
    cmd(python,str(ROOT/'tests/phase0/http_seed.py'),env=dict(env,DATABASE_URL=url.replace('phase0_owner:','postgres:')),cwd=run)
    policy='/tmp/runtime-policy'
    cmd('docker','exec',name,'mkdir','-p',policy)
    for filename in ('app_acl_policy.sql','converge_runtime_object_acls.sql','bootstrap_roles.sql','validate_roles.sql'):
        cmd('docker','cp',str(ROOT/'deploy/postgres'/filename),f'{name}:{policy}/{filename}')
    variables=['-v',f'database_name={database}','-v','app_role=phase0_app_test','-v','migrator_role=phase0_owner','-v','backup_role=phase0_backup_test']
    for filename in ('bootstrap_roles.sql','validate_roles.sql'):
        cmd('docker','exec',name,'psql','-X','-U','postgres','-d',database,*variables,'-f',f'{policy}/{filename}')
    cmd('docker','exec',name,'psql','-U','postgres','-c',f"ALTER ROLE phase0_app_test PASSWORD '{password}'")
    env['DATABASE_URL']=url.replace('phase0_owner:','phase0_app_test:')
    with socket.socket() as sock:
        sock.bind(('127.0.0.1',0)); http_port=sock.getsockname()[1]
    assert http_port>=49152
    log=(run/'server.log').open('w')
    server=subprocess.Popen([python,'-m','tests.disposable_server','--port',str(http_port)],env=env,cwd=run,stdout=log,stderr=log)
    base=f'http://127.0.0.1:{http_port}'
    for _ in range(120):
        try:
            with urllib.request.urlopen(base+'/health/ready',timeout=2) as response:
                if response.status==200:break
        except Exception:pass
        if server.poll() is not None:raise RuntimeError('server exited')
        time.sleep(.5)
    else:raise RuntimeError('HTTP server not ready')
    # Test client may connect only to the owned HTTP server; backend only to DB.
    result=subprocess.run([python,'-m','pytest','tests/integration','-q','-rs','--run-disposable-integration','--disposable-target-url',base,'--disposable-target-token',env['DISPOSABLE_TEST_TOKEN'],'--junitxml=work/task9-http.xml'],env=dict(env,PHASE0_PORT=str(http_port)),cwd=ROOT)
    sys.exit(result.returncode)
finally:
    if server:
        server.terminate()
        try:server.wait(timeout=20)
        except subprocess.TimeoutExpired:server.kill();server.wait()
    if log:log.close()
    cmd('docker','rm','-f','-v',name)
