# Market write switch: repository pieces

The market write switch moves the 43 market operations from Python to Rust with
one Caddy change. This page describes the parts of the switch that live in this
repository. The release sequence, rollback and acceptance are operator
procedures and are not part of this page.

## Serving composition

`rs/read/src/runtime/market_write.rs` mounts every market operation in the
served router when the Phase 2 verification settings exist. The routes are
reachable only when Caddy sends traffic to them; health and readiness do not
change. The settings come from the deployed environment:

| Key | Default | Rule |
|---|---|---|
| `AUTO_MATCHING_ENABLED` | `true` | Python's boolean spellings; matching on create, amend and publish |
| `MARKET_SUPPORT_ENABLED` | `false` | Python's boolean spellings |
| `MARKET_SUPPORT_MAX_TTL_HOURS` | `168` | 1 to 720 |
| `MARKET_SUPPORT_CONTEXT_TTL_MINUTES` | `60` | 1 to 1440 |
| `MARKET_SUPPORT_CONSENT_VERSION` | `v1` | required when support is enabled; the placeholder is refused in staging and production |
| `MARKET_SUPPORT_CONSENT_REFERENCE` | `market-support/v1` | as above |
| `MARKET_SUPPORT_BOOTSTRAP_ADMIN_USER_IDS` | empty | comma-separated; entries that are not UUIDs are ignored |

`scripts/prepare_rust_runtime.py` projects the keys present in the deployed
`.env` into `.runtime-rust/runtime.env` after the same checks. It never projects
the watchlist producer keys.

## Routing

`deploy/caddy/rust-market-write.caddy` defines `(verdaxis_rust_market)`. Argument
0 is the Rust upstream, argument 1 the Python upstream; when both are equal no
matcher is active. The matchers own exactly the method and path pairs of
`tests/rust_contract/phase89/switch-routes.json`: canonical paths only, plus both
forms of `POST /api/trades`. `HEAD`, `OPTIONS` and slash variants stay on Python,
which redirects a slash variant to its canonical path. There is no Python
fallback for a matched operation.

`deploy/caddy/phase10-phase7-retire.caddy` is the reviewed retirement snippet for
the 20 indicative operations (`tests/rust_contract/phase10/phase7-routes.json`).
It answers 404 with no upstream.

## Write markers

`rs/read/tests/fixtures/market_write_markers.sql` counts the durable rows that
only Rust market writes add once the switch is live: outbox rows with producer
evidence, trusted watchlist events, freeze rows, command receipts, the audit
actions of commission, support and freeze changes, dismissed suggestions, and
support context and capability rows. `rs/read/tests/market_write_serving.rs`
requires each successful market write through the served router to raise at
least one of them.

## Live updates

Every outbox insert of a Rust market write (`order_write`:
`append_market_effect`, `insert_outbox` and the organization invalidation in
`events.rs`) also runs `pg_notify('verdaxis_market_events', '')` in the same
transaction: the signal Python's writers send. PostgreSQL delivers it when the
transaction commits, so the dispatchers wake at once instead of at their next
poll. Three other Rust outbox writers send no wake and are delivered at the
next poll: organization approval (`business/organizations.rs`), user admission
(`runtime/phase4_adapters.rs`) and price alerts (`streams/price_alerts.rs`).

## Watchlist producer activation

`scripts/deploy.sh` takes two optional approval variables,
`WATCHLIST_EVENT_PRODUCER` (`legacy-python` or `rust-market-v1`) and
`WATCHLIST_PRODUCER_ACTIVATED_AT` (a real UTC time in RFC 3339 form, offset `Z`
or `+00:00`; an impossible date or clock time is refused), and writes them to
`.runtime-release.env`:

- without a supplied value, the recorded keys are carried forward unchanged;
- a recorded activation instant never changes;
- the producer may move between its two names under the same instant;
- a producer needs an instant and an instant needs a producer;
- below the producer checkpoint both keys are dropped, and supplying one is
  refused.

`deploy.sh --dry-run` prints the recorded and supplied value of each key.

After activation, the operator creates the producer watermark once:

```
verdaxis-read --bootstrap-watchlist-producer --budget-file <prepared budget path>
```

It runs under the deployed environment, refuses before the producer
checkpoint, and prints `outcome=created watermark=<n>`,
`outcome=already_present watermark=<n>` or `refused reason=<reason>`.
