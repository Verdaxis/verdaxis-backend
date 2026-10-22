# Stream component interfaces

## Scope and ownership

The `streams` cutover group contains GET `/api/stream/prices`,
`/api/stream/orderbook`, and `/api/stream/trades`, with explicit trailing-slash
aliases. Other methods are not part of the selector. Direct Rust requests use a
stream-local 405 response with `Allow: GET`; HEAD has no response body.
`/api/stream/activity` remains Python-owned. `/api/auth/stream-token` belongs to
the authentication component. This component does not issue tokens or create
watchlist events. Feed and metrics eligibility rules remain in force.

The JSON files in `tests/rust_contract/phase6/` define the route inventory,
client fields, terminal controls and required scenario set. They specify
functional contracts, not JSON byte layout or exception message wording.
The inventory is a requirement list, not evidence that later tasks have run.

## Integration seams

Keep `AppState`, the current read and health pools, the serving binary, release
identity and runtime configuration owner. Inject a separate `Arc<Streams>` into
the stream router. Do not place dispatcher state in each REST route.
`start_streams` returns an unavailable route handle on subsystem failure; it
does not make unrelated REST startup fail. Enabled required streams make the
existing four-key readiness response non-OK while unavailable.

The current auth seams are `auth::token::VerificationSettings`, `verify_access`,
`auth::claims::AccountState`, and `validate_account`. `verify_access` is
access-only, and the signature/claim helpers are private. Stream admission must
use a shared cryptographic profile through this owner, not call access validation
and replace the token type. The current issuance representation is
`VerifiedAccess.issued_at: Option<DateTime<Utc>>` plus a crate-private range flag;
there is not yet a public `Issuance` type. The auth owner must expose one bounded
representation for the stream profile. It must retain inclusive microsecond
cutoffs and password-cutoff precedence. No separate signing configuration,
issuer, authentication store or execution policy belongs to this module.

No Phase 4 or Phase 5 interface is required by this contract. Any later need for
one requires an explicit integration question before use.

## Reserved module and target ownership

| Module | Responsibility | Rust test target | PostgreSQL orchestration |
| --- | --- | --- | --- |
| `types`, `policy`, `bus` | Scoped registration and terminal controls | `streams_bus` | none |
| `auth` | Typed signed claims and current account/org admission | `streams_auth` | `test_phase6_authorization.py` |
| `store` | Bounded read-only replay snapshots | `streams_store` | `test_phase6_retention.py` |
| `election` | Dedicated physical-session sequencing | `streams_election` | `test_phase6_election.py` |
| `dispatch` | Ordered scans and public invalidation state | `streams_dispatch` | `test_phase6_dispatch.py` |
| `replay` | Replay/live merge and control priority | `streams_replay` | `test_phase6_replay.py` |
| `http` | Axum SSE routes and pre-header errors | `streams_http` | `test_phase6_http.py` |
| `runtime` | Supervision, budgets and shutdown | `streams_runtime` | `test_phase6_runtime.py` |
| existing watchlist modules | Feed/count guard integration | `streams_watchlist_guard` | `test_phase6_watchlist_guard.py` |

These module declarations and test targets are reservations only. Empty targets
are not passing component evidence. No stream route or runtime is activated by
this skeleton. Coexistence, race and cutover suites are also reserved in the
required inventory. The disposable PostgreSQL harness discovers those files in
its explicit `tests/postgres` selection; it does not use live databases.

## Frozen API names

The following are signatures for subsequent implementations, not public
constructors that bypass validation. Authority-bearing fields and physical
connections stay private. Concrete error enums must expose bounded reason
classes, not database text or payloads.

```rust
// types.rs
struct StreamPrincipal {
    user_id: Uuid,
    organization_id: Uuid,
    expires_at: DateTime<Utc>,
    issuance: Issuance, // shared auth representation, not a second parser
}
struct ReplayBoundary { discarded_through: i64, high_water: i64 }
struct StoredEvent {
    seq: i64,
    participants: Vec<Uuid>,
    event_type: String,
    payload: serde_json::Value,
    public_invalidated: bool,
}
enum Terminal {
    Overflow, Retention, CursorAhead, AuthExpired, AuthRevoked, Unavailable,
    Capacity, InvalidEvent, Shutdown,
}
// AuthRevoked has bounded authorization_changed / authorization_unavailable
// reasons. The client contract defines every terminal's event and cursor policy.

// bus.rs; PublicChannel has Prices and Orderbook, never caller strings.
impl StreamHub {
    fn subscribe_public(&self, channel: PublicChannel) -> Result<Subscription, Capacity>;
    fn subscribe_org(&self, org: Uuid) -> Result<Subscription, Capacity>;
    fn publish_private(&self, event: Arc<StoredEvent>) -> Result<(), HubError>;
    fn invalidate_public(&self, channel: PublicChannel) -> Result<(), HubError>;
}
// Subscription owns registration cleanup and separate bounded terminal control.

// auth.rs
fn verify_stream(token: &str, auth: &VerificationSettings,
    environment: &str, now: DateTime<Utc>) -> Result<StreamPrincipal, StreamError>;
async fn revalidate(db: &mut PgConnection, principal: &StreamPrincipal)
    -> Result<(), StreamError>;

// store.rs: read authority, org, after, through and limits are explicit inputs.
async fn replay_boundary(/* shared read authority */) -> Result<ReplayBoundary, StoreError>;
async fn replay_page(/* shared read authority, org, after, through, limits */)
    -> Result<ReplayPage, StoreError>;
// ReplayPage includes its same-snapshot boundary and bounded validated rows.

// election.rs: no public assign(pool) API.
impl ElectionSession {
    async fn assign_one_batch(&mut self) -> Result<AssignOutcome, ElectionError>;
}
// Outcomes distinguish not elected, committed, and lost/unknown connection.

// runtime.rs / mod.rs
async fn start_streams(/* attested config, pools, auth, budget */)
    -> (Arc<Streams>, StreamSupervisor);
impl StreamSupervisor { async fn shutdown(self) -> Result<(), ShutdownError>; }
fn stream_routes(streams: Arc<Streams>) -> axum::Router;
```

The shared sequencer key is `7264537260226625_i64`; the wake channel is
`verdaxis_market_events`. Each assignment batch takes and releases the session
lock on the same pinned physical connection. A replacement connection must be
attested and elected again. Keep the public read pool read-only. No client wait
may retain a database connection. Count the dedicated election slot without
subtracting any Python worker slots.

Replay snapshots include an explicit durable coverage boundary. Cursor equal to
the floor is valid; a cursor below it receives terminal `replay_retention`, and a
cursor above high-water receives terminal `cursor_ahead`. Global sequence holes
and other-organization rows alone do not cause reset. Register before capturing
high-water; merge in increasing sequence order without duplicates per connection.
Terminal control precedes buffered data. No business frame follows a terminal.

## Bounds and libraries

Use existing Axum SSE framing, Tokio bounded channels/timers/task ownership,
SQLx PostgreSQL connections, futures-util combinators and jsonwebtoken signature
validation. Add tokio-stream as a direct dependency only when its APIs are used.
No new broker, handwritten cryptography or SSE framing implementation is needed.
The application owns only scope, election, replay and lifecycle policy.

One policy owner validates: 200 subscriptions/channel, 2,000/process, 100 queued
events/subscription, 500 rows/page, 256 KiB serialized payload/row, 256 participant
UUIDs/row, 2 MiB/page, four batches and 32 wake notifications/turn. Inspect database
size metadata before decoding payloads. Invalid/oversized persisted events cause
bounded terminal resync, never truncation. These are proposed runtime safety
bounds, not a claim that existing writers enforce them. Writer inventory review
must resolve legitimate larger envelopes before implementation acceptance.

Use 30-second maximum idle heartbeats, 15-second private revalidation, two-second
poll/retry, two-second connection acquisition, five-second statements and
500-millisecond lock waits. The issuer owns the 60-second token lifetime.
Inject wall/monotonic clocks and finite barriers in tests only. No test-control
HTTP endpoint is present in normal builds.

## Commands and evidence interpretation

Run inventory tests with explicit test settings. Run all builds/tests through
the authorized staging test host and `heavy run 3600`. Common Rust checks are
`cargo fmt --manifest-path rs/Cargo.toml --all -- --check`, locked workspace
Clippy with all targets and `contract-tests`, and whole-lockfile `cargo audit`.
Each component target must run populated tests; compilation or zero tests is
not acceptance. PostgreSQL orchestration uses the disposable role-separated
harness, not an ambient production/staging URL. Frontend journeys use the
existing journey harness and its authorized host. No measurements are required.
