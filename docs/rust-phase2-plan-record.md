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
PD5 founder default: hide old best-price events before the approved cutoff and
retain their rows. The precise cutoff instant must be pinned in evidence before
feed/counter acceptance; do not infer it from a developer clock or source commit.
No `app/` or `alembic/` changes are authorized.

## Ownership

This integration lane owns Cargo manifests, runtime composition, comparator,
runner, CI/deploy files and marked blocks in `rs/read/src/routes/mod.rs`. Parallel
route lanes add only their own marked registration block; merge every block.
Tasks 11–15 run here in order. Report and re-estimate after Task 13. Task 21 and
all measurements remain unstarted.
