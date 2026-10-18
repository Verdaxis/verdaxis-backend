"""Task 9 disposable preferences differential. Uses the existing lifecycle owner."""

import hashlib
import json
import os
import subprocess
import urllib.error
import urllib.request
from datetime import UTC, datetime, timedelta
from pathlib import Path

from scripts.rust_contract.compare import compare_responses
from scripts.rust_contract.phase2_seed import (
    APP_ROLE,
    BACKUP_ROLE,
    DATABASES,
    MIGRATOR_ROLE,
    PASSWORD,
    connect,
    digest,
    identifier,
    snapshot,
)
from scripts.rust_contract.phase2_state import compare_state

ROOT = Path(__file__).resolve().parents[2]
ACTOR = "20000000-0000-4000-8000-000000000001"
OTHER = "20000000-0000-4000-8000-000000000002"


def request(endpoint, method, path, body, headers):
    req = urllib.request.Request(endpoint + path, data=body, method=method, headers=headers)
    try:
        response = urllib.request.urlopen(req, timeout=30)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        return {"status": response.status, "headers": dict(response.headers), "body": response.read()}


def run_preferences(runner, args):
    from scripts.rust_contract.run_disposable import (
        DB_NAME,
        RunnerError,
        attest,
        docker,
        ephemeral_port,
        wait_for_port,
    )

    port = runner.start_database()
    env = {**os.environ, "ENVIRONMENT": "test", "RELEASE_SHA": runner.sha,
           "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars", **runner.urls(port)}
    runner.seed = False
    runner.migrate_and_seed(env)
    with connect(port, DB_NAME) as source:
        source.execute("INSERT INTO organizations(id,name,domain,type) VALUES ('10000000-0000-4000-8000-000000000001','Preferences fixture','preferences.invalid','FUEL_BUYER')")
        for user, email in ((ACTOR, "actor"), (OTHER, "other")):
            source.execute("INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password) VALUES (%s,%s,'not-a-login','BUYER','APPROVED','10000000-0000-4000-8000-000000000001',true,false)", (user, email + "@preferences.invalid"))
        source.execute("INSERT INTO user_preferences(id,user_id,namespace,value,updated_at) VALUES ('30000000-0000-4000-8000-000000000001',%s,'legacy_unknown','[1,{\"keep\":null}]','2026-10-18T00:00:00Z')", (ACTOR,))
    with connect(port, "postgres", autocommit=True) as admin:
        for database in DATABASES:
            admin.execute(f"CREATE DATABASE {identifier(database)} TEMPLATE {identifier(DB_NAME)}")
    policy_dir = "/tmp/preferences-role-policy"
    docker("exec", runner.container, "mkdir", "-p", policy_dir)
    for name in ("app_acl_policy.sql", "converge_runtime_object_acls.sql", "bootstrap_roles.sql", "validate_roles.sql"):
        docker("cp", str(ROOT / "deploy/postgres" / name), f"{runner.container}:{policy_dir}/{name}")
    for database in DATABASES:
        for script in ("bootstrap_roles.sql", "validate_roles.sql"):
            docker("exec", runner.container, "psql", "-X", "-U", "postgres", "-d", database,
                   "-v", f"database_name={database}", "-v", f"app_role={APP_ROLE}",
                   "-v", f"migrator_role={MIGRATOR_ROLE}", "-v", f"backup_role={BACKUP_ROLE}",
                   "-f", f"{policy_dir}/{script}")
    with connect(port, DATABASES[0]) as admin:
        admin.execute(f"ALTER ROLE {identifier(APP_ROLE)} PASSWORD '{PASSWORD}'")
    initial = []
    for database in DATABASES:
        with connect(port, database) as reader:
            initial.append(snapshot(reader))
    if compare_state(*initial):
        raise RunnerError("preferences twin seed differs")
    subprocess.run(["cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read",
                    "--features", "contract-tests", "--example", "preferences_server"], cwd=ROOT,
                   env={**env, "SQLX_OFFLINE": "true"}, check=True)
    endpoints = []
    for runtime, database in zip(("python", "rust"), DATABASES):
        bind = ephemeral_port()
        url = f"postgresql://{APP_ROLE}:{PASSWORD}@127.0.0.1:{port}/{database}"
        cell_env = {**env, "DATABASE_URL": url.replace("postgresql://", "postgresql+asyncpg://"),
                    "PHASE2_DATABASE_URL": url, "PHASE2_BIND": f"127.0.0.1:{bind}",
                    "DISPOSABLE_TEST_TOKEN": runner.token}
        command = [runner.python_bin, "-m", "tests.rust_contract.phase2.preferences_server", "--port", str(bind)] if runtime == "python" else [str(ROOT / "rs/target/debug/examples/preferences_server")]
        runner.spawn(command, cell_env)
        wait_for_port(bind)
        endpoint = f"http://127.0.0.1:{bind}"
        attest(endpoint, runner.token)
        endpoints.append(endpoint)
    import jwt
    now = datetime.now(UTC)
    tokens = {user: jwt.encode({"sub": user, "type": "access", "iss": "verdaxis-test-api", "aud": "verdaxis-test-web",
                               "iat": int(now.timestamp()), "exp": int((now + timedelta(hours=2)).timestamp())}, env["JWT_SECRET"], algorithm="HS256") for user in (ACTOR, OTHER)}
    rules = hashlib.sha256((ROOT / "tests/rust_contract/phase2/normalization.json").read_bytes()).hexdigest()
    actions = []
    fixtures = json.loads((ROOT / "tests/rust_contract/phase2/preferences.json").read_text())
    cases = [("read-unknown", "GET", "", None, ACTOR)]
    cases += [(case["id"], "PUT", "/" + case["namespace"], case["raw"].encode(), ACTOR) for case in fixtures["vectors"]]
    cases += [("repeat-first", "PUT", "/tutorial", b'{"completed":true}', ACTOR),
              ("repeat-second", "PUT", "/tutorial", b'{"completed":true}', ACTOR),
              ("other-isolated", "GET", "", None, OTHER),
              ("read-final", "GET", "", None, ACTOR)]
    for index, (name, method, suffix, body, user) in enumerate(cases):
        clock = (datetime(2026, 10, 19, tzinfo=UTC) + timedelta(seconds=index)).isoformat()
        row_id = f"40000000-0000-4000-8000-{index:012d}"
        headers = {"content-type": "application/json", "authorization": "Bearer " + tokens[user],
                   "x-request-id": "preferences-contract", "x-disposable-token": runner.token,
                   "x-test-preference-clock": clock, "x-test-preference-id": row_id,
                   "x-forwarded-for": f"192.0.2.{index + 1}"}
        responses = [request(endpoint, method, "/api/users/me/preferences" + suffix, body, headers) for endpoint in endpoints]
        print("PREFERENCES_ACTION", name, [(r["status"], r["headers"].get("content-type", r["headers"].get("Content-Type"))) for r in responses], flush=True)
        if name == "empty-namespace":
            # The JSON profile deliberately refuses redirects. Check the empty
            # body and exact route-local Location separately, without widening it.
            for endpoint, response in zip(endpoints, responses):
                location = next((v for k, v in response["headers"].items() if k.lower() == "location"), None)
                if response["status"] != 307 or response["body"] or location != endpoint + "/api/users/me/preferences":
                    raise RunnerError("preferences slash redirect differs")
            differences = []
        else:
            differences = compare_responses(*responses, profile="phase2-meaning-v1", route_id="P01" if method == "GET" else "P02", rules_sha256=rules, request_id="preferences-contract")
        if differences:
            raise RunnerError(f"preferences response {name}: {differences}")
        states = []
        for database in DATABASES:
            with connect(port, database) as reader:
                states.append(snapshot(reader))
        if compare_state(*states):
            raise RunnerError(f"preferences committed state {name} differs")
        for table in initial[0]:
            if table != "user_preferences" and states[0][table] != initial[0][table]:
                raise RunnerError(f"preferences changed unrelated table {table}")
        if name == "other-isolated" and json.loads(responses[0]["body"]) != {}:
            raise RunnerError("preferences crossed user boundary")
        actions.append({"id": name, "status": responses[0]["status"], "state_sha256": digest(states[0]), "differences": []})
    # Hold the actor row until both requests are observed waiting in PostgreSQL.
    # The same exact value/clock/ID makes either accepted ordering equivalent.
    import time
    from concurrent.futures import ThreadPoolExecutor
    for endpoint, database in zip(endpoints, DATABASES):
        with connect(port, database) as blocker:
            blocker.execute("SELECT id FROM users WHERE id=%s FOR UPDATE", (ACTOR,))
            with ThreadPoolExecutor(max_workers=2) as workers:
                futures = [workers.submit(request, endpoint, "PUT", "/api/users/me/preferences/tutorial",
                    b'{"completed":false}', {**headers, "authorization": "Bearer " + tokens[ACTOR],
                    "x-test-preference-clock": "2026-10-19T01:00:00+00:00", "x-forwarded-for": "192.0.2.200"}) for _ in range(2)]
                deadline = time.monotonic() + 15
                with connect(port, database, autocommit=True) as observer:
                    while True:
                        waiting = observer.execute("SELECT count(*) AS count FROM pg_stat_activity WHERE datname=%s AND wait_event_type='Lock' AND usename=%s", (database, APP_ROLE)).fetchone()[0]
                        if waiting >= 2:
                            break
                        if time.monotonic() > deadline:
                            blocker.rollback()
                            raise RunnerError("concurrent preferences did not reach user lock")
                blocker.commit()
                if [future.result()["status"] for future in futures] != [200, 200]:
                    raise RunnerError("concurrent preference PUT failed")
    states = []
    for database in DATABASES:
        with connect(port, database) as reader:
            states.append(snapshot(reader))
    if compare_state(*states):
        raise RunnerError("concurrent preferences state differs")
    actions.append({"id": "same-user-concurrent-put", "status": 200, "state_sha256": digest(states[0]), "differences": []})
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps({"source_sha": runner.sha, "scenario": "preferences", "scope": "task9-component",
            "seed_sha256": digest(initial[0]), "actions": actions, "dispatchers": "not-started"}, indent=2) + "\n")
