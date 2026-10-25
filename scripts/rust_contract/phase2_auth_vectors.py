"""Test-only full-HTTP access-token oracle. Never uses live keys or databases.

The session double returns a synthetic current user and unread count. It rejects
all writes. PostgreSQL lock/concurrency proof belongs to Task 6, not this export.
Run --write on the isolated test host; --check detects stale Python evidence.
"""

import argparse
import copy
import hashlib
import json
import subprocess
from datetime import UTC, datetime
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from uuid import UUID

import jwt
from fastapi.testclient import TestClient

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "tests/rust_contract/phase2/auth-vectors.json"
NOW = 1_792_320_000
USER_ID = "a550c69b-2934-4b36-87f5-e4f9976d1af0"
# Python's exact legacy refusal body for an unverified credential.
LEGACY_CREDENTIAL_REFUSAL = '{"detail":"Could not validate credentials"}'
# Public synthetic fixture keys. They never authenticate a deployed service.
CURRENT = "phase2-fixture-current-key-not-for-runtime-0001"
PREVIOUS = "phase2-fixture-previous-key-not-for-runtime-0002"
WRONG = "phase2-fixture-unrelated-key-not-for-runtime-0003"
ISSUER = "verdaxis-test-api"
AUDIENCE = "verdaxis-test-web"
BASE = {"sub": USER_ID, "type": "access", "iss": ISSUER, "aud": AUDIENCE,
        "iat": NOW - 60, "iat_us": (NOW - 60) * 1_000_000, "exp": NOW + 60}


def specifications():
    cases = []

    def add(name, changes=None, *, omit=(), **options):
        claims = {**BASE, **(changes or {})}
        for field in omit:
            claims.pop(field, None)
        cases.append({"id": name, "claims": claims, **options})

    add("current-key")
    add("previous-key", signing_key="previous")
    add("previous-key-absent", signing_key="previous", previous_enabled=False)
    add("wrong-key", signing_key="wrong")
    add("altered-payload", alter_payload=True)
    add("algorithm-none", algorithm="none")
    add("algorithm-hs384", algorithm="HS384")
    for token in ("", "broken", "a.b.c", "a.b", "a.b.c.d"):
        add("malformed-token-" + (token or "empty"), raw_token=token)
    for name, header in (("missing", None), ("empty", ""), ("basic", "Basic abc"),
                         ("scheme-only", "Bearer"), ("leading-space", " Bearer {token}"),
                         ("double-space", "Bearer  {token}"), ("mixed-case", "bEaReR {token}")):
        add("bearer-" + name, authorization=header)
    add("duplicate-header-valid-first", repeated_authorization=["Bearer {token}", "Basic invalid"])
    add("duplicate-header-invalid-first", repeated_authorization=["Basic invalid", "Bearer {token}"])
    for field in ("iss", "aud"):
        add(field + "-absent", omit=(field,))
        for name, value in (("wrong", "other"), ("null", None), ("number", 1), ("boolean", True), ("empty", ""), ("object", {})):
            add(field + "-" + name, {field: value})
    for name, value in (("list", [AUDIENCE]), ("extra-list", ["other", AUDIENCE]),
                        ("wrong-list", ["other"]), ("mixed-list", [AUDIENCE, 1]), ("empty-list", [])):
        add("aud-" + name, {"aud": value})
    add("environment-mismatch", {"iss": "verdaxis-staging-api", "aud": "verdaxis-staging-web"})
    for field in ("exp", "nbf", "iat"):
        add(field + "-absent", omit=(field,))
        for name, value in (("before", NOW - 1), ("equal", NOW), ("after", NOW + 1),
                            ("numeric-string", str(NOW)), ("float", NOW + 0.9),
                            ("float-string", str(NOW) + ".5"), ("null", None),
                            ("boolean", True), ("array", []), ("object", {}),
                            ("invalid-string", "never"), ("negative", -1),
                            ("spaced-string", f"  +{NOW}  "), ("underscored-string", f"{NOW:_}"),
                            ("unicode-string", str(NOW).translate(str.maketrans("0123456789", "٠١٢٣٤٥٦٧٨٩")))):
            add(field + "-" + name, {field: value})
    for name, value in (("refresh", "refresh"), ("stream", "stream"), ("unknown", "other"), ("null", None), ("number", 1)):
        add("type-" + name, {"type": value})
    add("type-absent", omit=("type",))
    subject = UUID(USER_ID)
    for name, value in (("uppercase", USER_ID.upper()), ("hex", subject.hex),
                        ("braces", "{" + USER_ID + "}"), ("urn", subject.urn),
                        ("unusual-hyphens", subject.hex[:4] + "-" + subject.hex[4:]),
                        ("invalid", "unknown"), ("empty", ""), ("null", None),
                        ("number", 1), ("array", [])):
        add("sub-" + name, {"sub": value})
    add("sub-absent", omit=("sub",))
    for name, value in (("string", "test-jti"), ("empty", ""), ("null", None), ("number", 1), ("array", [])):
        add("jti-" + name, {"jti": value})
    add("jti-absent", omit=("jti",))
    for name, state in (("user-unknown", {"exists": False}), ("user-pending", {"status": "PENDING"}),
                         ("user-rejected", {"status": "REJECTED"}), ("password-change-required", {"must_change_password": True}),
                         ("unverified-user", {"email_verified": False}), ("no-organization", {"organization_id": None})):
        add(name, user=state)
    cutoff_us = (NOW - 60) * 1_000_000
    for field in ("password_changed_at", "authentication_revoked_at"):
        for name, offset in (("before", -1), ("equal", 0), ("after", 1)):
            add(field + "-" + name, {"iat_us": cutoff_us + offset}, user={field: cutoff_us})
        for name, value in (("boolean-true", True), ("boolean-false", False), ("negative", -1),
                            ("string", str(cutoff_us + 1)), ("float", cutoff_us + 1.0),
                            ("out-of-range", 10**30)):
            add(field + "-iat-us-" + name, {"iat_us": value}, user={field: cutoff_us})
        for name, value in (("integer", NOW - 59), ("float", NOW - 59.5),
                            ("string", str(NOW - 59)), ("boolean", True), ("out-of-range", -10**30)):
            add(field + "-fallback-" + name, {"iat": value}, omit=("iat_us",), user={field: cutoff_us})
        add(field + "-no-issuance", omit=("iat_us", "iat"), user={field: cutoff_us})
        for fraction in (0.000001, 0.123456, 0.999999):
            add(field + "-fractional-fallback-" + str(fraction), {"iat": NOW - 60 + fraction}, omit=("iat_us",), user={field: cutoff_us})
    add("both-cutoffs-password-first", user={"password_changed_at": cutoff_us, "authentication_revoked_at": cutoff_us})
    add("reapproved-old-token", user={"authentication_revoked_at": cutoff_us + 1})
    add("jwt-role-status-ignored", {"role": "ADMIN", "status": "APPROVED"}, user={"status": "REJECTED"})
    for name, cookie in (("expired-refresh", "refresh_token=expired"), ("foreign-device-revoked-refresh", "device_session=foreign; refresh_token=revoked"), ("foreign-refresh", "refresh_token=foreign")):
        add("cookies-" + name, cookie=cookie)
    add("cookie-only", authorization=None, cookie="refresh_token=not-a-bearer")
    return cases


def export():
    from app.config import settings
    from app.database import get_db
    from app.main import app
    from app.models.user import UserStatus
    from app.routers.auth_simple import _token_issued_at

    if settings.ENVIRONMENT != "test" or jwt.__version__ != "2.15.1":
        raise RuntimeError("oracle requires test environment and PyJWT 2.15.1")

    class Clock(datetime):
        @classmethod
        def now(cls, tz=None):
            return datetime.fromtimestamp(NOW, UTC).astimezone(tz) if tz else datetime.fromtimestamp(NOW)

    class Session:
        def __init__(self, user):
            self.user = user
            self.reads = 0

        async def execute(self, statement):
            self.reads += 1
            if self.reads == 1:
                parameters = statement.compile().params
                requested = next(iter(parameters.values()))
                found = self.user if self.user is not None and requested == self.user.id else None
                return SimpleNamespace(scalar_one_or_none=lambda: found)
            return SimpleNamespace(scalar=lambda: 0)

    vectors = []
    originals = {name: getattr(settings, name) for name in ("JWT_SECRET", "JWT_SECRET_PREVIOUS", "JWT_ISSUER", "JWT_AUDIENCE")}
    overrides = dict(app.dependency_overrides)
    try:
        settings.JWT_SECRET, settings.JWT_ISSUER, settings.JWT_AUDIENCE = CURRENT, ISSUER, AUDIENCE
        client = TestClient(app, raise_server_exceptions=False)
        with patch("jwt.api_jwt.datetime", Clock):
            for spec in specifications():
                settings.JWT_SECRET_PREVIOUS = PREVIOUS if spec.get("previous_enabled", True) else None
                state = {"exists": True, "status": "APPROVED", "must_change_password": False,
                         "password_changed_at": None, "authentication_revoked_at": None,
                         "email_verified": True, "organization_id": "synthetic-org", **spec.get("user", {})}
                user = SimpleNamespace(id=UUID(USER_ID), **{k: v for k, v in state.items() if k != "exists"})
                user.status = UserStatus(state["status"])
                for field in ("password_changed_at", "authentication_revoked_at"):
                    value = state[field]
                    setattr(user, field, datetime.fromtimestamp(value // 1_000_000, UTC).replace(microsecond=value % 1_000_000) if value is not None else None)
                session = Session(user if state["exists"] else None)

                async def db():
                    yield session

                app.dependency_overrides[get_db] = db
                algorithm = spec.get("algorithm", "HS256")
                key = {"current": CURRENT, "previous": PREVIOUS, "wrong": WRONG}[spec.get("signing_key", "current")]
                # api_jwt.encode rejects malformed registered claims before
                # signing. api_jws signs arbitrary test payload bytes so the
                # receiving decoder, not the fixture generator, decides.
                token = jwt.api_jws.encode(json.dumps(spec["claims"], separators=(",", ":")).encode(), "" if algorithm == "none" else key, algorithm=algorithm)
                if spec.get("alter_payload"):
                    head, _, signature = token.split(".")
                    import base64
                    payload = base64.urlsafe_b64encode(json.dumps({**spec["claims"], "sub": str(UUID(int=1))}).encode()).decode().rstrip("=")
                    token = f"{head}.{payload}.{signature}"
                token = spec.get("raw_token", token)
                authorization = spec.get("authorization", "Bearer {token}")
                headers = [("x-request-id", "phase2-auth-vector")]
                if authorization is not None:
                    headers += [("authorization", authorization.replace("{token}", token))]
                if "repeated_authorization" in spec:
                    headers = [("x-request-id", "phase2-auth-vector")] + [("authorization", h.replace("{token}", token)) for h in spec["repeated_authorization"]]
                if "cookie" in spec:
                    headers.append(("cookie", spec["cookie"]))
                before = copy.deepcopy(vars(user))
                response = client.get("/api/notifications/unread-count", headers=headers)
                if vars(user) != before:
                    raise AssertionError("oracle mutated synthetic account state")
                try:
                    issuance = _token_issued_at(spec["claims"])
                    issued_at_us = int(issuance.timestamp()) * 1_000_000 + issuance.microsecond if issuance else None
                except (ValueError, OverflowError, OSError):
                    issued_at_us = "out-of-range"
                vectors.append({
                    "id": spec["id"], "token": token, "authorization": [v for k, v in headers if k == "authorization"],
                    "previous_enabled": spec.get("previous_enabled", True), "user": state,
                    "issued_at_us": issued_at_us,
                    "python": {"status": response.status_code, "body": response.text,
                               "www_authenticate": response.headers.get("www-authenticate")},
                })
    finally:
        app.dependency_overrides.clear()
        app.dependency_overrides.update(overrides)
        for name, value in originals.items():
            setattr(settings, name, value)
    for vector in vectors:
        if vector["python"]["status"] == 500 and vector["id"].endswith("out-of-range"):
            password = vector["id"].startswith("password_changed_at")
            vector["approved_difference"] = {"decision": 94, "defect": "PD7", "reference_sha": "cf0cb692d75d7d6ce9a372dc80c7d09cdef6fdb7"}
            vector["rust_expected"] = {"status": 401, "www_authenticate": "Bearer", "body": json.dumps({"detail": {
                "code": "AUTH_PASSWORD_CHANGED" if password else "AUTH_SESSION_REVOKED",
                "message": "Password was changed; sign in again" if password else "Session was revoked; sign in again"}}, separators=(",", ":"))}
        elif vector["python"]["body"] == LEGACY_CREDENTIAL_REFUSAL:
            # AUTH-ENVELOPE+AUTH-REFUSAL+ACCESS-EXPIRY (auth contract): Python
            # keeps its legacy `detail` string and Rust answers with the
            # reviewed code envelope. Record both instead of letting the Rust
            # test substitute its own output for the expectation.
            vector["approved_difference"] = {"decisions": ["AUTH-ENVELOPE", "AUTH-REFUSAL", "ACCESS-EXPIRY"],
                                             "ruling": "AUTH-CONTRACT", "python_change": "none"}
            vector["rust_expected"] = {"status": 401, "www_authenticate": "Bearer",
                                       "body": json.dumps({"detail": {"code": "AUTH_CREDENTIALS_INVALID"}}, separators=(",", ":"))}
        elif vector["id"] == "exp-absent":
            # ACCESS-EXPIRY: Rust requires `exp` and refuses what Python accepts.
            vector["approved_difference"] = {"decision": "ACCESS-EXPIRY",
                                             "python_change": "none"}
            vector["rust_expected"] = {"status": 401, "www_authenticate": "Bearer",
                                       "body": json.dumps({"detail": {"code": "AUTH_CREDENTIALS_INVALID"}}, separators=(",", ":"))}
    return {"version": 1, "oracle_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
            "pyjwt_version": jwt.__version__, "now": NOW, "fixture_only": True,
            "settings": {"current": CURRENT, "previous": PREVIOUS, "issuer": ISSUER, "audience": AUDIENCE},
            "sources": {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in ("app/core/security.py", "app/routers/auth_simple.py")},
            "vectors": vectors}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    data = export()
    if args.write:
        OUT.write_text(json.dumps(data, indent=2) + "\n")
    elif args.check:
        pinned = json.loads(OUT.read_text())
        data["oracle_sha"] = pinned["oracle_sha"]
        if data != pinned:
            raise SystemExit("Python auth oracle changed; review and regenerate vectors")
    else:
        parser.error("choose --write or --check")
    print(f"AUTH_VECTOR_COUNT={len(data['vectors'])}")
    print(f"PYTHON_500_COUNT={sum(v['python']['status'] == 500 for v in data['vectors'])}")


if __name__ == "__main__":
    main()
