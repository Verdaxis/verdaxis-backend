# KYC document vault: design stop

## Blocking conflict

The requested absolute-path `LoadCredential=kyc-document-keys:/etc/verdaxis/...` directive makes a missing source file fatal at systemd service startup. The backend then cannot log its own startup error, serve KYC-only HTTP 503 responses, or keep readiness available. Current staging startup runs the backend directly from the service (`deploy/systemd/verdaxis-backend-staging.service:19-23`). Unit installation does not start or restart services (`docs/runtime-hardening.md:244-256`).

Evidence: [systemd v255 credential loader](https://github.com/systemd/systemd/blob/v255/src/core/exec-credential.c#L415-L491) sets `missing_ok = false` for absolute paths. Its ENOENT exception permits a configured `SetCredential` fallback; otherwise it returns the error. The [systemd manual](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html#LoadCredential=) distinguishes nonfatal missing inherited credentials from absolute-path sources. No live systemd version was inspected.

Proposed correction, subject to approval: retain the requested absolute-path `LoadCredential` and add `SetCredential=kyc-document-keys:{}` as a non-secret, deliberately invalid fallback in both backend units. A missing source would supply `{}`, which strict application validation must reject with the required startup log and KYC-only 503. Existing valid credentials take precedence. This adds no master key to source. Verify this behavior against the supported systemd version in a disposable Linux test before acceptance. Unreadable source files and other systemd credential setup failures remain service-level failures; document that boundary explicitly.

## Deletion review so far

- Monitor cleanup physically deletes a user and then an empty organization (`app/routers/monitor.py:58-82`). Inference: if either identity has retained documents, RESTRICT would reject this path. Proposed fix: check document retention before either deletion and return a stable conflict without deleting either identity. Do not delete documents or weaken the foreign keys.
- Optional SQLAdmin registers User and Organization ModelViews without local deletion overrides (`app/admin.py:47-58`, `app/admin.py:113-114`). It is disabled by default (`app/config.py:283`). Inference: enabled SQLAdmin needs an explicit retention-aware deletion policy. Library deletion defaults were not verified.
- No account-deletion route was found in the searched `app/routers/auth_simple.py`; search is not proof that every indirect deletion path is absent.
- Demo seed cleanup deletes market records, not users or organizations (`app/seeds/market_seed.py:134-145`).
- PostgreSQL fixtures use disposable `TRUNCATE ... CASCADE` (`tests/postgres/conftest.py:114-116`, `tests/postgres/conftest.py:150-151`). Unit organization-delete and runtime-role cleanup test locations were found but not fully reviewed. The full deletion inventory is incomplete because this task stopped at the design conflict.

## State and resume point

No application, migration, ACL, dependency, or unit changes were made. No keys were created. No agents, background jobs, live hosts, or live databases were used.

No tests ran: implementation stopped before TDD. Local PostgreSQL availability was not checked. No new revision or checkpoint row exists. The current checkpoint file ends with `kycg_20261014_staging -> kycg_20261014_staging` (`deploy/migration-checkpoints.tsv:43`).

Resume after approval of missing-credential behavior and retention-aware deletion handling. Finish the deletion inventory, review the implementation plan, then use failing tests first for the requested vault, route, migration, ACL, and unit contracts. Keep approve/reject request contracts unchanged and retain the specified RESTRICT foreign keys.
