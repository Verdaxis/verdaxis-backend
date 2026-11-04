"""Observe a disposable maintenance child's acquired advisory lock."""
import os

if os.environ.get("P89_PROTOCOL_CHILD") == "1":
    import asyncio
    from pathlib import Path

    import asyncpg
    from sqlalchemy import event
    from sqlalchemy.engine import Engine
    seen=set()

    def observe(connection,cursor,statement,parameters,context,executemany):
        if "pg_advisory_xact_lock(" not in statement.lower():return
        url=connection.engine.url
        if url.database in seen:return
        assert url.host=="127.0.0.1" and url.database.endswith("_test")
        seen.add(url.database)
        pid=connection.connection.driver_connection.get_server_pid()
        async def verify():
            observer=await asyncpg.connect(url.set(drivername="postgresql").render_as_string(hide_password=False))
            try:
                held=await observer.fetchval("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND granted)",pid)
                assert held,"maintenance child backend must hold its advisory lock"
            finally:await observer.close()
        # The SQLAlchemy greenlet runs inside the application event loop.
        import threading
        errors=[]
        def check():
            try:asyncio.run(verify())
            except BaseException as error:errors.append(error)
        thread=threading.Thread(target=check);thread.start();thread.join()
        if errors:raise errors[0]
        Path(os.environ["P89_PROTOCOL_MARKER"]).write_text(str(pid))
    event.listen(Engine,"after_cursor_execute",observe)
