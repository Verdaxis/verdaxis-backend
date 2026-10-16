# Security-v2 operator review

This is a read-only preflight and review workflow. It does not deploy, change
services, mutate admission state, or approve KYC.

From the repository root, verify the command and then run the database report:

```bash
./venv/bin/python -m scripts.security_preflight --check-imports
./venv/bin/python -m scripts.security_preflight
```

An exit code of `2` means a provenance or identity blocker remains. This
includes an approved trader whose email or current approved organization is
missing. Pending and rejected registrations are reported through their normal
review queues but do not block a release because they cannot execute market
actions. KYC evidence counts remain advisory while the product-owner rollout
hold is in force.

The report shows total ownerless legacy rows for operator visibility. Only
executable rows outside the exact deterministic DEMO and TEST organization
registries are blockers. Synthetic exemptions require both parties to belong
to the same registry; mixed, unknown, or partially synthetic pairs still fail
closed. The market-integrity migration snapshots those deterministic
organization provenances, and the runtime continues to prevent synthetic and
real liquidity from matching.

Review pending cases through the admin-only read APIs:

- `GET /api/auth/admin/review-queue?limit=50`
- `GET /api/auth/admin/review-queue/{user_id}`

Compare email verification, account state, current and requested organization,
organization verification, KYC organization, reviewer/time, and external
evidence metadata. Each candidate receives an independent bounded join-history
slice; a heavy candidate cannot consume another candidate's cap. KYC documents are retained encrypted. Use `GET /api/kyc/admin/{user_id}/documents`
and `GET /api/kyc/admin/documents/{document_id}` for audited review. No document
is sent to an external provider. A DeepSeek V4.1 Flash advisory check on a
non-PRC host is planned, not built.

If an admission/KYC/membership decision returns
`409 EXECUTION_INVALIDATION_BUSY` with `Retry-After: 1`, no review transition
was committed. Wait at least the bounded retry interval, reload the case, and
retry the single decision. Do not bypass the endpoint with direct bulk SQL.

Do not auto-approve. Hold all user admission, organization admission,
membership, and KYC decisions for an authenticated operator using the existing
single-case decision endpoints. Hold production KYC execution enforcement until
the product owner explicitly approves rollout.


## KYC execution gate switch (Private Beta)

`KYC_EXECUTION_GATE_ENABLED` (in `app/services/kyc_execution.py`, the one
decision point) turns the KYC execution rule off without removing any KYC code
or data. Its code default is `true` (fail safe): a missing setting still
enforces KYC.

- **Prod and staging run with it `false` through Private Beta.** Set
  `KYC_EXECUTION_GATE_ENABLED=false` in each environment's `.env` and restart.
  Confirm the startup log line `KYC execution gate: disabled` (one line per
  Uvicorn worker) after each deploy.
- **With the gate off**, only a `REJECTED` review or a review bound to another
  organization revokes access. Every other status is admitted and reports
  `KYC_NOT_ENFORCED` with no grace deadline; resting orders of non-KYC users
  show and match again. KYC submission and admin approve/reject keep working.
- `python -m app.cli.send_kyc_grace_notices` refuses with a non-zero exit while
  the gate is off, so a notice is never sent that no user needs.

**Launch note.** When the gate is turned back on, grace has expired (28 Oct
2026), so every user without approved KYC is blocked at once: `/api/auth/me`
reports `KYC_REQUIRED` and new executions return 403 `KYC_REQUIRED`. Plan KYC
approvals — or write a new reviewed grace snapshot — before you set the switch
to `true`.


## KYC key operations (separate operator approval required)

Run only on the approved host as root. Use `/etc/verdaxis/kyc-document-keys-staging.json`
for staging and `/etc/verdaxis/kyc-document-keys.json` for production. Never put key
material in git, `.env`, a database, terminal output, logs or a support message.

**Inspect first, every time.** Before creating or replacing anything, look at the
current state of the exact key path:

```bash
stat -c '%U:%G %a %s %n' /etc/verdaxis/kyc-document-keys.json
```

Expected `root:root 400`. An existing **empty** file (size 0) is the approved
**placeholder meaning "no key ring"**, not a ring: the application treats the
empty credential as absent (KYC storage disabled, readiness unaffected), and the
backend unit starts normally. The founder's real key ring replaces it. A
**non-empty** file is a live ring: never overwrite it; use the rotation procedure.

For a first key file, use a new path and do not overwrite an existing key ring:

```bash
umask 077
set -o noclobber
python3 -c 'import base64,json,secrets; print(json.dumps({"active":"20261015","keys":{"20261015":base64.b64encode(secrets.token_bytes(32)).decode()}}))' > /etc/verdaxis/kyc-document-keys-staging.json
chown root:root /etc/verdaxis/kyc-document-keys-staging.json
chmod 0400 /etc/verdaxis/kyc-document-keys-staging.json
```

The directory must already exist and be root-owned. The founder must keep a
verified offline encrypted copy of the complete key ring, with recovery access
stored separately. Back up the database and key ring independently. If every
copy of a master key is lost, the documents encrypted with it cannot be recovered.

For rotation, preserve every old key ID and value. Generate a new random 32-byte
key in a root-only process, add it under a new ID, set `active` to that ID, and
write a new root:root 0400 file. Do not print its contents. Back up and verify the
new complete ring offline, then atomically replace the host file and restart via
the approved deployment procedure. Workers load keys once at startup; systemd
credentials are snapshots. Old documents keep their original key IDs; new
submissions use the new active key. Never remove an old key while its documents
or backups remain. There is no operator document-delete command yet.

### Empty placeholder when no key ring exists yet

The backend unit's `LoadCredential=kyc-document-keys:/etc/verdaxis/...` makes an
**absent** file a fatal start failure (243/CREDENTIALS) on both deployed systemd
versions (249 staging, 255.4 production); the historical empty `SetCredential=`
fallback is invalid syntax on systemd 255 and is removed from the units. A host
without a real key ring therefore publishes the approved **empty placeholder**:

```bash
umask 077
printf '' > /etc/verdaxis/kyc-document-keys.json
chown root:root /etc/verdaxis/kyc-document-keys.json
chmod 0400 /etc/verdaxis/kyc-document-keys.json
stat -c '%U:%G %a %s %n' /etc/verdaxis/kyc-document-keys.json
```

Expected `root:root 400 0 ...`. `LoadCredential` then succeeds, the application
treats the empty credential as missing (storage disabled, 503 on submission,
readiness unaffected). This placeholder is the state the Phase 0 production
cutover (17 October 2026) started with; the founder's real ring replaces it
through the B11 replacement below. Never treat the placeholder as a ring, and
never widen its mode or ownership.

### B11 — replacing the placeholder with the founder's real ring (post-cutover)

With the founder present, after public access has reopened:

1. Inspect first: `stat -c '%U:%G %a %s %n' /etc/verdaxis/kyc-document-keys.json`.
   Size must be 0. If it is not, a ring already exists — use rotation, not this
   procedure.
2. The founder creates the new complete ring offline, keeps the verified
   encrypted offline copy and the separate recovery secret, and proves restore
   before the host file changes. Follow the first-key-file block above, writing
   to a new temporary root:root 0400 path.
3. Atomically replace the placeholder: `mv` the new ring over
   `/etc/verdaxis/kyc-document-keys.json`, keeping root:root 0400. Never print
   its contents.
4. Run the unit-installer preflight
   (`scripts/install_systemd_units.sh --dry-run --environment production
   --source-ref <approved-40-hex-sha>`); it must accept the new real file's
   metadata (root:root 0400 regular file).
5. Restart the backend through the separately approved deployment procedure so
   workers load the ring; then verify storage works and readiness still reports
   `status=ok`. No raw keys or documents in chat, CI or reports.

Install units only through the exact-SHA manifest installer in runtime-hardening.md.
The units load the key file directly; missing/invalid keys log one error per worker
and make submission return 503; readiness does not assert vault availability.
The unit installer's dry-run and apply modes now check source metadata as root,
using isolated system Python before any unit replacement. An existing source must
be a regular file (not a symlink), owned by root:root, with exact mode 0400.
`/`, `/etc`, and `/etc/verdaxis` must be root-owned directories without group or
other write permission; symlink parents are refused. **An absent key source (or
absent `/etc/verdaxis`) is refused**: the installer prints the exact empty-placeholder
command above, because a missing file would fail the backend start. An empty
root:root 0400 placeholder passes, as does a real ring; the check never reads
key contents. It checks the host source, not the systemd credential copy, whose
owner can be the service user. Run this installer preflight after restoring or
rotating a key file; a prior check does not protect later metadata changes.
The code deploy dry-run does not perform this host metadata check.
Unreadable source files can still fail systemd startup. Test KYC storage separately
from readiness. Development/test explicit paths are not covered by this root-only
host check; use synthetic documents and keys there, not real customer evidence.
No live action was performed by the implementation task.
