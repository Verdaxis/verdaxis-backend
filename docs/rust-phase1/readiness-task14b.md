# Task 14b source readiness

This work changes source only. It does not authorize activation, publication,
installation, a live comparison, or public routing. Decision 96: no measurements.

## Schema review

`p0_20261016_union` descends from `kycd_20261015_staging`. Its upgrade inserts
missing canonical product/delivery rows without changing existing values,
replaces `trades.ck_trades_snapshot`, widens the market-product checks on
`market_indications`, `fair_price_bands`, and `physical_stems`, and replaces
`verdaxis_validate_trade_snapshot()`. It removes or changes no queried table,
column, type, or read permission. The replacement trigger applies to writes;
Rust does not invoke it.

Reviewed query families in `rs/read/sql/` and inline SQL in `rs/read/src/`:

| Family | Compatibility with union |
| --- | --- |
| Catalog products/points | Exact identity joins to exported policy; added canonical rows already exist in that policy. Existing row values remain authoritative. |
| Shared order eligibility, keyed slices, private digest, lists/snapshot/map/count/distinct/listings, live benchmarks | No referenced column or type changes. Product/lane qualification stays policy-bound, not check-constraint-bound. |
| Formal trade snapshots, quarantine, tape, summary/latest/reference, curve trades | Snapshot column layout is unchanged. Expanded checks admit the distinct policy products; evidence predicates still use immutable snapshot identities. No trigger call. |
| Curve indications/bands/stems, depth/orders/benchmarks and MCP market reads | Signal check expansion changes admissible rows, not projection. Policy and trusted-ingestion predicates remain in force. |
| Ports/intelligence/news | Unaffected tables. |
| Runtime role/membership, connection budget, session settings, Alembic singleton and readiness | Unaffected system/schema metadata. The revision check consumes the compiled JSON allowlist. |

The allowlist is `deploy/rust/allowed-revisions.json`, compiled with `include_str!`
by `runtime/config.rs` and packaged as runtime/artifact policy. It retains the
previous checkpoint and adds the exact union checkpoint. It grants no production
activation: runtime configuration still refuses production.

Disposable contracts **do cover the union schema**: `run_disposable.py` calls
`scripts/verify_migrations.sh`, whose disposable upgrade reaches Alembic head, including
this migration. Its historical runtime fixture label is not the migration target.
That is populated disposable correctness evidence, not proof of current live
staging data or permission to migrate staging.

## Gates

All runs use `verdaxis-staging-test` and `heavy run 3600`. No local tests.

- Schema test first run: exit 101 (missing required build `RELEASE_SHA`, not a red test).
- Corrected schema red: exit 101; exact union revision refused as expected.
- Schema green: exit 0; all 15 runtime tests pass, including production refusal.

## Staging policy

The committed staging policy now allows Rust at `127.0.0.1:8011`. No public
route, unit, installer, or deploy helper changed. Production reads neither
activation record, has no Rust manifest entry, and remains disabled. Tests
execute the existing policy loader for both staging and production.

- Policy initial red: exit 2 (missing test import; corrected before implementation).
- Policy corrected red: exit 1; disabled staging policy rejected as expected.
- First combined green: exit 1; 48 passed, one fixture readiness failure in
  `test_host_key_matching_the_pinned_digest_is_preferred`.
- Separate rerun: unit policy suite exit 0 (15 passed); release suite exit 0
  (34 passed). No source change was needed for the fixture readiness rerun.

## Staging comparator interface

After separate operator authorization, candidate installation and private case/floor
approval, the interface is:

```bash
umask 077
/path/to/reviewed/python -m scripts.rust_contract.shadow \
  --config /private/approved-14b.toml --duration-seconds 1800
```

This is an interface, not a live command approval. Pin the comparator's full
source SHA and supply separate `python_sha` and `candidate_sha`. The template
`staging.example.toml` intentionally has empty identities, no approved floor and
no usable case path. A 35-minute maximum is enforced; no measurements run.

The private JSON case file is an array with all R01–R27 IDs and each of
`list_markets`, `get_market_slice`, `plan_voyage_fuel`. Each entry supplies a
synthetic `case_id` (`R05-public`), `method`, `path`, `query` for GET or `body` for
POST, and `kind`. Groups are derived from route IDs, then scheduled round-robin.
Use only approved public catalog/port selections. R02 requires a real five-letter
port code, never `T4B_ONE`. Route paths and MCP tool names are allowlisted. No
credentials, extra headers, fixture endpoints or disposable token are accepted.
Keep the query mapping private; never commit this populated file.

One in-memory response retains status, relevant headers and raw bytes. The one
authority is `compare.compare_responses(profile="byte", live=True)`, first
Python-to-Python, then Python-to-Rust. Decision 85 permits only exact trade-tape
item permutations within equal `confirmed_at` groups. CSV, decimal spelling,
null/absent, other order and headers remain exact. Request-clock normalization
requires UTC and each call's own start/end interval; stored timestamps are not
removed. SDK slice clocks are checked in both text and structured content.

The loop has concurrency one, rolling 3 calls/10 seconds including health,
endpoint minute limits counting both Python calls, direct HTTP (no proxies or
redirects), a 2-second absolute alarm, 1 MiB body bound, 60-second busy/pressure
backoff and deploy-state checks before every call. All three health contracts
are checked; readiness identities are separate and exact. An identity mismatch
stops. A readiness transport failure pauses, then stops conservatively.

Exit 0 is PASS, 1 is difference/safety refusal, 2 is INCONCLUSIVE. A forced
same-length content difference is tested through the CLI and returns 1. A pass
requires the explicit approved floor on every route/tool, nonempty observations,
99% stable successful sandwiches with all attempts retained in the denominator,
and completed final readiness. Tool errors cannot supply economic coverage.
A difference stops at once. Diagnostics use the root `/body` schema path rather
than risk persisting data-dependent JSON keys. Files are exclusive-create 0600
in a private directory outside the source checkout, capped at 10 MiB. No body,
query, auth header, row ID, value or payload hash is persisted.

### Comparator test history

- Content red: exit 1, 11 failures for missing byte entry point/sandwich API.
- Content first green attempt: exit 1 (import and CSV test header corrected).
- Content green: exit 0, 61 passed.
- Staging safety red: exit 1, 30 missing-behavior failures.
- Staging initial implementation checks: exit 1 (imports, test duration corrected).
- Final-readiness red: exit 1, 91 passed / 1 failed; elapsed cap could falsely pass.
- Health/round-robin red: exit 1, 94 passed / 2 failed; then exit 0, 96 passed.
- MCP clock red: exit 1, 99 passed / 1 failed; then exit 0, 100 passed.
- Tool/evidence admission red: exit 1, 38 passed / 2 failed; corrected.
- Busy/oversize red: exit 1, 105 passed / 1 failed; corrected to pause for status
  independently of the body outcome.
- Full contract harness suite: exit 0, 684 passed, five existing warnings.
- Final full contract harness suite: exit 0, 685 passed, five existing Pydantic
  deprecation warnings. Includes transport deadline/body bounds and CLI failure.
- Initial Rust formatting check: exit 1; local editor hook had applied a different
  rustfmt style to existing test code. Restored the pinned-toolchain formatting.
- Final Rust formatting: exit 0. Final runtime suite: exit 0, 15 passed.

Push/PR/CI is pending clarification: the brief requests a push and draft PR but
also explicitly says not to push because the PM owns it. No origin push or PR
creation was performed. CI for these local commits has therefore not run.

## Canonical path correction after decisions 99 and 101–103

The accepted integration merge is `06a62b5028e38aa0e846d0bb44d77ef52d771fff`.
The earlier candidate-only artifact/identity discussion is superseded: normal
staging pushes publish signed artifacts, and canonical Python/Rust activation
intentionally shares one SHA. The current source is policy-off again to permit
an actual first bootstrap deploy followed by the root-owned unit installation.
A later separately reviewed policy-on staging release supplies activation SHA S.
No workflow change or independent identity is required.

Source review found no installed-unit refusal in the previous helper: an enabled
release could reach the joint restart with no Rust unit. Added a staging-only
LoadState preflight, before the lock/state/source mutation, refusing not-found,
masked and error states. Artifact verification still precedes state/source
mutation and restart. Unit byte compatibility must be checked by the operator
between bootstrap and activation sources; LoadState alone does not attest bytes.

Bootstrap regression red: exit 1, five expected failures (three unavailable unit
states and policy-off checks). Green: exit 0, 52 release/unit tests passed.
The first source diff against staging has exactly two app files and no Alembic
changes: the approved deterministic UUID tie-break for two list routes, and
partial-fill seed quantities. No other Python request behavior change was found.
The full scoped stat is retained in the private PM runbook, not payload evidence.

## Remaining operator gates

Candidate artifact publication and independent candidate activation remain owned
by the PM. No deploy helper or artifact workflow change is included. The proposed
short comparison floor still needs explicit PM approval. Do not infer a live PASS
from these source checks.
