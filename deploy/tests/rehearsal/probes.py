"""Local HTTPS observations for the disposable systemd rehearsal.

seed(database, private_dir=None) creates one private auth fixture.  probe returns
only safe status values and writes the same observations to public_file.  HTTP
bodies, cookies, credentials and database row images remain in private_dir.
"""
from __future__ import annotations

import base64
import hashlib
import json
import os
import re
import secrets
import subprocess
import time
import uuid
from http.cookies import SimpleCookie
from pathlib import Path
from urllib.parse import urlencode

ENVIRONMENTS = {
    "production": ("verdaxis_prod", "api.verdaxis.exchange"),
    "staging": ("verdaxis_staging", "api-staging.verdaxis.exchange"),
}
# These are observations, not market fixtures.  No economic rows are inserted.
MARKET_TABLES = (
    "organizations", "orderbook_orders", "inventory_items", "trades",
    "benchmarks", "live_slice_benchmarks", "market_signal_ingestion_runs",
    "market_indications", "fair_price_bands", "physical_stems", "seed_runs",
    "market_row_quarantines", "organization_market_approvals",
)


class ProbeError(RuntimeError):
    """A safe, fixed diagnostic suitable for the public result."""


def _require(condition, message):
    if not condition:
        raise ProbeError(message)


def _guard(database):
    _require(os.environ.get("VERDAXIS_DISPOSABLE_DEPLOY") == "1",
             "disposable gate is absent")
    _require(database in {entry[0] for entry in ENVIRONMENTS.values()},
             "database is not a rehearsal target")
    marker = Path("/run/verdaxis-disposable-rehearsal")
    _require(marker.is_file() and not marker.is_symlink(),
             "disposable host marker is absent")
    _require(os.geteuid() == 0, "owner observations require root")


def _private(path):
    path = Path(path)
    _require(not path.is_symlink(), "private directory is a link")
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    _require(path.stat().st_uid == os.geteuid(), "private directory owner differs")
    _require(path.stat().st_mode & 0o077 == 0, "private directory is accessible to others")
    return path


def _write_private(path, value):
    data = json.dumps(value, sort_keys=True, indent=2).encode() + b"\n"
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
    with os.fdopen(os.open(path, flags, 0o600), "wb") as stream:
        stream.write(data)


def _psql(database, sql):
    # Fix socket, port and role; do not inherit libpq routing or startup files.
    env = {"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "LC_ALL": "C"}
    result = subprocess.run(
        ["runuser", "-u", "postgres", "--", "psql", "-XqAt",
         "-v", "ON_ERROR_STOP=1", "-h", "/var/run/postgresql", "-p", "5432",
         "-U", "postgres", "-d", database],
        input=sql.encode(), capture_output=True,
        env=env, timeout=45, check=False,
    )
    _require(result.returncode == 0, "owner database observation failed")
    return result.stdout.decode().strip()


def seed(database, private_dir=None):
    """Insert an approved bcrypt user as postgres; return no credentials.

    private_dir defaults to the driver's PRIVATE environment variable.  Call
    once per new disposable database, before starting either release.
    """
    _guard(database)
    private = _private(private_dir if private_dir is not None else os.environ["PRIVATE"])
    credential_file = private / "probe-credentials.json"
    _require(not credential_file.exists(), "auth fixture already exists")
    user_id = str(uuid.uuid4())
    password = secrets.token_urlsafe(36)
    email = f"rehearsal-{user_id}@example.invalid"
    source = Path(os.environ["REHEARSAL_SOURCE_DIR"])
    result = subprocess.run(
        [str(source / "venv/bin/python"), "-c",
         ("import bcrypt,sys; print(bcrypt.hashpw(sys.stdin.buffer.read(), "
          "bcrypt.gensalt(rounds=12)).decode())")],
        input=password.encode(), capture_output=True,
        timeout=45, check=False,
    )
    _require(result.returncode == 0, "bcrypt fixture generation failed")
    password_hash = result.stdout.decode().strip()
    _require(re.fullmatch(r"\$2[aby]\$12\$[./A-Za-z0-9]{53}", password_hash) is not None,
             "bcrypt fixture has an unexpected format")
    # Persist first so an interrupted seed never loses the fixture identity.
    _write_private(credential_file, {"database": database, "user_id": user_id,
                                     "email": email, "password": password})
    sql = f"""BEGIN;
DO $$ BEGIN
IF current_user <> 'postgres' OR current_database() <> '{database}' THEN
    RAISE EXCEPTION 'rehearsal database identity differs';
END IF;
END; $$;
INSERT INTO public.users
(id,email,password_hash,role,status,email_verified,must_change_password)
VALUES ('{user_id}','{email}','{password_hash}','BUYER','APPROVED',true,false);
COMMIT;
"""
    _psql(database, sql)
    return {"seeded": True}


def _market_snapshot(database, path):
    statements = ["BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;",
                  "SET LOCAL statement_timeout='15s';"]
    for table in MARKET_TABLES:
        statements.append(
            f"SELECT json_build_object('table','{table}','count',count(*),"
            "'rows',coalesce(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text "
            f"COLLATE \"C\"),'[]'::jsonb)) FROM public.{table} t;"
        )
    statements.append("COMMIT;")
    raw = _psql(database, "\n".join(statements))
    rows = [json.loads(line) for line in raw.splitlines()]
    _require(len(rows) == len(MARKET_TABLES), "market snapshot is incomplete")
    _write_private(path, rows)
    return {
        row["table"]: {
            "count": row["count"],
            "sha256": hashlib.sha256(json.dumps(row["rows"], sort_keys=True,
                                                 separators=(",", ":")).encode()).hexdigest(),
        }
        for row in rows
    }


def _sessions(database, user_id, path):
    raw = _psql(database, f"""BEGIN READ ONLY;
SELECT coalesce(jsonb_agg(to_jsonb(s) ORDER BY id),'[]'::jsonb)
FROM public.refresh_sessions s WHERE user_id='{user_id}';
COMMIT;
""")
    rows = json.loads(raw)
    _write_private(path, rows)
    return rows


def _claims(token):
    # Used only to locate the row issued by the real HTTP service.  Successful
    # bearer /me requests, not this decoding, establish token acceptance.
    encoded = token.split(".")[1]
    return json.loads(base64.urlsafe_b64decode(encoded + "=" * (-len(encoded) % 4)))


def _digest(value):
    return hashlib.sha256(value.encode()).hexdigest()


class _HTTPS:
    def __init__(self, domain, private, ca):
        self.domain = domain
        self.private = private
        self.ca = ca
        self.jar = private / "cookies.txt"
        self.sequence = 0
        self.statuses = []
        self.retries = []
        self.retry_wait_seconds = 0

    def request(self, method, route, *, payload=None, bearer=None, cookie=None, json_body=None):
        self.sequence += 1
        prefix = self.private / f"http-{self.sequence:02d}"
        headers = Path(str(prefix) + ".headers")
        body = Path(str(prefix) + ".body")
        # Secrets travel in a private curl config on stdin, never in argv.
        options = {"request": method, "url": f"https://{self.domain}{route}",
                   "resolve": f"{self.domain}:443:127.0.0.1", "cacert": str(self.ca),
                   "dump-header": str(headers), "output": str(body),
                   "cookie-jar": str(self.jar), "proto": "=https", "noproxy": "*",
                   "connect-timeout": "5", "max-time": "25", "max-filesize": "1048576",
                   "write-out": "%{http_code}"}
        lines = [f"{key} = {json.dumps(value)}" for key, value in options.items()]
        lines.extend(["silent", "show-error"])
        lines.append(f'header = "Origin: https://{self.domain}"')
        if bearer is not None:
            lines.append(f"header = {json.dumps('Authorization: Bearer ' + bearer)}")
        if cookie is not None:
            lines.append(f"cookie = {json.dumps(cookie)}")
        elif self.jar.exists():
            lines.append(f"cookie = {json.dumps(str(self.jar))}")
        if payload is not None:
            lines.append(f"data = {json.dumps(urlencode(payload))}")
        if json_body is not None:
            lines.append('header = "Content-Type: application/json"')
            lines.append(f"data = {json.dumps(json.dumps(json_body))}")
        previous_mask = os.umask(0o077)
        try:
            result = subprocess.run(
                ["curl", "--disable", "--config", "-"],
                input=("\n".join(lines) + "\n").encode(),
                capture_output=True,
                env={"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "LC_ALL": "C"},
                timeout=30, check=False,
            )
        finally:
            os.umask(previous_mask)
        _write_private(Path(str(prefix) + ".transport.json"),
                       {"exit_code": result.returncode,
                        "stderr": result.stderr.decode(errors="replace")})
        _require(result.returncode == 0, "local verified HTTPS request failed")
        status = int(result.stdout)
        self.statuses.append(status)
        values = SimpleCookie()
        for line in headers.read_text().splitlines():
            if line.lower().startswith("set-cookie:"):
                values.load(line.split(":", 1)[1].strip())
        if status == 429:
            delays = [line.split(":", 1)[1].strip() for line in headers.read_text().splitlines()
                      if line.lower().startswith("retry-after:")]
            _require(len(delays) == 1 and delays[0].isdigit(),
                     "rate limit has no numeric retry delay")
            delay = int(delays[0])
            _require(0 < delay <= 120 and self.retry_wait_seconds + delay <= 120
                     and len(self.retries) < 3, "rate limit retry budget was exhausted")
            safe_route = re.sub(r"[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}", "{id}", route)
            self.retries.append({"method": method, "route": safe_route, "delay_seconds": delay})
            self.retry_wait_seconds += delay
            time.sleep(delay)
            return self.request(method, route, payload=payload, bearer=bearer,
                                cookie=cookie, json_body=json_body)
        raw_body = body.read_text()
        response = json.loads(raw_body) if raw_body.strip() else None
        return status, response, values


def _check_cookies(cookies):
    for name in ("refresh_token", "device_session"):
        _require(name in cookies and bool(cookies[name].value), "session cookie is absent")
        item = cookies[name]
        _require(bool(item["secure"]) and bool(item["httponly"])
                 and item["samesite"].lower() == "lax"
                 and item["path"] == "/api/auth" and item["max-age"] == "604800",
                 "session cookie attributes differ")


def _access(status, body):
    _require(status == 200 and body.get("token_type") == "bearer"
             and isinstance(body.get("access_token"), str)
             and bool(body["access_token"]), "access token was not issued")
    return body["access_token"]


def _me(http, token, credential):
    status, body, _ = http.request("GET", "/api/auth/me", bearer=token)
    _require(status == 200 and body.get("id") == credential["user_id"]
             and body.get("email") == credential["email"]
             and body.get("role") == "BUYER" and body.get("status") == "APPROVED",
             "bearer profile does not match the approved fixture")


def _authenticated_mutations(http, token):
    """Exercise supported personal state routes, without market publication."""
    status, preferences, _ = http.request("GET", "/api/users/me/preferences", bearer=token)
    _require(status == 200 and isinstance(preferences, dict),
             "personal preference baseline is unavailable")
    original = preferences.get("tutorial")
    # A missing tutorial record is created through its supported upsert route.
    # Its neutral value then becomes the persistent baseline for later calls.
    baseline = original if original is not None else {"completed": False}
    watchlist_id = None
    checks = {}
    try:
        status, body, _ = http.request("PUT", "/api/users/me/preferences/tutorial",
                                      bearer=token, json_body=baseline)
        _require(status == 200 and body.get("value") == baseline,
                 "personal preference baseline upsert failed")
        changed = {"completed": not baseline["completed"]}
        status, body, _ = http.request("PUT", "/api/users/me/preferences/tutorial",
                                      bearer=token, json_body=changed)
        _require(status == 200 and body.get("value") == changed,
                 "personal preference update failed")
        status, body, _ = http.request("GET", "/api/users/me/preferences", bearer=token)
        _require(status == 200 and body.get("tutorial") == changed,
                 "personal preference read did not observe the update")
        checks["preference_upsert_update_read"] = True
        name = "Rehearsal " + uuid.uuid4().hex
        status, body, _ = http.request("POST", "/api/watchlists", bearer=token,
                                      json_body={"name": name})
        _require(status == 201 and isinstance(body, dict), "personal watchlist create failed")
        watchlist_id = str(uuid.UUID(body["id"]))
        _require(body.get("name") == name, "created watchlist name differs")
        status, body, _ = http.request("GET", "/api/watchlists/" + watchlist_id, bearer=token)
        _require(status == 200 and body.get("id") == watchlist_id and body.get("name") == name,
                 "personal watchlist read differs")
        checks["watchlist_create_read"] = True
    finally:
        if watchlist_id is not None:
            route = "/api/watchlists/" + watchlist_id
            status, _, _ = http.request("DELETE", route, bearer=token)
            _require(status == 204, "personal watchlist cleanup failed")
            status, _, _ = http.request("GET", route, bearer=token)
            _require(status == 404, "deleted personal watchlist remains readable")
            checks["watchlist_delete_and_absence"] = True
        status, body, _ = http.request("PUT", "/api/users/me/preferences/tutorial",
                                      bearer=token, json_body=baseline)
        _require(status == 200 and body.get("value") == baseline,
                 "personal preference restore failed")
        status, body, _ = http.request("GET", "/api/users/me/preferences", bearer=token)
        _require(status == 200 and body.get("tutorial") == baseline,
                 "restored personal preference read differs")
        checks["preference_restore_read"] = True
    return checks


def _observe_command(args):
    result = subprocess.run(args, capture_output=True,
                            env={"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "LC_ALL": "C"},
                            timeout=30, check=False)
    _require(result.returncode == 0, "service observation command failed")
    return result.stdout.decode()


def _service_snapshot(unit, path):
    properties = ("NRestarts", "MemoryCurrent", "MemoryPeak", "MemoryMax", "MainPID",
                  "ActiveState", "InvocationID")
    raw = _observe_command(["systemctl", "show", unit, "--no-pager",
                            "--property=" + ",".join(properties)])
    values = dict(line.split("=", 1) for line in raw.splitlines() if "=" in line)
    _write_private(path, values)
    _require(set(values) == set(properties) and values["ActiveState"] == "active",
             "service observation is incomplete or inactive")
    for name in ("NRestarts", "MemoryCurrent", "MemoryPeak", "MemoryMax", "MainPID"):
        _require(values[name].isdigit(), "service numeric observation is unavailable")
        values[name] = int(values[name])
    _require(values["MainPID"] > 0 and values["MemoryMax"] > 0,
             "service process or memory limit is absent")
    return values


def _journal_cursor():
    raw = _observe_command(["journalctl", "--no-pager", "-n", "1", "--output=json"])
    entries = [json.loads(line) for line in raw.splitlines() if line]
    _require(len(entries) == 1 and bool(entries[0].get("__CURSOR")),
             "journal start cursor is absent")
    return entries[0]["__CURSOR"]


def _journal_errors(unit, cursor, path):
    # Flush pending journal messages before reading the bounded interval.
    _observe_command(["journalctl", "--sync"])
    # Query only entries created after the start cursor, never an arbitrary tail.
    raw = _observe_command(["journalctl", "--no-pager", "--unit=" + unit,
                            "--after-cursor=" + cursor, "--output=json", "-n", "10001"])
    _require(len(raw.encode()) <= 8 * 1024 * 1024, "journal observation exceeded its bound")
    entries = [json.loads(line) for line in raw.splitlines() if line]
    _write_private(path, entries)
    _require(len(entries) <= 10000, "journal observation exceeded its entry bound")
    counts = {"priority_error": 0, "application_error": 0, "panic_or_traceback": 0}
    for entry in entries:
        message = str(entry.get("MESSAGE", ""))
        priority = str(entry.get("PRIORITY", ""))
        if priority.isdigit() and int(priority) <= 3:
            counts["priority_error"] += 1
        if re.search(r"\b(?:ERROR|FATAL)\b|\"level\"\s*:\s*\"(?:error|fatal)\"", message, re.IGNORECASE):
            counts["application_error"] += 1
        if re.search(r"panicked at|thread .*panicked|Traceback \(most recent call last\)", message):
            counts["panic_or_traceback"] += 1
    return counts


def probe(checkout, environment, release_sha, private_dir, public_file, *, require_empty=True):
    """Observe login, rotation, replay refusal and unchanged market state.

    The returned mapping has auth/session/passive_harm status strings and safe
    details.  Each call uses a new private directory and a new login family.
    The replay intentionally revokes that call's family after its grace ends.
    Set require_empty=False only after the caller independently verifies its
    known demo baseline.  This does not check demo job effects or provenance.
    """
    _require(environment in ENVIRONMENTS, "environment is not a rehearsal target")
    database, domain = ENVIRONMENTS[environment]
    _guard(database)
    _require(re.fullmatch(r"[a-f0-9]{40}", release_sha) is not None,
             "release SHA is not full length")
    checkout = Path(checkout)
    _require(checkout.is_dir(), "release checkout is absent")
    private = _private(private_dir)
    ca = private / "caddy-ca.pem"
    _require(ca.is_file() and not ca.is_symlink(), "local CA file is absent")
    credential_file = private / "probe-credentials.json"
    _require(not credential_file.is_symlink()
             and credential_file.stat().st_mode & 0o077 == 0,
             "auth fixture permissions differ")
    credential = json.loads(credential_file.read_text())
    _require(credential["database"] == database, "auth fixture database differs")
    _require(str(uuid.UUID(credential["user_id"])) == credential["user_id"],
             "auth fixture identity is invalid")
    run_dir = _private(private / ("probe-" + uuid.uuid4().hex))
    http = _HTTPS(domain, run_dir, ca)
    result = {"schema_version": 1, "scope": "disposable-candidate", "environment": environment,
              "release_sha": release_sha, "auth": "FAIL", "session": "FAIL", "passive_harm": "FAIL",
              "require_empty": require_empty, "checks": {}, "failures": []}
    before = None
    service_before = None
    cursor = None
    unit = "verdaxis-rust" + ("-staging" if environment == "staging" else "") + ".service"
    try:
        cursor = _journal_cursor()
        service_before = _service_snapshot(unit, run_dir / "service-before.json")
        before = _market_snapshot(database, run_dir / "market-before.json")
        status, ready, _ = http.request("GET", "/health/ready")
        _require(status == 200 and ready == {"status": "ok", "db": "ok",
                 "environment": environment, "release_sha": release_sha}, "readiness identity differs")
        result["checks"]["readiness_identity"] = True
        result["observed_release_sha"] = ready["release_sha"]
        status, body, cookies = http.request("POST", "/api/auth/login", payload={
            "username": credential["email"], "password": credential["password"]})
        access = _access(status, body)
        _check_cookies(cookies)
        _me(http, access, credential)
        result["auth"] = "PASS"
        result["checks"]["approved_login_and_bearer_profile"] = True
        result["checks"].update(_authenticated_mutations(http, access))
        result["checks"]["authenticated_personal_state_operations"] = True
        old_refresh = cookies["refresh_token"].value
        device = cookies["device_session"].value
        old_claims = _claims(old_refresh)
        old_hash = _digest(old_claims["jti"])
        rows = _sessions(database, credential["user_id"], run_dir / "sessions-login.json")
        old = [row for row in rows if row["jti_hash"] == old_hash]
        _require(len(old) == 1 and not old[0]["revoked"]
                 and old[0]["device_id_hash"] == _digest(device),
                 "login session was not stored with device binding")
        status, body, rotated = http.request("POST", "/api/auth/refresh")
        access = _access(status, body)
        _check_cookies(rotated)
        _require(rotated["refresh_token"].value != old_refresh
                 and rotated["device_session"].value == device,
                 "refresh cookie did not rotate with the same device")
        _me(http, access, credential)
        next_claims = _claims(rotated["refresh_token"].value)
        next_hash = _digest(next_claims["jti"])
        rows = _sessions(database, credential["user_id"], run_dir / "sessions-rotated.json")
        family = [row for row in rows if str(row["family_id"]) == old_claims["family_id"]]
        old = [row for row in family if row["jti_hash"] == old_hash]
        live = [row for row in family if not row["revoked"]]
        _require(next_claims["family_id"] == old_claims["family_id"]
                 and len(old) == 1 and old[0]["revoked"]
                 and old[0]["replaced_by_jti_hash"] == next_hash
                 and old[0]["rotation_grace_until"] is not None
                 and old[0]["last_used_at"] is not None
                 and len(live) == 1 and live[0]["jti_hash"] == next_hash
                 and live[0]["device_id_hash"] == _digest(device),
                 "database rotation observations differ")
        result["checks"]["cookie_and_database_rotation"] = True
        # Both releases give duplicate requests a five-second grace window.
        # Wait naturally; the observer never changes session state using SQL.
        time.sleep(6)
        status, body, _ = http.request("POST", "/api/auth/refresh",
            cookie=f"refresh_token={old_refresh}; device_session={device}")
        _require(status == 401 and body.get("detail", {}).get("code") == "REFRESH_TOKEN_REPLAYED",
                 "old refresh token replay was not refused")
        rows = _sessions(database, credential["user_id"], run_dir / "sessions-replayed.json")
        family = [row for row in rows if str(row["family_id"]) == old_claims["family_id"]]
        _require(bool(family) and all(row["revoked"] for row in family),
                 "replay did not revoke the observed family")
        status, body, _ = http.request("POST", "/api/auth/refresh", cookie=(
            f"refresh_token={rotated['refresh_token'].value}; device_session={device}"))
        _require(status == 401 and body.get("detail", {}).get("code") == "REFRESH_SESSION_REVOKED",
                 "revoked successor was not refused")
        result["checks"]["replay_and_family_revocation"] = True
        result["session"] = "PASS"
    except (ProbeError, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        result["failures"].append(str(error) if isinstance(error, ProbeError)
                                  else "observation could not complete")
    finally:
        try:
            after = _market_snapshot(database, run_dir / "market-after.json")
            result["market_before"] = before
            result["market_after"] = after
            unchanged = before is not None and before == after
            # This interval covers only this auth/session observer.  It does
            # not establish safety of a demo job run before this baseline.
            result["checks"]["auth_probe_market_unchanged"] = unchanged
            result["checks"]["market_tables_empty"] = all(
                item["count"] == 0 for item in after.values()
            )
        except (ProbeError, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
            result["failures"].append("final market observation could not complete")
        try:
            _require(service_before is not None and cursor is not None,
                     "initial service observation is absent")
            service_after = _service_snapshot(unit, run_dir / "service-after.json")
            errors = _journal_errors(unit, cursor, run_dir / "journal-delta.json")
            no_restarts = service_after["NRestarts"] == service_before["NRestarts"]
            same_process = (service_after["MainPID"] == service_before["MainPID"]
                            and service_after["InvocationID"] == service_before["InvocationID"])
            memory_within_limit = all(
                item["MemoryCurrent"] <= item["MemoryMax"]
                and item["MemoryPeak"] <= item["MemoryMax"]
                for item in (service_before, service_after)
            )
            result["service_before"] = service_before
            result["service_after"] = service_after
            result["journal_error_class_delta"] = errors
            result["http_statuses"] = http.statuses
            result["rate_limit_retries"] = http.retries
            result["new_5xx"] = sum(500 <= status < 600 for status in http.statuses)
            result["checks"].update(
                no_new_5xx=result["new_5xx"] == 0 and bool(http.statuses),
                no_new_journal_errors=all(count == 0 for count in errors.values()),
                no_service_restarts=no_restarts and same_process,
                memory_within_unit_limit=memory_within_limit,
            )
            required = ("auth_probe_market_unchanged", "no_new_5xx",
                        "no_new_journal_errors", "no_service_restarts", "memory_within_unit_limit",
                        "readiness_identity", "approved_login_and_bearer_profile",
                        "cookie_and_database_rotation", "replay_and_family_revocation",
                        "authenticated_personal_state_operations")
            if require_empty:
                required += ("market_tables_empty",)
            if all(result["checks"].get(name) is True for name in required):
                result["passive_harm"] = "PASS"
        except (ProbeError, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
            result["failures"].append("final service observation could not complete")
    public_file = Path(public_file)
    _require(not public_file.is_symlink() and not public_file.exists(),
             "public observation file already exists")
    public_file.write_text(json.dumps(result, sort_keys=True, indent=2) + "\n")
    return result
