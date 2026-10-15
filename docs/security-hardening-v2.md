# Security hardening v2 rollout

This release is deliberately fail closed. Applying migrations or running the
preflight does not deploy or mutate admission/ownership decisions. Production
promotion still requires the product decisions listed below.

## JWT configuration, rotation, and refresh contract

Every environment must explicitly configure its own `JWT_SECRET`,
`JWT_ISSUER`, and `JWT_AUDIENCE`. There are no issuer/audience defaults, and
startup fails if any value is absent/blank, a key is shorter than 32
characters, issuer equals audience, or current/previous keys are equal. Never
reuse these values between development, staging, and production. Tokens are
accepted only with the configured signature, issuer, and audience.

Staging key rotation is supported now:

1. Generate a new staging-only key outside logs/shell history.
2. Set the new key as `JWT_SECRET` and old key as `JWT_SECRET_PREVIOUS`.
3. Restart every staging API/CLI process and verify newly issued token claims
   against the expected staging issuer/audience without printing token values.
4. Keep the overlap for the maximum refresh-token lifetime (currently seven
   days), not merely the 15-minute access-token lifetime.
5. Remove `JWT_SECRET_PREVIOUS` and restart every process.

Use the same controlled sequence for production after staging validation. An
emergency forced logout instead rotates `JWT_SECRET` without a previous key and
transactionally revokes all `refresh_sessions`; old access tokens then fail
signature validation immediately and old refresh families are unusable. If
only refresh rows are revoked, existing access tokens remain valid until their
15-minute expiry.

Refresh tokens require `jti` and `family_id`, a matching durable hashed session
row, and a separate opaque browser-device cookie. Only the device SHA-256
digest is stored; the raw identifier is HttpOnly and is never placed in JSON or
logs. Login, refresh, logout, and password-change issuance acquire the same
device-scoped PostgreSQL transaction advisory lock before user/session rows.
Login revokes every existing family for that device before issuing its new
family, and logout revokes every family for the device. Therefore a delayed
refresh response from account A can set only a token already revoked by a
later account-B login or logout, regardless browser response order.

The device column is nullable only for migration compatibility. Migration
revokes every existing NULL-device family because there is no trustworthy
browser identifier to backfill. A refresh with no valid device cookie fails
closed with `REFRESH_DEVICE_REQUIRED`, and runtime never binds a legacy family;
the user must sign in again. Login/logout may still one-way revoke a presented
signed legacy family while holding the new device lock. Rotation then locks the user/session, revokes the predecessor, and
persists its successor in one transaction. Persistence failure emits no token
or cookie. A duplicate within the five-second grace returns HTTP 409 without
revoking the usable successor:

```json
{"detail":{"code":"REFRESH_ROTATION_IN_PROGRESS","message":"...","retry_after_seconds":1}}
```

The terminal HTTP 401 code allowlist is:

- `REFRESH_TOKEN_MISSING`
- `REFRESH_TOKEN_EXPIRED`
- `REFRESH_TOKEN_INVALID`
- `REFRESH_SESSION_REVOKED`
- `REFRESH_TOKEN_REPLAYED`
- `REFRESH_ACCOUNT_INACTIVE`
- `REFRESH_PASSWORD_CHANGED`
- `REFRESH_DEVICE_REQUIRED`

Human guidance remains in `detail.message`. A post-grace replay revokes the
family and returns `REFRESH_TOKEN_REPLAYED`. A duplicate racing a logout or
password reset sees `REFRESH_SESSION_REVOKED`, not a misleading 409. Clients
preserve authentication on 409, 429, unknown, and 5xx responses and clear it
only on terminal 401. Pre-hardening refresh tokens lacking durable claims are
intentionally rejected with `REFRESH_TOKEN_INVALID`; users sign in again. The
JSON-body refresh transport is a temporary compatibility input; the current
contract uses the HttpOnly cookie.

Refresh and device cookies are HttpOnly, `SameSite=Lax`, and scoped to
`/api/auth`. `Secure` is disabled only when `ENVIRONMENT=development`;
staging, production, test, and unknown environments are secure. Email identity
is trimmed and lowercased at registration/login/recovery boundaries and
protected by a case-insensitive unique index. Register/change/reset passwords
are capped at 1024 UTF-8 bytes, and auth requests are capped at 128 KiB.

Credentialed CORS is a closed environment-derived set: production accepts
only `https://app.verdaxis.exchange` and `https://verdaxis.exchange`; staging
accepts only `https://staging.verdaxis.exchange`. Cookie-backed auth reads and
mutations require an exact allowed `Origin`; native bearer clients without a
cookie may omit it. Production and staging API units listen on loopback, and
forwarded client addresses are honored only from a loopback reverse proxy.

## Admission, membership, and KYC

These are independent audited transitions:

1. User admission: `PENDING` to `APPROVED`/`REJECTED`.
2. Organization verification: `PENDING` to `APPROVED`/`REJECTED`.
3. Organization membership: a reviewed `OrganizationJoinRequest` approval or
   rejection. User/org approval never changes it.
4. KYC review: submission sets `SUBMITTED`; only a trusted admin can approve or
   reject it.

Email links open the frontend verification page, which exchanges the one-time
token through `POST /api/auth/verify-email`. Verification is a mutation and is
never exposed as a state-changing `GET`. The token hash remains valid until its
original expiry so repeated POSTs are idempotent; this prevents corporate email
link scanners or a browser retry from replacing a successful verification with
an invalid-link result.

Admin membership review uses:

- `GET /api/auth/organization-joins?status=PENDING`
- `PUT /api/auth/organization-joins/{id}/approve`
- `PUT /api/auth/organization-joins/{id}/reject`

The read-only operator queue/detail are `GET /api/auth/admin/review-queue` and
`GET /api/auth/admin/review-queue/{user_id}`. They expose bounded email,
admission, current/requested organization, KYC organization, reviewer/time,
and external evidence metadata only to ADMIN. Join-request history is capped
per candidate with a partitioned row number, so one heavy history cannot
starve another candidate. These endpoints never approve a case.

Registration stores the password hash/PII in expiring server-side pending
state and returns one opaque, one-time 30-minute token. A newer registration
invalidates the prior pending token for the same normalized email. Domain
matching only proposes a pending join; it grants no organization membership or
tenant data access. `disposable-email-domains==0.0.225` and
`free-email-domains==1.0.2` are pinned deterministic release datasets.
Public/disposable domains, including `mailinator.com` and
`guerrillamail.com`, never become shared tenant boundaries. If either dataset
cannot load, all domain attachment is disabled fail safely.

KYC documents are validated for bounded size/type and organization fields, then
retained with application-level encryption. No KYC document is sent to Gemini
or any other external provider. Submission audit records use
`{"provider": "none", "status": "disabled"}`. A DeepSeek V4.1 Flash advisory
check on a non-PRC host is planned, not built. Human review remains authoritative.
Approve/reject request contracts are unchanged. Legacy discarded documents cannot
be recovered. Review audit flags report whether retained documents exist for the
reviewed user and organization.

### Encrypted document vault: final design (15 October 2026)

`kycd_20261015_staging` follows `kycg_20261014_staging`. The checkpoint policy
allows that exact transition and a same-revision restart. Two `kyc_documents`
rows share a submission UUID and commit with status and audit. Resubmission keeps
history. No plaintext filename or hash is stored. Each document uses a random
32-byte AES-256-GCM data key and separate random 12-byte data/wrap nonces. Versioned
associated data binds document, submission, user, organization, kind and MIME;
wrapping binds document and key ID. The runtime has SELECT/INSERT only; PUBLIC
has no access. User/organization foreign keys use RESTRICT. Downgrade refuses
nonempty evidence storage. No document-deletion command is provided pending counsel.

ADMIN metadata reads are bounded to 50 newest records. Downloads authenticate,
decrypt, and commit `kyc.document_viewed` before returning bytes. They use
attachment disposition, no-store and nosniff headers, and a 30/minute limit.
Decryption errors return `KYC_DOCUMENT_DECRYPTION_FAILED` and log only the document
ID. Missing/invalid keys disable submission with `KYC_DOCUMENT_STORAGE_UNAVAILABLE`
(503), with one sanitized startup error per worker; readiness is unchanged.

The initial blocker was that an absolute-path LoadCredential source alone makes
a missing file fatal before Python starts. The approved solution is an empty,
invalid fallback: `SetCredential=kyc-document-keys:` plus the requested
`LoadCredential=kyc-document-keys:/etc/verdaxis/kyc-document-keys[-staging].json`.
Both host versions support it:
- [v249 execute.c:2545-2559](https://github.com/systemd/systemd/blob/v249/src/core/execute.c#L2545-L2559)
  writes literal credentials first; [2598-2610](https://github.com/systemd/systemd/blob/v249/src/core/execute.c#L2598-L2610)
  accepts ENOENT when the credential already exists. The loaded file overrides it.
- [v255 exec-credential.c:479-491](https://github.com/systemd/systemd/blob/v255/src/core/exec-credential.c#L479-L491)
  accepts ENOENT when SetCredential is configured; [684-716](https://github.com/systemd/systemd/blob/v255/src/core/exec-credential.c#L684-L716)
  writes defaults only when no loaded credential exists.

An empty credential fails strict application JSON validation. Other systemd
errors, such as unreadable sources, remain service-start failures. This is source
verification for both tags, not live-host execution. The immutable manifest still
lists the same units; the installer derives digests from committed unit bytes,
not a separate checked-in digest list. The installer now checks key-source owner,
mode and parent-directory permissions before dry-run acceptance or replacement;
missing sources remain allowed. Code-deploy logic is unchanged. See the runbook.

Deletion review: monitor cleanup deletes users and empty organizations; both now
check retention before deletion. SQLAdmin User/Organization delete overrides lock
the target and refuse with `KYC_DOCUMENTS_RETAINED`; SQLAdmin displays the conflict
in its error banner rather than returning an API 409. No document ModelView is
registered. RESTRICT remains the database backstop. No account-delete route exists
in auth_simple. Seed/demo/market-quarantine cleanup deletes market records, not
users or organizations. Registration/auth maintenance deletes pending records and
sessions, not accounts. Test-only deletion paths are organization resets in
`test_orderbook_marketplace_filters.py`, explicit user/org cleanup in
`test_runtime_role_policy.py`, and disposable PostgreSQL TRUNCATE CASCADE fixtures
and `test_market_integrity_concurrency.py`. Those fixtures retain their existing
cleanup semantics. See the operator runbook for keys and rotation.

## Two-step migrations and preflight

The branch has one linear head and four security revisions:

```text
pa_20260715_analytics_facts
  -> rh_20260720_runtime_metadata
  -> sec_20260720_identity
  -> sec_20260720_boundaries
  -> sec_20260720_fresh
  -> sec_20260720_device
  -> miq_20260720_market_quarantine
  -> mi_20260720_market_integrity
  -> sse_20260720_market_event_stream (head)
```

Deployed environments apply these steps as allowlisted checkpoint
transitions — see docs/runbooks/integrated-migration-cutover.md. For a
database containing legacy plaintext email-verification tokens:

1. Apply `alembic upgrade sec_20260720_identity`. It hashes token identifiers
   in bounded batches and sets every migrated token expiry to that migration
   time plus 24 hours. It also creates durable refresh sessions/indexes.
2. Keep the compatibility revision for the full 24 hours, or deliberately
   expire outstanding links and notify affected users.
3. Apply `alembic upgrade sec_20260720_boundaries`. It refuses to drop the
   plaintext column while any compatibility token is active.
4. Before starting enforcement-capable workers, run the candidate release's
   read-only report:

   ```bash
   ./venv/bin/python -m scripts.security_preflight
   ```

The report emits no token/password values or user emails. It enumerates
ineligible approved-user UUID/state (including approved users with no current
organization), case-insensitive email conflicts, pending joins, advisory KYC
evidence gaps, total legacy provenance gaps, and outstanding executable legacy
rows. Pending and rejected registrations remain non-executable and are not
release blockers. Ownerless executable rows are exempt only when every party
belongs to the same exact deterministic DEMO or TEST registry; mixed or
unknown provenance remains blocked. Ineligible approved users are blockers. Exit 0 means
`enforcement_preflight=READY`; exit 2 means `BLOCKED` and production promotion
must stop.

The boundary migration lowercases email only after aborting on
case-insensitive duplicates. It normalizes only semantically equivalent legacy
organization `VERIFIED` to canonical `APPROVED`; it never promotes `PENDING`
users, organizations, joins, or KYC. Ownership columns remain nullable for
schema compatibility and no owner is inferred. Legacy market provenance is
resolved only through exact operator decisions: quarantine/cancellation of
outstanding state and the migrator-owned organization approval ledger. The
approval CLI freezes organization identity plus the exact qualifying member
set; `mi` fails on drift and promotes only ledgered organizations to REAL.
Re-run the preflight after those decisions.

When integrating another Alembic branch, linearize/rebase these revisions onto
the selected predecessor and re-run fresh/realistic upgrades. Do not add an
empty merge-head revision. Preserve advisory-only KYC and the two-step
plaintext-token compatibility window.

CI and `scripts/run_product_analytics_postgres_tests.sh` use the pinned
`postgis/postgis:17-3.5` image. Alembic fixes reflection to `public`, excludes
all non-public extension schemas, and discovers extension-owned/configured
public relations from PostgreSQL catalogs rather than a Tiger/PostGIS table
denylist. The disposable suite requires clean-head `alembic check` exit 0 and
also creates a temporary public app column to prove real drift still fails.

## Execution and private streams

The central execution gate requires a concrete BUYER/SUPPLIER user whose
`organization_id` exactly equals a non-null approved organization, approved
user/email state, and no required password change. Pending/legacy KYC is
advisory; a bound KYC organization must match current membership, and explicit
rejection revokes eligibility. ADMIN has no acting-mode contract.

Order matching, direct trades, RFQ acceptance, negotiations, trade lifecycle,
and inventory publication lock and revalidate every concrete user and exact
organization in the transaction, including resting order/quote owners.
Created orders/RFQs/quotes/negotiations/trades retain user provenance.
Negotiation acceptance locks both canonical orders with NOWAIT, revalidates
party/provenance/expiry/status, and atomically consumes remaining capacity.
Security invalidation acquires canonical sorted slice/order/inventory locks,
revalidates active rows, invokes the same cancellation lifecycle, releases only
unfilled linked inventory reservations, and writes audit before the caller
commits; SSE publication occurs only after commit. It never bulk-cancels.
Expected `NOWAIT` SQLSTATE `55P03` contention at cancellation and every
admin/KYC/membership invalidation route rolls back and returns the same
retryable HTTP 409 contract. This includes both active user-rejection routes
(`/auth/reject/{user_id}` and
`/admin/analytics/users/{user_id}/reject`); neither admin surface may bypass
the canonical cancellation lifecycle. The body is:

```json
{"detail":{"code":"EXECUTION_INVALIDATION_BUSY","message":"Execution state is being updated; retry this request","retry_after_seconds":1}}
```

The response includes `Retry-After: 1`; unexpected database errors are not
misclassified.
Authenticated concrete owners retain cancel/decline operations after execution
eligibility is revoked. Ownerless legacy rows stay
non-executable until the explicit product decision above.

Anonymous RFQs omit stable buyer organization IDs/names from non-owner
responses. Private `/api/stream/activity` and `/api/stream/trades` accept only
the explicit `stream_token` query parameter, reject simultaneous Authorization
headers, require exact issuer/audience/environment and token-bound `org_id`,
subscribe only after token/user/org validation, terminate at the
60-second token expiry, and revalidate mutable state plus the exact originally
subscribed organization every 15 seconds using short-lived sessions. All
subscription paths unsubscribe on error/cancellation. Production and staging
systemd units disable Uvicorn access logs so query tokens are not logged;
application logs contain only fixed event names and path-safe metadata.

## Removed and scheduled surfaces

The unauthenticated dashboard log/host-metric routes, manual news refresh API,
role-switch route, and nonfunctional legacy compliance upload/ledger router are
not registered. `/health/ready` remains a sanitized readiness probe. The
active authenticated compliance scoring API remains.

News refresh and auth cleanup are independent systemd oneshot/timer contracts;
neither runs in web workers. See `docs/news-refresh-timer.md` and
`docs/auth-maintenance-timer.md`. News uses provider-native deadlines, bounded
per-process provider capacity retained until the underlying call returns, a
thread-safe process-local three-failure/60-second breaker, and a PostgreSQL
transaction advisory lock preventing timer/CLI overlap. RSS streams each body
with a 2 MiB decoded cap, 64 KiB chunks, 15-second timeout, four-fetch
concurrency, 100 entries/feed, 250 entries/run, and 50 provider calls/run.
Database URL checks/inserts inherit the 250-row run cap. Email operational logs
contain only a recipient hash, HTTP status, or exception class—never raw
recipient addresses, provider bodies, or exception messages.

## Production promotion decisions remaining

- Choose quarantine versus audited user mapping for every outstanding legacy
  order/RFQ/quote/trade/negotiation reported by preflight.
- Review/record external KYC evidence for previously approved KYC users; do not
  backfill evidence automatically.
- Product owner must explicitly approve KYC execution-enforcement rollout;
  leave pending/unknown KYC advisory until then.
- Approve the external KYC evidence system and legal retention policy. This
  release retains encrypted documents and review references/notes.
- Schedule the production JWT overlap or explicitly approve a forced logout.
