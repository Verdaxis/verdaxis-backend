# Phase 2 release runbook (Tasks 22 and 23)

This runbook covers the release integration that Tasks 22 and 23 prepare: the
private runtime settings, the deploy transaction, and the Caddy steps that move
the six Phase 2 route groups. Every live step needs the separate Task 24 and
Task 25 authorization. Nothing here deploys, reloads Caddy or measures.

## 1. Private runtime settings (Task 23)

The canonical deploy transaction (`scripts/deploy.sh`, see
`docs/runtime-hardening.md`) runs `scripts/prepare_rust_runtime.py` after the
migration helper and before the restart window. It reads the deployed `.env`
with no-follow semantics and writes `.runtime-rust/runtime.env` at mode 0600.

| Key | Required | Rule |
| --- | --- | --- |
| `JWT_SECRET` | yes | at least 32 characters; projected byte for byte |
| `JWT_SECRET_PREVIOUS` | no | only while a key rotation is in progress; blank means absent |
| `JWT_ISSUER`, `JWT_AUDIENCE` | yes | projected byte for byte |
| `FRONTEND_URL` | yes | absolute `http` or `https` URL; the referral link uses it unchanged |
| `EMAIL_FROM` | yes | the invite sender, unchanged |
| `RESEND_API_KEY` | yes | the referral invite provider key |

- Python and Rust read the same parsed values, so a token that Python issues
  verifies in Rust. Nothing is trimmed in silence.
- A missing, blank or short required value, or a control character, stops the
  preparation before any restart. The refusal names the key, never its value.
- The migrator URL and every other provider key stay out of the file.
- Values are double-quoted and escaped for systemd's `EnvironmentFile=` when
  they contain spaces or punctuation.

The Rust service refuses to start without the complete set, so deploy a Phase 2
binary only together with this preparer (both are on `feat/rust-phase-2`).

### Release metadata: the watchlist producer activation (PD5)

`.runtime-release.env` is the only place that may record
`WATCHLIST_PRODUCER_ACTIVATED_AT`, as an RFC 3339 instant with its offset.
Phase 2 releases record none: no trusted producer exists before Phase 8, and
the PD4-G guard hides every best-price event by type. An unreadable recorded
value stops the Rust start. The preparer never projects the key from `.env`.

## 2. Deploy and readiness

Use the existing dry-run, approval and deploy order in `README.md` and
`docs/runtime-hardening.md`. After the restart, both readiness checks must pass
for the exact release SHA: Python's public `/health/ready` and the Rust
loopback `/health/ready` (`127.0.0.1:8010` in production, `127.0.0.1:8011` in
staging). The Phase 2 routes are mounted in the Rust process, but Caddy still
sends every Phase 2 request to Python until the steps below.

## 3. Caddy steps

`scripts/rust_caddy_candidate.py --site <Caddyfile> --environment <env>
--group <step> --output-dir <new private folder>` writes `candidate.caddy` and
`rollback.caddy` (the exact previous file). It never reloads Caddy. Validate the
candidate with `caddy validate` before an authorized reload, and apply one step
per window.

1. **Preconditions.** The redacted access log and the Python fallback steps are
   installed and accepted.
2. **Probe the quota authority.** Before the `preauth` step, the running Rust
   release must answer the authority:

   ```bash
   curl -sS -o /dev/null -D - http://127.0.0.1:8010/internal/admin-preauth
   ```

   Require status 204 and the header `x-verdaxis-preauth-counted: 1` (staging:
   port 8011). A 404 means the binary has no authority route: stop. The probe
   uses one admin request of the loopback address's quota.
3. **`--group preauth`.** Every `/api/admin/` request is then counted once by
   the Rust authority before Caddy chooses an owner. All groups stay on Python.
4. **One group per step:** `notifications`, `preferences`, `feedback`,
   `watchlists`, `alerts` or `referrals`. The step refuses a group that is not
   on Python or a gate that is not Rust. Feedback POST and the admin feedback
   GET move together; referral resolve (R05) moves with referrals and stays
   unauthenticated. The legacy watchlist entry routes (W09, W10) stay on Python.

## 4. Rollback

- **One group:** apply that step's saved `rollback.caddy`. In production,
  `rollback(text, "production", "<group>")` in the same script derives the
  same change from the current file. The group's matchers then stop matching
  and the site's Python fallback serves it, without a passive-health window.
- **The gate:** `rollback(text, "production", "preauth")` removes the pre-auth
  block only when every group is back on Python. It compares each group's
  upstreams, so it still works after a later snippet revision.
- **The binary:** the Python fallback in every group pool keeps service up if
  the Rust process stops; a write is retried on Python only after a refused
  connection, never after a request was sent.

## 5. After each step

- `caddy validate` passed on the candidate before the reload.
- Both readiness checks still pass for the release SHA.
- The access log shows the moved group answered by Rust and no new 5xx or 502.
- Admin requests over the shared limit get 429 with `RATE_LIMITED` and
  `Retry-After: 60`, from either owner.
