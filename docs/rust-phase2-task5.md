# Phase 2 Task 5 — access-token verification

## Scope

Task 5 only. Tasks 3, 4 and 6 remain unstarted. No deployed routes, credentials,
user records, sessions, migrations, grants or runtime settings changed.

Oracle source: `64179cd15c85e1da76ec49d8756b6f7419e94c66`. The fixture also pins
SHA-256 digests of `app/core/security.py` and `app/routers/auth_simple.py`.
PyJWT 2.15.1 was inspected on the isolated test host. Its required-claim list
is empty; issuer/audience are required by configured validation, not by a new
Rust policy. jsonwebtoken is locked at 10.4.0 with signature verification on and
HS256 only. Python-compatible standard-claim validation uses the supplied clock,
not jsonwebtoken's default required-exp or 60-second leeway.

## Implementation

- `rs/read/src/auth/token.rs`: verification-only key settings with redacted Debug,
  bearer extraction, current/previous signature verification, explicit registered
  claim validation, UUID compatibility and issuance-time projection.
- `rs/read/src/auth/claims.rs`: typed verification result, source-equivalent error
  envelopes and a read-only current-account-state validator. Password cutoff is
  checked before authentication revocation, then status and password-change state.
- `scripts/rust_contract/phase2_auth_vectors.py`: full FastAPI HTTP oracle using
  synthetic accounts and a session double that exposes reads only. No lifespan,
  database connection or external HTTP call is used. Only test environment and
  PyJWT 2.15.1 are accepted. Public synthetic keys cannot be used by a live service.
- `rs/read/tests/auth_tokens.rs`: verifies all vectors directly and through an
  in-process axum HTTP test adapter. It compares status, complete JSON error body
  (or exact plain 500 body), authentication header, subject and usable issuance.
- `tests/rust_contract/test_phase2_auth_vectors.py`: unique IDs, coverage, source
  digests, fixture-key identity and absence of issuing/database interfaces.

Vector digest: `c59652c3871a62b6b4b63ec30b149465f328efa32ed6c573a6bc8200fb5e248c`.
There are 161 distinct vectors. Both stacks match every vector. No case accepts
in Rust when Python refuses. The table below contains results from the paired
check, not live or database-lock acceptance evidence.

## Known source findings and release limits

Four signed-token cases cause Python HTTP 500: out-of-range integer `iat_us` and
out-of-range fallback `iat`, each under password and revocation cutoffs. Rust
retains an explicit `IssuanceOutOfRange` result. This is not a 401 policy repair.
Affected cutover is blocked pending an approved Python disposition and refreshed
oracle. These inputs never gain access in Rust.

The account validator consumes current account state, not JWT role/status claims.
Task 6 must load it under the current-user row lock and keep that lock with the
route transaction. This task proves immutable synthetic state, not committed
PostgreSQL rows, lock ordering or concurrent revocation. No SQLx pool is exposed
by this module. No real refresh-family database state is created; cookie vectors
prove ordinary bearer verification ignores refresh/device cookie text. Database
refresh-family interoperability remains for Task 6.

The plan's `run_disposable --phase2 ... --phase2-scenario auth-token-matrix`
command was not run: those options belong to Task 4, which is explicitly blocked.
Its paired token-only equivalent here is the fresh Python HTTP oracle export/check
plus Rust HTTP vector test. No second process/database lifecycle was created.

## Gates and exits

All host calls used `verdaxis-prod-test "$PWD"` and
`/home/verdaxis-test/bin/heavy run 3600 -- bash -lc ...`. No project build or test
ran on the Mac. No call was interrupted. All calls finished.

| Gate | Exit | Evidence |
| --- | ---: | --- |
| PyJWT version/default validation inspection | 0 | `work/rust-phase2/task5-pyjwt-inspect.log` |
| Initial Python oracle export | 1 | Encoder rejected malformed issuer before signing; fixed generator to sign raw synthetic payload bytes with api_jws |
| Initial Rust RED attempt | 101 | Missing auth module; initial fixture export was incomplete, so not counted as valid RED evidence |
| Corrected Python oracle export | 0 | 155 full-HTTP vectors, four Python 500s |
| Fixture-backed Rust RED | 101 | Missing `verdaxis_read::auth`; `task5-red-valid-fixtures.log` |
| Dependency resolution | 0 | jsonwebtoken 10.4.0 pinned |
| First Rust GREEN attempt | 101 | Bearer extra-space acceptance differed; fixed to match Python |
| Second Rust GREEN attempt | 101 | Large integer issuance was parsed as float; enabled exact arbitrary-precision JSON parsing |
| Third Rust GREEN | 0 | All 155 vectors and redaction test pass |
| First lockfile-narrowing attempt | 101 | Wrong package name in command; no source policy change |
| Paired Rust HTTP vector gate | 0 | 155 vectors, complete status/body/header comparison |
| Corrected lockfile narrowing | 0 | Restored unrelated combine/libc/schannel/unicode-normalization versions |
| Expanded fractional issuance oracle | 0 | 161 vectors |
| Expanded fractional Rust gate | 0 | Exact microsecond fallback comparisons pass |
| Fresh Python oracle check | 0 | `task5-final.log` |
| Initial fixture/regression meta-suite | 1 | 52 passed, one duplicate vector ID found |
| Rust workspace tests | 0 | `task5-final.log`; database/contract-feature suites are not claimed by this command |
| Rust Clippy, workspace/all targets, warnings denied | 0 | `task5-final.log` |
| Rust format write/check | 0 / 0 | `task5-final.log`; formatted auth files copied back unchanged |
| Corrected final Python oracle export | 0 | 161 unique vectors; `task5-final-fixed.log` |
| Corrected final Python regression suite | 0 | 53 passed; 11 existing Pydantic warnings |
| Corrected final Rust HTTP/auth test | 0 | Two tests; all 161 vector cells pass |
| Git whitespace check | 0 | Local read-only Git check |
| Planned Task 4 runner scenario | Not run | Options absent; Task 4 is not authorized |

CI on draft PR #88 at the prior Tasks 1–2 tip passed test, postgres-phase0,
postgres-analytics, offline and contract; publish was skipped. These are not CI
results for Task 5. No Task 5 push was made; the brief also says the PM pushes.

## Resume point

Stop after Task 5. PM reviews these bytes and source findings before pushing.
Do not start Task 3 until Phase 1 gate fixes merge. Do not start Task 4 before
Task 3. Task 6 owns database admission and lock integration. Before accepting a
route port, rerun the paired scenario in the extended disposable harness and
regenerate vectors after any approved Python authentication change.

## Complete vector table

HTTP 200 means accepted; 401/403 means refused; 500 is the explicit source finding
above. Rust result cells are checked against Python by the passing HTTP test.

| Case | Python HTTP | Rust HTTP |
| --- | ---: | ---: |
| current-key | 200 | 200 |
| previous-key | 200 | 200 |
| previous-key-absent | 401 | 401 |
| wrong-key | 401 | 401 |
| altered-payload | 401 | 401 |
| algorithm-none | 401 | 401 |
| algorithm-hs384 | 401 | 401 |
| malformed-token-empty | 401 | 401 |
| malformed-token-broken | 401 | 401 |
| malformed-token-a.b.c | 401 | 401 |
| malformed-token-a.b | 401 | 401 |
| malformed-token-a.b.c.d | 401 | 401 |
| bearer-missing | 401 | 401 |
| bearer-empty | 401 | 401 |
| bearer-basic | 401 | 401 |
| bearer-scheme-only | 401 | 401 |
| bearer-leading-space | 401 | 401 |
| bearer-double-space | 200 | 200 |
| bearer-mixed-case | 200 | 200 |
| duplicate-header-valid-first | 200 | 200 |
| duplicate-header-invalid-first | 401 | 401 |
| iss-absent | 401 | 401 |
| iss-wrong | 401 | 401 |
| iss-null | 401 | 401 |
| iss-number | 401 | 401 |
| iss-boolean | 401 | 401 |
| iss-empty | 401 | 401 |
| iss-object | 401 | 401 |
| aud-absent | 401 | 401 |
| aud-wrong | 401 | 401 |
| aud-null | 401 | 401 |
| aud-number | 401 | 401 |
| aud-boolean | 401 | 401 |
| aud-empty | 401 | 401 |
| aud-object | 401 | 401 |
| aud-list | 200 | 200 |
| aud-extra-list | 200 | 200 |
| aud-wrong-list | 401 | 401 |
| aud-mixed-list | 401 | 401 |
| aud-empty-list | 401 | 401 |
| environment-mismatch | 401 | 401 |
| exp-absent | 200 | 200 |
| exp-before | 401 | 401 |
| exp-equal | 401 | 401 |
| exp-after | 200 | 200 |
| exp-numeric-string | 401 | 401 |
| exp-float | 401 | 401 |
| exp-float-string | 401 | 401 |
| exp-null | 401 | 401 |
| exp-boolean | 401 | 401 |
| exp-array | 401 | 401 |
| exp-object | 401 | 401 |
| exp-invalid-string | 401 | 401 |
| exp-negative | 401 | 401 |
| exp-spaced-string | 401 | 401 |
| exp-underscored-string | 401 | 401 |
| exp-unicode-string | 401 | 401 |
| nbf-absent | 200 | 200 |
| nbf-before | 200 | 200 |
| nbf-equal | 200 | 200 |
| nbf-after | 401 | 401 |
| nbf-numeric-string | 200 | 200 |
| nbf-float | 200 | 200 |
| nbf-float-string | 401 | 401 |
| nbf-null | 401 | 401 |
| nbf-boolean | 200 | 200 |
| nbf-array | 401 | 401 |
| nbf-object | 401 | 401 |
| nbf-invalid-string | 401 | 401 |
| nbf-negative | 200 | 200 |
| nbf-spaced-string | 200 | 200 |
| nbf-underscored-string | 200 | 200 |
| nbf-unicode-string | 200 | 200 |
| iat-absent | 200 | 200 |
| iat-before | 200 | 200 |
| iat-equal | 200 | 200 |
| iat-after | 401 | 401 |
| iat-numeric-string | 200 | 200 |
| iat-float | 200 | 200 |
| iat-float-string | 401 | 401 |
| iat-null | 401 | 401 |
| iat-boolean | 200 | 200 |
| iat-array | 401 | 401 |
| iat-object | 401 | 401 |
| iat-invalid-string | 401 | 401 |
| iat-negative | 200 | 200 |
| iat-spaced-string | 200 | 200 |
| iat-underscored-string | 200 | 200 |
| iat-unicode-string | 200 | 200 |
| type-refresh | 401 | 401 |
| type-stream | 401 | 401 |
| type-unknown | 401 | 401 |
| type-null | 401 | 401 |
| type-number | 401 | 401 |
| type-absent | 401 | 401 |
| sub-uppercase | 200 | 200 |
| sub-hex | 200 | 200 |
| sub-braces | 200 | 200 |
| sub-urn | 200 | 200 |
| sub-unusual-hyphens | 200 | 200 |
| sub-invalid | 401 | 401 |
| sub-empty | 401 | 401 |
| sub-null | 401 | 401 |
| sub-number | 401 | 401 |
| sub-array | 401 | 401 |
| sub-absent | 401 | 401 |
| jti-string | 200 | 200 |
| jti-empty | 200 | 200 |
| jti-null | 401 | 401 |
| jti-number | 401 | 401 |
| jti-array | 401 | 401 |
| jti-absent | 200 | 200 |
| user-unknown | 401 | 401 |
| user-pending | 403 | 403 |
| user-rejected | 403 | 403 |
| password-change-required | 403 | 403 |
| unverified-user | 200 | 200 |
| no-organization | 200 | 200 |
| password_changed_at-before | 401 | 401 |
| password_changed_at-equal | 401 | 401 |
| password_changed_at-after | 200 | 200 |
| password_changed_at-iat-us-boolean-true | 401 | 401 |
| password_changed_at-iat-us-boolean-false | 401 | 401 |
| password_changed_at-iat-us-negative | 401 | 401 |
| password_changed_at-iat-us-string | 401 | 401 |
| password_changed_at-iat-us-float | 401 | 401 |
| password_changed_at-iat-us-out-of-range | 500 | 500 |
| password_changed_at-fallback-integer | 200 | 200 |
| password_changed_at-fallback-float | 200 | 200 |
| password_changed_at-fallback-string | 401 | 401 |
| password_changed_at-fallback-boolean | 401 | 401 |
| password_changed_at-fallback-out-of-range | 500 | 500 |
| password_changed_at-no-issuance | 401 | 401 |
| password_changed_at-fractional-fallback-1e-06 | 200 | 200 |
| password_changed_at-fractional-fallback-0.123456 | 200 | 200 |
| password_changed_at-fractional-fallback-0.999999 | 200 | 200 |
| authentication_revoked_at-before | 401 | 401 |
| authentication_revoked_at-equal | 401 | 401 |
| authentication_revoked_at-after | 200 | 200 |
| authentication_revoked_at-iat-us-boolean-true | 401 | 401 |
| authentication_revoked_at-iat-us-boolean-false | 401 | 401 |
| authentication_revoked_at-iat-us-negative | 401 | 401 |
| authentication_revoked_at-iat-us-string | 401 | 401 |
| authentication_revoked_at-iat-us-float | 401 | 401 |
| authentication_revoked_at-iat-us-out-of-range | 500 | 500 |
| authentication_revoked_at-fallback-integer | 200 | 200 |
| authentication_revoked_at-fallback-float | 200 | 200 |
| authentication_revoked_at-fallback-string | 401 | 401 |
| authentication_revoked_at-fallback-boolean | 401 | 401 |
| authentication_revoked_at-fallback-out-of-range | 500 | 500 |
| authentication_revoked_at-no-issuance | 401 | 401 |
| authentication_revoked_at-fractional-fallback-1e-06 | 200 | 200 |
| authentication_revoked_at-fractional-fallback-0.123456 | 200 | 200 |
| authentication_revoked_at-fractional-fallback-0.999999 | 200 | 200 |
| both-cutoffs-password-first | 401 | 401 |
| reapproved-old-token | 401 | 401 |
| jwt-role-status-ignored | 403 | 403 |
| cookies-expired-refresh | 200 | 200 |
| cookies-foreign-device-revoked-refresh | 200 | 200 |
| cookies-foreign-refresh | 200 | 200 |
| cookie-only | 401 | 401 |
