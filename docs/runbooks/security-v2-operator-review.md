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
slice; a heavy candidate cannot consume another candidate's cap. Raw KYC
documents are not retained by this backend.

If an admission/KYC/membership decision returns
`409 EXECUTION_INVALIDATION_BUSY` with `Retry-After: 1`, no review transition
was committed. Wait at least the bounded retry interval, reload the case, and
retry the single decision. Do not bypass the endpoint with direct bulk SQL.

Do not auto-approve. Hold all user admission, organization admission,
membership, and KYC decisions for an authenticated operator using the existing
single-case decision endpoints. Hold production KYC execution enforcement until
the product owner explicitly approves rollout.
