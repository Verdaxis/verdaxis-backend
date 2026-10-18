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

## Ownership

This integration lane owns Cargo manifests, runtime composition, comparator,
runner, CI/deploy files and marked blocks in `rs/read/src/routes/mod.rs`. Parallel
route lanes add only their own marked registration block; merge every block.
Tasks 11–15 run here in order. Report and re-estimate after Task 13. Task 21 and
all measurements remain unstarted.
