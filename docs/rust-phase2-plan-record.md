# Phase 2 integration plan record

## Accepted Task 7 dispositions

PM accepted Tasks 6 and 7 at the integration handoff following `dee697f3`.

### JSON syntax diagnostics — explicit approved difference

Rule `json-syntax-wording-v1`, recorded in
`tests/rust_contract/phase2/json-syntax-differences.json`:

- HTTP status, error `type`, `loc` and `input` remain equal.
- Only `msg` and `ctx.error` wording may differ between Python's parser and serde.
- This is not a global error-message relaxation. Ordinary validation errors,
  authentication errors and all other envelopes remain exact.
- Both stacks check the vectors, including multiline and Unicode input. Rust
  converts serde line/byte columns to an absolute Unicode character location.
- No default comparator profile was changed. Differential callers must identify
  this named approved difference explicitly; an unexplained difference still fails.

### Shared quotas — Task 22 cutover gate

Rate-limited write groups, feedback first, stay on Python until Task 22 proves
one pre-auth `/api/admin/` counter across real separate Python and Rust upstream
processes, before the route split. Count each request once. A Rust-local bucket,
a shared in-process handle, or independent Python counters cannot satisfy this
cutover gate. Preserve exact written endpoint quotas and 429 responses; do not
multiply them by the Python worker count.

`SharedLimits` and `middleware_with_limits` are integration seams, not evidence
of deployed ingress wiring. Candidate or production acceptance must retain this
missing evidence until the separate-process test passes.

## Watchlist policy handoff

PD1–PD4 and PD6 use the approved corrected Rust behavior, not live Python defects.
Reference-only Python branches remain unreleased and are never merged or pushed.
PM disposition PD4-G (decision 94) supersedes cutoff-only protection for the
current Python producer. Hide all `SLICE_BEST_PRICE_MOVED` rows before and after
any cutoff; retain their rows. Event type is the durable deny property because
current rows have no trusted producer attestation. A currently public source ID
cannot attest the historical old/new aggregate prices. Source-linked PIN/order
events require the full public-order scope before feed pagination or metrics.
Approved-difference vectors are in `watchlist-approved-differences.json`.

User-visible effect: watchlist feeds show no best-price change events until the
producer is replaced. Unread counts and latest-event metrics exclude those rows.

PD5 remains for a later trusted producer: the cutoff is a per-environment release
activation instant at which corrected production and Rust reads are both live.
Deploy must record it explicitly in runtime release metadata, with no clock or
configuration default. Staging and production values are independent. Current
PD4-G protection does not depend on the cutoff. No `app/` or `alembic/` changes
are authorized.

### New task after Task 20 — trusted Rust watchlist event producer

Not started. Derive events from committed market state through the existing
market event outbox, using the one shared full public-order relation. The
market-write path must atomically provide a durable mutation identity, affected
order/slice keys, before/after evidence needed for price and PIN transitions,
and participant scope in the economic transaction. Consume committed events
with replay-safe identity and deterministic ordering; do not publish from an
uncommitted request or install a second dispatcher owner. Preserve explicit
producer identity/provenance so read admission can distinguish trusted events
from legacy Python rows. Review old/new price evidence, duplicates, replay,
concurrency, visibility transitions and rollback before removing PD4-G.

Guard removal is part of this new task, not a flag on today's event rows. It
requires approved producer activation metadata and tests proving both feeds
and counts use the same trusted-source rule. Keep historical rows and apply
PD5 to the later trusted producer's per-environment activation boundary.

## Task 15 — watchlist feed and read state

`rs/read/src/crud/watchlists/events.rs` serves W07 and W08 through the shared
authenticated transaction: keyset pagination with `(created_at DESC, id DESC)`,
the PD4-G guard applied in SQL before `LIMIT`, and a read acknowledgement that
writes nothing for an already-read event and refuses a guarded event before any
write. The update requires `is_read=false`, so a concurrent second reader
writes no row version. The client contract is pinned in
`tests/rust_contract/phase2/watchlist-events.json` and asserted against the
serialized response.

Evidence: 21 Rust tests, 10 of them on disposable PostgreSQL. No Python
comparison is a gate.

The trusted producer that would let the PD4-G guard go is a design proposal,
not code: `docs/rust-phase2-task15-producer-design.md`. It covers the outbox
consumer identity, additive producer columns, replay watermark, visibility
transitions, shadow mode, activation metadata, rollback and the guard-removal
preconditions. PM approval is required before implementation.

## Approved difference PD8 — price-alert cap concurrency

PD8 — Concurrent A01 calls by different users in one organization at four active
alerts can both return 201 and leave six active rows in Python. Approved Rust
behavior: after authenticated user admission and before active count, take a
per-organization transaction advisory lock in a fixed price-alert-cap namespace;
admit one create and return the existing cap-refusal 403 to the other, leaving
five active alerts. Keep Python unchanged. Only this race response and its
refused candidate row are an approved difference; all non-race contracts remain
unchanged. Evidence/vectors: `tests/rust_contract/phase2/pd8-differences.json`
and `docs/rust-phase2/task16-pd8-evidence.json`.

The lock key is SHA-256 over `verdaxis:price-alert-cap:v1:` and the full
organization UUID. It follows the authenticated user lock and is released by
commit or rollback. The disposable application role already holds the privilege;
no ACL grant or organization row lock is added. Because legacy Python does not
take this lock, this is not a claim that a simultaneous Python writer obeys the
repaired cap.

## Task 23 runtime projection notes

Task 23 owns the shared runtime projection. The route components above stay
unmounted until it lands; feedback also stays on Python until Task 22 supplies
the cross-process quota authority.

- Pass the raw `FRONTEND_URL` value to `ReferralSettings::new` without trimming.
  Referral link construction must use the configured string exactly.
- Inject the existing verification settings, `EMAIL_FROM`, the production mail
  client built from `RESEND_API_KEY`, and the shared quota authority into the
  composed routers. Do not create a second settings or quota owner.
- Merge the exported route factory (`routes::referral_routes(settings)`), and
  compose the notification, preference, feedback and price-alert routers through
  their exported seams. Keep the market write boundary unmounted: Task 21 stays
  excluded, and no market write route may be activated.
- Keep the marked registration blocks intact when composing; the shared runner
  selects route groups through them.

## Ownership

This integration lane owns Cargo manifests, runtime composition, comparator,
runner, CI/deploy files and marked blocks in `rs/read/src/routes/mod.rs`. Parallel
route lanes add only their own marked registration block; merge every block.
Tasks 11–15 run here in order. Report and re-estimate after Task 13. Task 21 and
all measurements remain unstarted.
