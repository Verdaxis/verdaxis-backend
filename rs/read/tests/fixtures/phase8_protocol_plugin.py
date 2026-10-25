"""Compare the actual writer statement under Python and Rust lock holders."""
import asyncio
import os
import subprocess
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import asyncpg
import pytest
from sqlalchemy import event
from sqlalchemy.engine import Engine


def wire_parameters(parameters):
    return tuple(parameters) if not isinstance(parameters, dict) else tuple(parameters.values())


async def statement_outcome(url, statement, parameters, key, holder, early_release):
    dsn=url.set(drivername="postgresql").render_as_string(hide_password=False)
    writer=await asyncpg.connect(dsn)
    blocker=None
    process=None
    directory=Path(tempfile.mkdtemp(prefix="p89-holder-"))
    release=directory/"release"
    ready=directory/"ready"
    try:
        if holder=="python":
            blocker=await asyncpg.connect(dsn)
            await blocker.execute("BEGIN")
            await blocker.execute("SELECT pg_advisory_xact_lock($1)",key)
        else:
            process=subprocess.Popen([os.environ["RUST_LOCK_PROTOCOL_BINARY"],"ordered_protocol_contender","--exact","--include-ignored","--nocapture"],
                env={**os.environ,"RUST_PROTOCOL_DATABASE_URL":dsn,"RUST_PROTOCOL_KEY":str(key),"RUST_PROTOCOL_READY":str(ready),"RUST_PROTOCOL_RELEASE":str(release)},
                stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
            for _ in range(1000):
                if ready.exists():break
                assert process.poll() is None, process.communicate()[0]
                await asyncio.sleep(.01)
            assert ready.exists(),"Rust holder must confirm acquisition"
        await writer.execute("BEGIN")
        await writer.execute("SET LOCAL lock_timeout='500ms'")
        await writer.execute("SET LOCAL statement_timeout='5s'")
        async def unlock():
            await asyncio.sleep(.05)
            if blocker is not None:await blocker.execute("ROLLBACK")
            else:
                release.write_text("release")
                await asyncio.to_thread(process.wait,timeout=10)
        task=asyncio.create_task(unlock()) if early_release else None
        start=time.monotonic()
        try:
            value=await writer.fetchval(statement,*wire_parameters(parameters))
            outcome=("ok",value)
        except asyncpg.PostgresError as error:
            assert error.sqlstate!="40P01","no side may deadlock"
            outcome=("refused",error.sqlstate)
        elapsed=time.monotonic()-start
        assert elapsed<1.5,"writer exceeds lock timeout plus margin"
        await writer.execute("ROLLBACK")
        if task is not None:
            await task
            if "pg_try_advisory" in statement.lower():
                await writer.execute("BEGIN")
                value=await writer.fetchval(statement,*wire_parameters(parameters))
                await writer.execute("ROLLBACK")
                outcome=("ok",value)
        if early_release:assert outcome[0]=="ok" and outcome[1] is not False
        return outcome
    finally:
        if blocker is not None:await blocker.close()
        release.write_text("release")
        if process is not None:
            output=await asyncio.to_thread(process.communicate,timeout=10)
            assert process.returncode==0,output
            assert "40P01" not in output[0]
        await writer.close()


def compare_statement(url,statement,parameters):
    key=int(wire_parameters(parameters)[0])
    python_outcome=asyncio.run(statement_outcome(url,statement,parameters,key,"python",False))
    rust_outcome=asyncio.run(statement_outcome(url,statement,parameters,key,"rust",False))
    assert python_outcome == rust_outcome
    assert rust_outcome==(("ok",False) if "pg_try_advisory" in statement.lower() else ("refused","55P03"))
    asyncio.run(statement_outcome(url,statement,parameters,key,"rust",True))
    print("PROTOCOL_PARITY",url.database,"55P03",flush=True)


@pytest.fixture(autouse=True)
def rust_protocol_contender(request):
    seen=set()
    failures=[]
    futures=[]
    workers=ThreadPoolExecutor(max_workers=1)
    guard=threading.Lock()
    def observe(connection,cursor,statement,parameters,context,executemany):
        if not any(name in statement.lower() for name in ("pg_advisory_xact_lock(","pg_try_advisory_xact_lock(")):return
        url=connection.engine.url
        assert url.host=="127.0.0.1" and url.database.endswith("_test")
        key=(url.database,request.node.nodeid)
        with guard:
            if key in seen:return
            seen.add(key)
        futures.append(workers.submit(compare_statement,url,statement,parameters))
    marker=Path(tempfile.mkdtemp(prefix="p89-protocol-"))/"child"
    original_run=subprocess.run
    def child_run(*args,**kwargs):
        env=kwargs.get("env")
        if env is not None:kwargs["env"]={**env,"PYTHONPATH":os.environ["PYTHONPATH"],"RUST_LOCK_PROTOCOL_BINARY":os.environ["RUST_LOCK_PROTOCOL_BINARY"],"P89_PROTOCOL_CHILD":"1","P89_PROTOCOL_MARKER":str(marker)}
        return original_run(*args,**kwargs)
    request.getfixturevalue("monkeypatch").setattr(subprocess,"run",child_run)
    event.listen(Engine,"before_cursor_execute",observe)
    yield
    event.remove(Engine,"before_cursor_execute",observe)
    workers.shutdown(wait=True)
    for future in futures:future.result()
    if "copied_leases" in request.node.nodeid:
        # This case contains only delivery leases; the scan protocol is exercised separately.
        assert request.node.rep_call.passed
    else:
        assert seen or marker.exists(),"actual writer advisory sequence must be observed"


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    outcome=yield
    report=outcome.get_result()
    setattr(item,"rep_"+report.when,report)
