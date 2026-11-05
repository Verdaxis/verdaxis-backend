# verdaxis-backend — AI Context Map

> **Stack:** fastapi, axum | sqlalchemy | unknown | python

> 230 routes | 330 models | 0 components | 215 lib files | 158 env vars | 20 middleware | 43% test coverage
> **Token savings:** this file is ~50,900 tokens. Without it, AI exploration would cost ~339,000 tokens. **Saves ~288,200 tokens per conversation.**
> **Last scanned:** 2026-10-22 20:18 — re-run after significant changes

---

# Routes

## CRUD Resources

- **``** GET | POST | GET/:id | PUT/:id | DELETE/:id
- **`/inventory`** GET | POST | GET/:id | PATCH/:id | DELETE/:id → Inventory
- **`/admin/subscriptions`** GET | GET/:id | PUT/:id → Subscription
- **`/api/alerts`** GET | POST | GET/:id | DELETE/:id → Alert
- **`/api/watchlists`** GET | POST | GET/:id | DELETE/:id → Watchlist

## Other Routes

### fastapi

- `GET` `/` params() ✓
- `GET` `/health` params() ✓
- `GET` `/health/live` params() ✓
- `GET` `/health/ready` params() [db] ✓
- `POST` `/endpoint` → in: Annotated
- `GET` `/premium-feature` → in: Subscriptio [db]
- `GET` `/activity` params() [auth, cache]
- `GET` `/api/product-usage` params() → out: ProductUsageResponse [auth, db]
- `GET` `/api/overview` params() → out: ProductUsageResponse [db]
- `GET` `/api/daily` params() → out: ProductUsageResponse [db]
- `GET` `/api/users` params() → out: ProductUsageResponse [db] ✓
- `PUT` `/api/users/{user_id}/reject` params(user_id) → out: ProductUsageResponse [db]
- `GET` `/api/product-analytics/overview` params() → out: ProductUsageResponse
- `GET` `/api/product-analytics/acquisition` params() → out: ProductUsageResponse
- `GET` `/api/product-analytics/activation` params() → out: ProductUsageResponse
- `GET` `/api/product-analytics/engagement` params() → out: ProductUsageResponse
- `GET` `/api/product-analytics/marketplace` params() → out: ProductUsageResponse
- `GET` `/api/product-analytics/retention` params() → out: ProductUsageResponse
- `GET` `/api/product-analytics/reliability` params() → out: ProductUsageResponse [db]
- `GET` `/api/admin/onboarding-attention` params() → out: AttentionResponse ✓
- `POST` `/ai/chat` params()
- `GET` `/admin/audit-logs` params() → in: Annotated, out: list [db]
- `POST` `/api/login` params() → in: _Request, out: LoginResponse [auth, db]
- `POST` `/api/refresh` params() → in: _Request, out: LoginResponse [auth, db]
- `POST` `/api/logout` params() → in: _Request, out: LoginResponse [auth]
- `GET` `/api/stream-token` params() → in: Annotated, out: LoginResponse [auth]
- `GET` `/api/admin/invitations/organizations` params() → in: Annotated, out: LoginResponse [db]
- `POST` `/api/admin/invitations` params() → in: _Request, out: LoginResponse [db]
- `POST` `/api/invitations/resolve` params() → in: _Request, out: LoginResponse [auth, db]
- `POST` `/api/invitations/accept` params() → in: _Request, out: LoginResponse [auth, db]
- `POST` `/api/register` params() → in: _Request, out: LoginResponse [auth, db]
- `POST` `/api/register-with-org` params() → in: _Request, out: LoginResponse [auth, db]
- `POST` `/api/verify-email` params() → in: _Request, out: LoginResponse [auth, db]
- `POST` `/api/resend-verification-email` params() → in: _Request, out: LoginResponse [db, email]
- `GET` `/api/me` params() → in: Annotated, out: LoginResponse
- `PUT` `/api/me` params() → in: UserUpdate, out: LoginResponse
- `PUT` `/api/me/password` params() → in: UserUpdate, out: LoginResponse [auth, db]
- `POST` `/api/forgot-password` params() → in: _Request, out: LoginResponse [auth, db]
- `POST` `/api/reset-password` params() → in: _Request, out: LoginResponse [auth, db]
- `PUT` `/api/approve/{user_id}` params(user_id) → in: UserUpdate, out: LoginResponse [db]
- `PUT` `/api/organization/{organization_id}/approve` params(organization_id) → in: UserUpdate, out: LoginResponse [db]
- `PUT` `/api/organization/{organization_id}/reject` params(organization_id) → in: UserUpdate, out: LoginResponse [db]
- `PUT` `/api/reject/{user_id}` params(user_id) → in: UserUpdate, out: LoginResponse [db]
- `GET` `/api/admin/review-queue` params() → in: Annotated, out: LoginResponse [db, queue]
- `GET` `/api/admin/review-queue/{user_id}` params(user_id) → in: Annotated, out: LoginResponse [queue]
- `GET` `/api/organization-joins` params() → in: Annotated, out: LoginResponse [db]
- `PUT` `/api/organization-joins/{join_request_id}/approve` params(join_request_id) → in: UserUpdate, out: LoginResponse [db]
- `PUT` `/api/organization-joins/{join_request_id}/reject` params(join_request_id) → in: UserUpdate, out: LoginResponse
- `POST` `/api/survey` params() → in: _Request, out: LoginResponse [auth]
- `GET` `/products` params() → in: AsyncSessio, out: list [db] ✓
- `GET` `/delivery-points` params() → in: AsyncSessio, out: list [db]
- `GET` `/api/vessels/{vessel_id}/score` params(vessel_id) → out: ComplianceScoreResponse [auth, db]
- `GET` `/api/fleet` params() → out: ComplianceScoreResponse [db]
- `POST` `/api/scenario` params() → in: ScenarioInput, out: ComplianceScoreResponse [db]
- `POST` `/api/pricing-overlay` params() → in: ScenarioInput, out: ComplianceScoreResponse [db]
- `GET` `/api/fuels` params() → out: ComplianceScoreResponse
- `GET` `/api/v1/table` params() → in: Optional, out: ForwardCurveTableResponse
- `GET` `/api/v1/slice` params() → in: Optional, out: ForwardCurveTableResponse
- `GET` `/api/v1/board` params() → in: Optional, out: ForwardCurveTableResponse
- `GET` `/api/v1` params() → in: Optional, out: ForwardCurveTableResponse ✓
- `GET` `/api/v1/export` params() → in: Optional, out: ForwardCurveTableResponse
- `POST` `/api/feedback` params() → out: FeedbackCreated ✓
- `GET` `/api/admin/feedback` params() → out: FeedbackCreated [db] ✓
- `POST` `/inventory/{item_id}/publish` params(item_id) → out: List [db]
- `GET` `/listings` params() → in: Annotated, out: List [db]
- `GET` `/listings/my` params() → in: Annotated, out: List [db]
- `POST` `/submit` params() [auth, db, upload]
- `GET` `/admin/{user_id}/documents` params(user_id) → in: Annotated [db]
- `GET` `/admin/documents/{document_id}` params(document_id) → in: Annotated [cache]
- `GET` `/status` params() → in: Annotated ✓
- `PUT` `/admin/{user_id}/approve` params(user_id) → in: uuid [db]
- `PUT` `/admin/{user_id}/reject` params(user_id) → in: uuid [db]
- `GET` `/capabilities` params() → in: Annotated, out: list
- `POST` `/capability-assignments` params() → out: list [db]
- `POST` `/capability-assignments/{assignment_id}/revoke` params(assignment_id) → out: list [db]
- `GET` `/organizations` params() → in: Annotated, out: list [db]
- `GET` `/organizations/{organization_id}/entry` params(organization_id) → in: Annotated, out: list
- `POST` `/contexts` params() → out: list [db]
- `GET` `/contexts/active` params() → in: Annotated, out: list [db]
- `GET` `/contexts/{context_id}` params(context_id) → in: Annotated, out: list
- `POST` `/contexts/{context_id}/exit` params(context_id) → out: list
- `POST` `/organizations/{organization_id}/authorizations` params(organization_id) → out: list [auth, db]
- `GET` `/organizations/{organization_id}/authorizations` params(organization_id) → in: Annotated, out: list [auth, db]
- `POST` `/organizations/{organization_id}/listings` params(organization_id) → out: list [auth, db]
- `GET` `/organizations/{organization_id}/listings` params(organization_id) → in: Annotated, out: list [db]
- `POST` `/organizations/{organization_id}/listings/{order_id}/cancel` params(organization_id, order_id) → out: list ✓
- `POST` `/organizations/{organization_id}/authorizations/{authorization_id}/revoke` params(organization_id, authorization_id) → out: list [auth, db]
- `GET` `/organizations/{organization_id}/context` params(organization_id) → in: Annotated, out: list [auth, db]
- `GET` `/api/suggestions` params() → in: Annotated [db]
- `PATCH` `/api/suggestions/{order_id}/dismiss` params(order_id) → in: UUID [db]
- `POST` `/signup-canary-cleanup` params() → in: CanaryCleanupRequest, out: CanaryCleanupResponse [db]
- `POST` `/{negotiation_id}/counter` params(negotiation_id) → out: NegotiationResponse
- `POST` `/{negotiation_id}/accept` params(negotiation_id) → out: NegotiationResponse
- `POST` `/{negotiation_id}/decline` params(negotiation_id) → out: NegotiationResponse
- `GET` `/unread-count` params() → in: in, out: list [db]
- `PATCH` `/{notification_id}/read` params(notification_id) → in: uuid, out: list [db]
- `PATCH` `/read-all` params() → in: uuid, out: list [db]
- `GET` `/snapshot` params() → in: MarketProduc, out: OrderBookSnapshotResponse [db] ✓
- `GET` `/bids` params() → in: MarketProduc, out: OrderBookSnapshotResponse [db] ✓
- `GET` `/asks` params() → in: MarketProduc, out: OrderBookSnapshotResponse [db] ✓
- `GET` `/with-ci` params() → in: MarketProduc, out: OrderBookSnapshotResponse [db] ✓
- `GET` `/my` params() → in: MarketProduc, out: OrderBookSnapshotResponse [db]
- `GET` `/my/latest-ask-template` params() → in: MarketProduc, out: OrderBookSnapshotResponse [db]
- `GET` `/aggregated` params() → in: MarketProduc, out: OrderBookSnapshotResponse ✓
- `GET` `/product-counts` params() → in: MarketProduc, out: OrderBookSnapshotResponse [db] ✓
- `GET` `/map-summary` params() → in: MarketProduc, out: OrderBookSnapshotResponse ✓
- `GET` `/map-summary/compact` params() → in: MarketProduc, out: OrderBookSnapshotResponse ✓
- `GET` `/regions` params() → in: MarketProduc, out: OrderBookSnapshotResponse [db] ✓
- `GET` `/fuel-types` params() → in: MarketProduc, out: OrderBookSnapshotResponse [db] ✓
- `POST` `/{order_id}/cancel` params(order_id) → out: OrderBookSnapshotResponse [auth, db]
- `GET` `/admin/commissions` params() → in: Use, out: list [db]
- `GET` `/admin/commissions/summary` params() → in: Use, out: list [db]
- `PUT` `/admin/commissions/{commission_id}` params(commission_id) → in: UUID, out: list [db]
- `GET` `/ports` params() → in: Annotated, out: List [db]
- `GET` `/ports/{port_id}` params(port_id) → in: Annotated, out: List [db]
- `GET` `/api` params() → in: Use [db] ✓
- `PUT` `/api/{namespace}` params(namespace) [db]
- `GET` `/reference` params() → out: PriceDiscoveryResponse [auth]
- `GET` `/reference/export` params() → out: PriceDiscoveryResponse [auth]
- `POST` `/my-code` params() → in: Annotated, out: ReferralCodeResponse
- `GET` `/my-referrals` params() → in: Annotated, out: ReferralCodeResponse [db]
- `GET` `/leaderboard` params() → in: Annotated, out: ReferralCodeResponse [db]
- `POST` `/invite` params() → in: Annotated, out: ReferralCodeResponse [db]
- `GET` `/resolve/{code}` params(code) → in: Annotated, out: ReferralCodeResponse [db]
- `POST` `/{rfq_id}/quote` params(rfq_id) → out: RFQResponse
- `PUT` `/{rfq_id}/quotes/{quote_id}` params(rfq_id, quote_id) → out: RFQResponse
- `POST` `/{rfq_id}/quotes/{quote_id}/withdraw` params(rfq_id, quote_id) → out: RFQResponse
- `POST` `/{rfq_id}/accept/{quote_id}` params(rfq_id, quote_id) → out: RFQResponse
- `POST` `/{rfq_id}/cancel` params(rfq_id) → out: RFQResponse [db]
- `GET` `/api/prices` params() [auth, cache] ✓
- `GET` `/api/orderbook` params() [cache] ✓
- `GET` `/api/trades` params() [auth, db, cache] ✓
- `GET` `/subscriptions/fees` params() → in: Annotated, out: FeeScheduleResponse
- `GET` `/subscriptions/me` params() → in: Annotated, out: FeeScheduleResponse
- `POST` `/{offer_id}/withdraw` params(offer_id) → out: SupplierOfferList
- `POST` `/` params() → out: TradeResponse [db] ✓
- `GET` `/summary` params() → out: TradeResponse [db]
- `PUT` `/{trade_id}/confirm` params(trade_id) → out: TradeResponse [db]
- `PUT` `/{trade_id}/decline` params(trade_id) → out: TradeResponse
- `PUT` `/{trade_id}/deliver` params(trade_id) → out: TradeResponse [auth]
- `POST` `/{trade_id}/pay` params(trade_id) → out: TradeResponse
- `GET` `/vessels` params() → in: Annotated, out: List [db]
- `GET` `/vessels/{vessel_id}` params(vessel_id) → in: Annotated, out: List [db]
- `POST` `/me` params() → in: Annotated, out: list
- `POST` `/{watchlist_id}/targets` params(watchlist_id) → in: Annotated, out: list [db]
- `DELETE` `/{watchlist_id}/targets/{target_id}` params(watchlist_id, target_id) → in: UUID, out: list [db]
- `GET` `/{watchlist_id}/events` params(watchlist_id) → in: Annotated, out: list
- `PATCH` `/{watchlist_id}/events/{event_id}` params(watchlist_id, event_id) → in: UUID, out: list
- `POST` `/{watchlist_id}/entries` params(watchlist_id) → in: Annotated, out: list [db]
- `DELETE` `/{watchlist_id}/entries/{entry_id}` params(watchlist_id, entry_id) → in: UUID, out: list [db]
- `GET` `/.well-known/verdaxis-disposable-test` params() [auth] ✓
- `POST` `/__contract__/order-write` params() [auth, db]
- `GET` `/owner-cleanup-proof` params() → in: Annotated ✓
- `GET` `/one` params() → out: AlertResponse ✓
- `GET` `/many` params() → out: AlertResponse ✓
- `POST` `/upload` params() → in: UploadFile [auth, upload] ✓
- `GET` `/api/admin/analytics/overview` ✓

### axum

- `GET` `/__contract__/live-benchmarks` [auth, cache] ✓
- `POST` `/__contract__/phase2/write` [auth, db] ✓
- `GET` `/api/users/me/preferences` [auth, db] ✓
- `PUT` `/api/users/me/preferences/{namespace}` params(namespace) [auth, db] ✓
- `POST` `/api/referrals/my-code` [auth] ✓
- `POST` `/api/referrals/invite` [auth] ✓
- `GET` `/api/referrals/my-referrals` [auth] ✓
- `HEAD` `/api/referrals/my-referrals` [auth] ✓
- `GET` `/api/referrals/leaderboard` [auth] ✓
- `HEAD` `/api/referrals/leaderboard` [auth] ✓
- `GET` `/api/referrals/resolve/{code}` params(code) [auth] ✓
- `HEAD` `/api/referrals/resolve/{code}` params(code) [auth] ✓
- `POST` `/api/watchlists/me` [auth, db] ✓
- `POST` `/api/watchlists/{watchlist_id}/targets` params(watchlist_id) [auth, db] ✓
- `DELETE` `/api/watchlists/{watchlist_id}/targets/{target_id}` params(watchlist_id, target_id) [auth, db] ✓
- `GET` `/api/watchlists/{watchlist_id}/events` params(watchlist_id) [auth, db] ✓
- `PATCH` `/api/watchlists/{watchlist_id}/events/{event_id}` params(watchlist_id, event_id) [auth, db] ✓
- `GET` `/api/catalog/products` [db, cache] ✓
- `GET` `/api/catalog/delivery-points` [db, cache] ✓
- `GET` `/api/curves/forward/table` [db] ✓
- `GET` `/api/curves/forward/slice` [db] ✓
- `GET` `/api/curves/forward/board` [db] ✓
- `GET` `/api/curves/forward` [db] ✓
- `GET` `/api/curves/forward/export` [db] ✓
- `GET` `/api/news` [db, cache] ✓
- `GET` `/api/orderbook/bids` [auth, db, cache] ✓
- `GET` `/api/orderbook/asks` [auth, db, cache] ✓
- `POST` `/api/orderbook` [auth, db, cache] ✓
- `GET` `/api/orderbook/with-ci` [auth, db, cache] ✓
- `GET` `/api/orderbook/aggregated` [db] ✓
- `GET` `/api/orderbook/product-counts` [db] ✓
- `GET` `/api/orderbook/map-summary` [db] ✓
- `GET` `/api/orderbook/map-summary/compact` [db] ✓
- `GET` `/api/orderbook/products` [db] ✓
- `GET` `/api/orderbook/regions` [db] ✓
- `GET` `/api/orderbook/fuel-types` [db] ✓
- `GET` `/api/ports` [db, cache] ✓
- `GET` `/api/ports/{port_id}` params(port_id) [db, cache] ✓
- `GET` `/api/prices/reference` [db] ✓
- `GET` `/api/prices/reference/export` [db] ✓
- `GET` `/api/trade-tape` [db] ✓
- `GET` `/api/notifications/unread-count` [auth, db] ✓
- `POST` `/emails` [auth, email] ✓
- `GET` `/fixture` [auth, db] ✓
- `GET` `/ok` [auth, db] ✓
- `GET` `/api/admin/analytics` [auth] ✓
- `GET` `/unhandled` ✓
- `GET` `/handled` ✓
- `GET` `/busy` ✓

---

# Schema

## sqlalchemy

### PriceAlert
- id: UUID (pk, default)
- org_id: UUID (fk)
- product_id: UUID (fk)
- delivery_point_id: UUID (fk, nullable)
- direction: String
- threshold_usd: Numeric
- is_active: Boolean (default)
- triggered_at: DateTime (nullable)
- created_at: DateTime

### AuditLog
- id: UUID (pk, default)
- user_id: UUID (fk, nullable)
- action: String (index)
- resource_type: String (index)
- resource_id: String (nullable)
- changes: unknown (nullable)
- ip_address: String (nullable)
- request_id: String (nullable)
- timestamp: DateTime (default, index)

### Benchmark
- id: UUID (pk, default)
- market_product: String
- delivery_point_id: UUID (fk)
- availability_window: String
- price_per_mt_usd: Numeric
- source: String (default)
- created_at: DateTime (default)
- updated_at: DateTime (default)

### Product
- id: UUID (pk, default)
- name: String (unique)
- fuel_type: String
- fuel_grade: String
- unit: String (default)
- min_lot_size: Numeric (default)
- spec_description: String (nullable)
- is_active: Boolean (default)
- created_at: DateTime

### DeliveryPoint
- id: UUID (pk, default)
- name: String (unique)
- region: String
- timezone: String (nullable)
- is_active: Boolean (default)
- created_at: DateTime

### MarketCommandResult
- id: UUID (pk, default)
- actor_user_id: unknown (fk)
- effective_organization_id: unknown (fk)
- support_context_id: unknown (fk, nullable)
- operation: String
- idempotency_key: String
- request_hash: String
- resource_type: String
- resource_id: UUID
- response_status: SmallInteger
- response_body: unknown (nullable)
- created_at: DateTime (default)

### TraceabilityEvent
- id: UUID (pk, default)
- direct_order_id: UUID
- stage: String
- location_name: String
- timestamp: DateTime
- verification_type: String
- verification_doc_url: String
- verification_hash: String
- is_verified: Boolean (default)

### ComplianceLedger
- id: UUID (pk, default)
- organization_id: unknown (fk)
- transaction_type: String
- amount: Numeric
- currency: String (default)
- units: Numeric
- description: String
- reference_id: String
- created_at: DateTime (default)

### FeedbackEntry
- id: UUID (pk, default)
- user_id: UUID (fk, index)
- message: String
- page: String (nullable)
- created_at: DateTime

### MarketSignalIngestionRun
- id: UUID (pk, default)
- signal_family: String
- source: String
- source_kind: String
- started_at: DateTime (default)
- verified_at: DateTime (nullable)
- created_at: DateTime (default)

### MarketIndication
- id: UUID (pk, default)
- market_product: String
- delivery_point_id: UUID (fk)
- availability_window: String
- side: String
- price_per_mt_usd: Numeric
- quantity_mt: Numeric (nullable)
- source: String
- source_record_id: String (nullable)
- source_event_id: String (nullable)
- trusted_ingestion_run_id: UUID (fk, nullable)
- is_demo: Boolean (default)
- is_verified_real: Boolean (default)
- verified_real_at: DateTime (nullable)
- observed_at: DateTime
- created_at: DateTime (default)

### FairPriceBand
- id: UUID (pk, default)
- market_product: String
- delivery_point_id: UUID (fk)
- availability_window: String
- low_price_per_mt_usd: Numeric
- mid_price_per_mt_usd: Numeric
- high_price_per_mt_usd: Numeric
- model_name: String
- model_version: String (nullable)
- source: String
- source_event_id: String (nullable)
- trusted_ingestion_run_id: UUID (fk, nullable)
- is_demo: Boolean (default)
- is_verified_real: Boolean (default)
- verified_real_at: DateTime (nullable)
- observed_at: DateTime
- created_at: DateTime (default)

### PhysicalStem
- id: UUID (pk, default)
- market_product: String
- delivery_point_id: UUID (fk)
- availability_window: String
- quantity_mt: Numeric
- stem_start: DateTime (nullable)
- stem_end: DateTime (nullable)
- status: String
- source: String
- stem_uid: String
- source_record_id: String (nullable)
- source_event_id: String (nullable)
- trusted_ingestion_run_id: UUID (fk, nullable)
- is_demo: Boolean (default)
- is_verified_real: Boolean (default)
- verified_real_at: DateTime (nullable)
- observed_at: DateTime
- created_at: DateTime (default)

### KYCDocument
- id: UUID (pk)
- submission_id: UUID
- user_id: UUID (fk)
- organization_id: UUID (fk)
- kind: Text
- content_type: Text
- size_bytes: int
- key_id: Text
- wrapped_key: LargeBinary
- key_nonce: LargeBinary
- data_nonce: LargeBinary
- ciphertext: LargeBinary
- created_at: DateTime

### KYCExecutionGrace
- id: UUID (pk, default)
- user_id: UUID (fk)
- organization_id: UUID (fk)
- kyc_status_at_enrollment: String
- enrolled_at: DateTime
- expires_at: DateTime
- created_by: String

### KYCGraceNotice
- grace_id: UUID (fk, pk)
- claimed_at: DateTime
- sent_at: DateTime (nullable)

### LiveSliceBenchmark
- id: UUID (pk, default)
- side: Enum
- market_product: String
- delivery_point_id: UUID (fk)
- availability_window: String
- benchmark_price_per_mt_usd: Numeric
- total_remaining_quantity_mt: Numeric
- order_count: Integer (default)
- source: String (default)
- created_at: DateTime (default)
- updated_at: DateTime (default)

### MarketEventOutbox
- id: UUID (pk, default)
- event_type: String
- aggregate_type: String
- aggregate_id: String
- participant_org_ids: JSON
- payload: JSON
- created_at: DateTime (default)
- dispatched_at: DateTime (nullable)
- stream_seq: BigInteger (nullable)
- delivery_attempts: Integer (default)
- last_error: Text (nullable)

### MarketSupportContext
- id: UUID (pk, default)
- actor_user_id: UUID (fk)
- organization_id: UUID (fk)
- accountable_user_id: UUID (fk)
- support_reference: String
- scope: Enum (default)
- started_at: DateTime (default)
- expires_at: DateTime
- ended_at: DateTime
- status: Enum (default)
- version: Integer (default)

### StaffCapabilityAssignment
- id: UUID (pk, default)
- user_id: UUID (fk)
- capability: Enum
- reason: String
- granted_by_user_id: UUID (fk)
- granted_at: DateTime (default)
- expires_at: DateTime
- revoked_at: DateTime
- revoked_by_user_id: UUID (fk)
- revocation_reason: String

### MarketSupportAuthorization
- id: UUID (pk, default)
- organization_id: UUID (fk)
- accountable_user_id: UUID (fk)
- market_support_context_id: UUID (fk, nullable)
- status: Enum (default)
- product_id: UUID (fk)
- delivery_point_id: UUID (fk)
- availability_window: String
- quantity_mt: Numeric
- price_per_mt_usd: Numeric
- order_side: String (default)
- authorization_expires_at: DateTime
- order_expires_at: DateTime
- is_anonymous: Boolean (default)
- certifications: JSON (default)
- certification_declared: Boolean
- certification_scheme: String
- specification_standard: String
- msds_available: Boolean
- carbon_intensity_gco2_mj: Numeric
- carbon_intensity_method: String
- feedstock: String
- origin: String
- off_spec: Boolean
- off_spec_notes: Text
- fame_terms: JSON (nullable)
- lng_terms: JSON (nullable)
- terms_digest: String
- evidence_reference: String
- evidence_sha256: String
- commercial_consent_version: String
- commercial_consent_reference: String
- support_case_reference: String
- instruction_at: DateTime
- acknowledge_exact_terms: Boolean
- acknowledge_executable_standing_order: Boolean
- idempotency_key: String
- idempotency_request_hash: String
- created_by_actor_user_id: UUID (fk)
- created_at: DateTime (default)
- consumed_at: DateTime
- revoked_at: DateTime
- revoked_by_actor_user_id: UUID (fk)
- revocation_reason: String

### InventoryItem
- id: UUID (pk, default)
- supplier_id: unknown (fk)
- owner_user_id: unknown (fk, nullable)
- port_id: unknown (fk)
- fuel_type: Enum
- product_name: String
- fame_terms: JSON (nullable)
- lng_terms: JSON (nullable)
- current_stock_mt: Numeric
- incoming_stock_mt: Numeric (default)
- reserved_stock_mt: Numeric (default)
- price_per_mt_usd: Numeric
- energy_density_mj_kg: Numeric
- is_certified: Boolean (default)
- certification_declared: Boolean (default)
- certification_scheme: String
- specification_standard: String
- msds_available: Boolean (default)
- carbon_intensity_gco2_mj: Numeric
- carbon_intensity_method: String
- feedstock: String
- origin: String
- off_spec: Boolean (default)
- off_spec_notes: Text
- updated_at: DateTime (default)
- _relations_: port: Port, supplier: Organization

### MatchSuggestion
- id: UUID (pk, default)
- bid_order_id: unknown (fk)
- ask_order_id: unknown (fk)
- score: Numeric
- match_reasons: JSON (default)
- status: Enum (default)
- recipient_org_id: unknown (fk)
- created_at: DateTime (default)
- _relations_: bid_order: OrderBookOrder, ask_order: OrderBookOrder, recipient_org: Organization

### Negotiation
- id: UUID (pk, default)
- bid_order_id: UUID (fk, nullable)
- ask_order_id: UUID (fk, nullable)
- initiator_org_id: UUID (fk)
- counterparty_org_id: UUID (fk)
- initiator_user_id: UUID (fk, nullable, index)
- counterparty_user_id: UUID (fk, nullable, index)
- accepted_by_user_id: UUID (fk, nullable)
- initiator_side: String
- product_id: UUID (fk)
- delivery_point_id: UUID (fk, nullable)
- availability_window: String (default)
- lng_terms_snapshot: JSON (nullable)
- fame_terms_snapshot: JSON (nullable)
- quantity_mt: Numeric
- current_price: Numeric
- status: Enum (default)
- last_actor_org_id: UUID (fk)
- trade_id: UUID (fk, nullable)
- expires_at: DateTime
- created_at: DateTime (default)
- updated_at: DateTime (default)
- _relations_: rounds: 

### NegotiationRound
- id: UUID (pk, default)
- negotiation_id: UUID (fk)
- round_number: Integer
- proposer_org_id: UUID (fk)
- proposer_user_id: UUID (fk, nullable)
- proposed_price: Numeric
- notes: Text (nullable)
- created_at: DateTime (default)
- _relations_: negotiation: 

### NewsItem
- id: UUID (pk, default)
- title: String
- summary: Text (nullable)
- source: String
- source_url: String
- url: String (unique)
- category: String (default)
- relevance: Integer (default)
- published_at: DateTime
- fetched_at: DateTime (default)

### Notification
- id: UUID (pk, default)
- recipient_id: unknown (fk)
- type: Enum
- title: String
- message: Text
- data: JSON (default)
- is_read: Boolean (default)
- created_at: DateTime (default)
- _relations_: recipient: User

### OrderBookOrder
- id: UUID (pk, default)
- acceptance_ordinal: BigInteger
- organization_id: unknown (fk)
- owner_user_id: unknown (fk, nullable, index)
- created_by_actor_user_id: unknown (fk, nullable)
- creation_method: Enum (default)
- support_authorization_id: UUID (fk, nullable)
- version: Integer (default)
- inventory_item_id: unknown (fk, nullable, index)
- provenance: Enum (default)
- side: Enum
- product_id: UUID (fk)
- delivery_point_id: UUID (fk, nullable)
- port_id: String (nullable)
- vessel_id: unknown (fk, nullable)
- quantity_mt: Numeric
- remaining_quantity_mt: Numeric
- price_per_mt_usd: Numeric
- availability_window: String (default)
- delivery_window_start: Date (nullable)
- delivery_window_end: Date (nullable)
- certifications: JSON (default)
- certification_declared: Boolean (default)
- certification_scheme: String (nullable)
- fame_terms: JSON (nullable)
- lng_terms: JSON (nullable)
- specification_standard: String (nullable)
- msds_available: Boolean (default)
- is_verdaxis_verified: Boolean (default)
- carbon_intensity_gco2_mj: Numeric (nullable)
- carbon_intensity_method: String (nullable)
- energy_density_mj_kg: Numeric (nullable)
- feedstock: String (nullable)
- origin: String (nullable)
- off_spec: Boolean (default)
- off_spec_notes: Text (nullable)
- status: Enum (default)
- expires_at: DateTime (nullable)
- idempotency_key: String (nullable)
- idempotency_operation: String (nullable)
- idempotency_request_hash: String (nullable)
- created_at: DateTime (default)
- updated_at: DateTime (default)
- _relations_: organization: Organization, product: Product, delivery_point: DeliveryPoint, vessel: Vessel, inventory_item: InventoryItem, bid_trades: Trade, ask_trades: Trade

### Trade
- id: UUID (pk, default)
- bid_order_id: unknown (fk, nullable)
- ask_order_id: unknown (fk, nullable)
- buyer_id: unknown (fk)
- seller_id: unknown (fk)
- buyer_user_id: unknown (fk, nullable, index)
- seller_user_id: unknown (fk, nullable, index)
- initiator_org_id: unknown (fk)
- buyer_provenance: Enum (default)
- seller_provenance: Enum (default)
- initiated_by: Enum
- is_anonymous: Boolean (default)
- product_id: UUID (fk, nullable)
- product_name: String (nullable)
- fuel_type: String (nullable)
- fuel_grade: String (nullable)
- market_product: String (nullable)
- delivery_point_id: UUID (fk, nullable)
- delivery_point_name: String (nullable)
- delivery_point_region: String (nullable)
- availability_window: String (nullable)
- market_snapshot_version: SmallInteger (nullable, default)
- fame_terms_snapshot: JSON (nullable)
- lng_terms_snapshot: JSON (nullable)
- quantity_mt: Numeric
- price_per_mt_usd: Numeric
- status: Enum (default)
- final_quantity_mt: Numeric (nullable)
- final_price_per_mt: Numeric (nullable)
- final_total_usd: Numeric (nullable)
- commission_rate_pct: Numeric (default)
- commission_fee_per_mt_usd: Numeric (nullable)
- commission_plan: String (nullable)
- commission_amount_usd: Numeric (nullable)
- confirmed_at: DateTime (nullable)
- delivered_at: DateTime (nullable)
- paid_at: DateTime (nullable)
- created_at: DateTime (default)
- idempotency_key: String (nullable)
- idempotency_operation: String (nullable)
- idempotency_request_hash: String (nullable)
- _relations_: bid_order: OrderBookOrder, ask_order: OrderBookOrder, buyer: Organization, seller: Organization, commission: Commission

### Commission
- id: UUID (pk, default)
- match_id: unknown (unique)
- trade_id: unknown (fk, nullable)
- amount_usd: Numeric
- status: Enum (default)
- invoice_number: String
- invoice_date: Date
- payment_date: Date
- notes: String
- created_at: DateTime (default)
- updated_at: DateTime (default)
- _relations_: trade: Trade

### Port
- id: String (pk)
- name: String
- country: String
- location: Geography
- timezone: String
- is_active: Boolean (default)
- _relations_: intelligence: PortIntelligence, inventory_items: InventoryItem

### PortIntelligence
- id: UUID (pk, default)
- port_id: str (fk)
- congestion_level: Enum
- methanol_price_avg: Numeric
- biofuel_price_avg: Numeric
- captured_at: DateTime (default)
- _relations_: port: 

### Vessel
- id: UUID (pk, default)
- organization_id: unknown (fk)
- name: String
- imo_number: String (unique)
- vessel_type: String
- flag_state: String
- dwt: Numeric
- cii_rating: String
- eu_ets_status: String
- fueleu_status: String
- current_location: unknown
- previous_location: unknown
- updated_at: DateTime (default)
- _relations_: organization: Organization

### ProducerProject
- id: UUID (pk, default)
- name: String
- fuel_type: String
- capacity_kt_per_year: Numeric
- country: String
- region: String
- location: unknown (nullable)
- cod_date: Date (nullable)
- cod_year: Integer (nullable)
- status: Enum (default)
- data_source: String
- gena_project_id: String (unique, nullable)
- organization_id: unknown (fk, nullable)
- feedstock: String
- technology: String
- carbon_intensity_gco2_mj: Numeric
- notes: Text
- created_at: DateTime (default)
- updated_at: DateTime (default)
- _relations_: organization: Organization

### UserLoginDay
- id: UUID (pk, default)
- activity_date: Date
- user_id: unknown (fk)
- organization_id: UUID (nullable)
- role: Enum (nullable)
- login_count: Integer (default)
- first_login_at: DateTime
- last_login_at: DateTime

### UserStatusTransition
- id: UUID (pk, default)
- user_id: unknown (fk)
- organization_id: UUID (nullable)
- role: Enum (nullable)
- from_status: Enum (nullable)
- to_status: Enum
- effective_at: DateTime (default)
- provenance: String (default)

### PublicReadCacheGeneration
- domain: Text (pk)
- generation: BigInteger

### PublicReadCacheEvent
- domain: Text (pk)
- generation: BigInteger (pk)

### PublicReadCacheIncarnation
- singleton: Boolean (pk)
- database_epoch: Text
- primary_identity: Text
- authority_revision: BigInteger
- active: Boolean

### Referral
- id: UUID (pk, default)
- referrer_id: UUID (fk)
- referred_user_id: UUID (fk, unique)
- referral_code_used: String
- status: Enum (default)
- created_at: DateTime (default)
- verified_at: DateTime (nullable)
- activated_at: DateTime (nullable)
- _relations_: referrer: , referred_user: 

### RefreshSession
- id: UUID (pk, default)
- user_id: unknown (fk, index)
- family_id: UUID (index)
- jti_hash: String (unique)
- device_id_hash: String (nullable, index)
- replaced_by_jti_hash: String (nullable)
- rotation_grace_until: DateTime (nullable)
- revoked: Boolean (default)
- expires_at: DateTime (index)
- created_at: DateTime (default)
- last_used_at: DateTime (nullable)

### PendingRegistration
- id: UUID (pk, default)
- token_hash: String (unique)
- email: String
- password_hash: String
- first_name: String (nullable)
- last_name: String (nullable)
- role: Enum (nullable)
- referral_code: String (nullable)
- expires_at: DateTime
- used_at: DateTime (nullable)
- created_at: DateTime (default)

### OrganizationJoinRequest
- id: UUID (pk, default)
- user_id: unknown (fk, index)
- organization_id: unknown (fk, index)
- status: Enum (default)
- reviewed_by: unknown (fk, nullable)
- reviewed_at: DateTime (nullable)
- review_note: Text (nullable)
- created_at: DateTime (default)

### RFQ
- id: UUID (pk, default)
- buyer_org_id: UUID (fk)
- buyer_user_id: unknown (fk, nullable, index)
- product_id: UUID (fk)
- delivery_point_id: UUID (fk, nullable)
- quantity_mt: Numeric
- target_price_per_mt: Numeric (nullable)
- availability_window: String (default)
- notes: Text (nullable)
- contract_terms: JSON (nullable)
- source_offer_id: unknown (fk, nullable)
- target_supplier_org_id: unknown (fk, nullable)
- source_offer_snapshot: JSON (nullable)
- is_anonymous: Boolean (default)
- status: Enum (default)
- accepted_quote_id: UUID (fk, nullable)
- trade_id: UUID (fk, nullable)
- expires_at: DateTime
- created_at: DateTime (default)
- _relations_: quotes: 

### RFQQuote
- id: UUID (pk, default)
- rfq_id: UUID (fk)
- seller_org_id: UUID (fk)
- seller_user_id: unknown (fk, nullable, index)
- price_per_mt_usd: Numeric
- notes: Text (nullable)
- offer_terms: JSON (nullable)
- expires_at: DateTime (nullable)
- revision: Integer (default)
- status: Enum (default)
- created_at: DateTime (default)
- _relations_: rfq: 

### SeedRun
- id: UUID (pk, default)
- seed_name: String
- environment: String
- run_metadata: JSON (nullable)
- created_at: DateTime (default)
- completed_at: DateTime (default)

### MarketRowQuarantine
- id: UUID (pk, default)
- source_table: String
- source_id: UUID
- original_row: JSON
- dependencies: JSON
- environment: String
- database_name: String
- reason: Text
- operator: String
- reference: String
- quarantined_at: DateTime (default)

### OrganizationMarketApproval
- organization_id: UUID (fk, pk)
- previous_verification_status: String
- reviewed_snapshot: JSON
- environment: String
- database_name: String
- reason: Text
- operator: String
- reference: String
- approved_at: DateTime (default)

### Subscription
- id: UUID (pk, default)
- org_id: UUID (fk, unique)
- tier: String (default)
- started_at: DateTime (nullable, default)
- expires_at: DateTime (nullable, default)
- is_active: Boolean (default)
- seller_fee_per_mt_usd: Numeric (nullable, default)
- _relations_: organization: Organization

### SupplierOffer
- id: UUID (pk, default)
- supplier_org_id: UUID (fk)
- supplier_user_id: UUID (fk)
- product_id: UUID (fk)
- delivery_point_id: UUID (fk)
- quantity_mt: Numeric
- min_fill_mt: Numeric
- price_per_mt_usd: Numeric
- availability_window: String
- listing_terms: JSON
- status: String (default)
- revision: Integer (default)
- expires_at: DateTime
- created_at: DateTime (default)
- updated_at: DateTime (default)
- idempotency_key: String
- idempotency_request_hash: String

### Organization
- id: UUID (pk, default)
- name: String
- domain: String (unique, nullable)
- type: Enum
- supplier_tier: Enum (nullable, default)
- tax_id: String
- country_code: String
- verification_status: String
- provenance: Enum
- created_at: DateTime (default)
- _relations_: users: , vessels: , orderbook_orders: 

### User
- id: UUID (pk, default)
- email: String (unique)
- password_hash: String
- first_name: String
- last_name: String
- role: Enum (nullable)
- status: Enum (default)
- pending_approval_email_transition_id: UUID (nullable)
- pending_approval_email_payload: unknown (nullable)
- pending_approval_email_retry_at: DateTime (nullable)
- organization_id: unknown (fk)
- last_login: DateTime
- password_changed_at: DateTime (nullable)
- authentication_revoked_at: DateTime (nullable)
- must_change_password: Boolean (default)
- created_at: DateTime (default)
- email_verified: Boolean (default)
- email_verification_token_hash: String (nullable, index)
- email_verification_token_expires_at: DateTime (nullable)
- kyc_status: String (default)
- kyc_organization_id: UUID (fk, nullable, index)
- kyc_rejection_reason: Text (nullable)
- kyc_external_evidence_reference: String (nullable)
- kyc_review_note: Text (nullable)
- kyc_reviewed_by: UUID (fk, nullable)
- kyc_reviewed_at: DateTime (nullable)
- password_reset_token_hash: String (nullable)
- password_reset_expires: DateTime (nullable)
- referral_code: String (unique, nullable)
- referred_by_id: UUID (fk, nullable)
- onboarding_use_case: String (nullable)
- onboarding_referral_source: String (nullable)
- _relations_: organization: , referrals_made: , referral_received: 

### UserPreference
- id: UUID (pk, default)
- user_id: UUID (fk, index)
- namespace: String
- value: JSON
- updated_at: DateTime

### Watchlist
- id: UUID (pk, default)
- user_id: UUID (fk)
- name: String
- kind: Enum (default)
- created_at: DateTime (default)
- _relations_: entries: , targets: , events: 

### WatchlistEntry
- id: UUID (pk, default)
- watchlist_id: UUID (fk)
- product_id: UUID (fk)
- delivery_point_id: UUID (fk, nullable)
- created_at: DateTime (default)
- _relations_: watchlist: 

### WatchlistTarget
- id: UUID (pk, default)
- watchlist_id: UUID (fk)
- target_type: Enum
- market_product_code: String (nullable)
- delivery_point_id: UUID (fk, nullable)
- availability_window_code: String (nullable)
- order_id: UUID (fk, nullable)
- snapshot_price_per_mt_usd: unknown (nullable)
- snapshot_quantity_mt: unknown (nullable)
- snapshot_remaining_quantity_mt: unknown (nullable)
- snapshot_status: String (nullable)
- snapshot_side: String (nullable)
- snapshot_market_product: String (nullable)
- snapshot_delivery_point_name: String (nullable)
- snapshot_availability_window: String (nullable)
- snapshot_counterparty_label: String (nullable)
- created_at: DateTime (default)
- _relations_: watchlist: , delivery_point: DeliveryPoint, order: OrderBookOrder, events: 

### WatchlistEvent
- id: UUID (pk, default)
- watchlist_id: UUID (fk)
- watchlist_target_id: UUID (fk)
- event_type: Enum
- event_payload: JSON (default)
- is_read: Boolean (default)
- created_at: DateTime (default)
- _relations_: watchlist: , target: 

## unknown

### AIChatRequest
- history: object[]
- message: string (required)

### AcquisitionKpis
- average_session_duration_seconds: DecimalMetricValue (required)
- cta_clicks: MetricValue (required)
- pageviews: MetricValue (required)
- visitors: MetricValue (required)
- visits: MetricValue (required)

### AcquisitionResponse
- calculator: CalculatorFunnel (required)
- cta_matrix: CtaMatrixRow[]
- entry_pages: RankedRow[]
- kpis: AcquisitionKpis (required)
- languages: RankedRow[]
- meta: AnalyticsMeta (required)
- previous_visitors_trend: SeriesPoint[]
- referrers: RankedRow[]
- visitors_trend: SeriesPoint[]
- visits_trend: SeriesPoint[]

### ActivationResponse
- drop_off: AggregateCell[]
- journey: JourneyStage[]
- meta: AnalyticsMeta (required)
- ratios: LabeledRatio[]
- time_to_first_live_order: DurationDistribution (required)
- time_to_first_login: DurationDistribution (required)

### ActivityTrend
- active_members: SeriesPoint[]
- confirmed_trades: SeriesPoint[]
- orders: SeriesPoint[]
- visitors: SeriesPoint[]

### AdminApproveBody
- external_evidence_reference: string (required)
- review_note: string (required)

### AdminDecisionBody
- reason: unknown

### AdminFeedbackEntry
- created_at: string(date-time) (required)
- id: string(uuid) (required, uuid)
- message: string (required)
- org_name: unknown (required)
- page: unknown (required)
- user_email: unknown (required)
- user_name: unknown (required)

### AdminFeedbackResponse
- items: AdminFeedbackEntry[] (required)
- total: integer (required)

### AdminInvitationCreate
- email: string(email) (required)
- first_name: string (required)
- last_name: unknown
- new_organization: unknown
- organization_id: unknown
- role: UserRole (required)

### AdminInvitationOrganizationCreate
- country_code: string (required)
- name: string (required)
- tax_id: unknown
- type: OrgType (required)

### AdminInvitationResponse
- acceptance_url: string (required)
- email: string (required)
- expires_at: string(date-time) (required)
- organization_created: boolean (required)
- organization_name: string (required)
- reissued: boolean (required)
- role: string (required)
- user_id: string(uuid) (required, uuid)

### AdminRejectBody
- reason: string (required)

### AdminUserEntry
- created_at: string(date-time) (required)
- email: string (required)
- email_verified: boolean
- first_name: unknown (required)
- id: string(uuid) (required, uuid)
- last_login: unknown
- last_name: unknown (required)
- must_change_password: boolean
- org_has_orders: boolean
- org_name: unknown (required)
- org_provenance: unknown (required)
- org_type: unknown (required)
- organization_id: unknown (required)
- role: string (required)
- status: string (required)

### AdminUsersResponse
- items: AdminUserEntry[] (required)
- total: integer (required)

### AggregateCell
- count: unknown
- key: string (required)
- suppressed: boolean

### AggregatedOrderbookResponse
- availability_window: string
- delivery_point_id: unknown
- delivery_point_name: unknown
- demo_status: MarketDemoStatus (required)
- evidence_class: string (required)
- fuel_type: string
- market_product: string (required)
- max_price: string (required)
- min_price: string (required)
- observed_at: string(date-time) (required)
- order_count: integer (required)
- product_id: string(uuid) (required, uuid)
- product_name: string
- product_total_order_count: integer (required)
- region: string
- scope: MarketScope
- side: OrderSide (required)
- source_kind: MarketSourceKind (required)
- total_quantity: string (required)

### AlertCreate
- delivery_point_id: unknown
- direction: string (required)
- product_id: string(uuid) (required, uuid)
- threshold_usd: unknown (required)

### AlertResponse
- created_at: string(date-time) (required)
- delivery_point_id: unknown (required)
- direction: string (required)
- id: string(uuid) (required, uuid)
- is_active: boolean (required)
- market_product: unknown
- org_id: string(uuid) (required, uuid)
- product_id: string(uuid) (required, uuid)
- product_name: unknown
- threshold_usd: string (required)
- triggered_at: unknown (required)

### AnalyticsCoverage
- authoritative: AnalyticsSourceCoverage (required)
- behavioral: AnalyticsSourceCoverage (required)
- login_history: AnalyticsSourceCoverage (required)
- reference: AnalyticsSourceCoverage (required)
- status_history: AnalyticsSourceCoverage (required)

### AnalyticsDataQuality
- cohort_complete: boolean
- legacy_timestamp_fallback_count: integer
- missing_commission_payment_date_count: integer
- missing_paid_at_count: integer
- suppressed_cell_count: integer

### AnalyticsMeta
- coverage: AnalyticsCoverage (required)
- data_quality: AnalyticsDataQuality (required)
- end: string(date-time) (required)
- observed_at: string(date-time) (required)
- previous_end: unknown (required)
- previous_start: unknown (required)
- start: string(date-time) (required)

### AnalyticsSourceCoverage
- coverage_end: unknown
- coverage_start: unknown
- diagnostic: unknown
- observed_at: unknown
- status: AnalyticsSourceStatus (required)

### AssistedListingCancel
- reason: string (required)

### AssistedListingCreate
- acknowledge_executable_standing_order: boolean (required)
- authorization_id: string(uuid) (required, uuid)

### AssistedListingResponse
- accountable_user_id: string(uuid) (required, uuid)
- created_by_actor_user_id: string(uuid) (required, uuid)
- creation_method: OrderCreationMethod (required)
- etag: string (required)
- order: OrderResponse (required)
- support_authorization_id: string(uuid) (required, uuid)
- version: integer (required)

### AttentionItem
- email: string (required)
- last_login: unknown (required)
- name: unknown (required)
- organization_name: unknown (required)
- role: unknown (required)
- since: string(date-time) (required)
- stage: string (required)

### AttentionResponse
- generated_at: string(date-time) (required)
- items: AttentionItem[] (required)

### AuditActivityRow
- action: string (required)
- actor_role: unknown
- occurred_at: string(date-time) (required)
- resource_type: unknown

### AuditLogResponse
- action: string (required)
- changes: unknown (required)
- id: string(uuid) (required, uuid)
- ip_address: unknown (required)
- request_id: unknown (required)
- resource_id: unknown (required)
- resource_type: string (required)
- timestamp: string(date-time) (required)
- user_id: unknown (required)

### AuthoritativeUsage
- order_placing_organizations: integer (required)
- registrations: integer (required)
- users_logging_in: integer (required)

### AuthorizationCreate
- accountable_user_id: string(uuid) (required, uuid)
- authorization_expires_at: string(date-time) (required)
- commercial_consent_reference: string (required)
- commercial_consent_version: string (required)
- evidence_reference: string (required)
- evidence_sha256: string (required)
- order: OrderCreate-Input (required)
- support_case_reference: unknown

### AuthorizationResponse
- accountable_user_id: string(uuid) (required, uuid)
- acknowledge_exact_terms: unknown (required)
- acknowledge_executable_standing_order: unknown (required)
- authorization_expires_at: string(date-time) (required)
- availability_window: string (required)
- commercial_consent_reference: string (required)
- commercial_consent_version: string (required)
- consumed_at: unknown (required)
- created_at: string(date-time) (required)
- created_by_actor_user_id: string(uuid) (required, uuid)
- delivery_point_id: string(uuid) (required, uuid)
- evidence_reference: string (required)
- evidence_sha256: unknown (required)
- fame_terms: unknown
- id: string(uuid) (required, uuid)
- instruction_at: unknown (required)
- lng_terms: unknown
- order: OrderCreate-Output (required)
- order_expires_at: unknown (required)
- organization_id: string(uuid) (required, uuid)
- price_per_mt_usd: string (required)
- product_id: string(uuid) (required, uuid)
- quantity_mt: string (required)
- revocation_reason: unknown (required)
- revoked_at: unknown (required)
- revoked_by_actor_user_id: unknown (required)
- status: MarketSupportAuthorizationStatus (required)
- support_case_reference: unknown (required)
- terms_digest: string (required)

### AuthorizationRevoke
- reason: string (required)

### BackendUnavailablePanel
- by_route_family: AggregateCell[]
- total: MetricValue (required)

### BehavioralUsage
- average_session_duration_seconds: number (required)
- daily_visitors: DailyVisitorPoint[] (required)
- event_series: DailyEventPoint[] (required)
- event_totals: object (required)
- pageviews: integer (required)
- top_entries: MetricEntry[] (required)
- top_referrers: MetricEntry[] (required)
- total_time_seconds: integer (required)
- visitors: integer (required)
- visits: integer (required)

### BenchmarkQuote
- availability_window: string (required)
- benchmark_price_per_mt_usd: string (required)
- delivery_point_id: string(uuid) (required, uuid)
- delivery_point_name: string (required)
- generated_at: string(date-time) (required)
- market_product: string (required)
- observed_at: unknown
- source: string (required)

### BenchmarkQuoteResponse
- generated_at: string(date-time) (required)
- items: BenchmarkQuote[] (required)

### Body_login_api_auth_login_post
- client_id: unknown
- client_secret: undefined(password)
- grant_type: unknown
- password: string(password) (required)
- scope: string
- username: string (required)

### Body_submit_kyc_api_kyc_submit_post
- company_doc: string (required)
- declared_company_name: unknown
- passport: string (required)
- registration_number: unknown

### CIAdjustedPrice
- base_price_per_mt: string (required)
- carbon_intensity_gco2_mj: string (required)
- compliance_cost_per_mt: string (required)
- effective_price_per_mt: string (required)
- financial_benefit_status: string
- fueleu_ghg_intensity: string (required)
- ghg_reduction_pct: string (required)

### CIIResponse
- rating: string (required)
- score: integer (required)

### CalculatorFunnel
- completions: MetricValue (required)
- starts: MetricValue (required)

### CapabilityAssignmentCreate
- capability: MarketSupportCapability (required)
- expires_at: unknown
- reason: string (required)
- user_id: string(uuid) (required, uuid)

### CapabilityAssignmentResponse
- capability: MarketSupportCapability (required)
- expires_at: unknown (required)
- granted_at: string(date-time) (required)
- granted_by_user_id: string(uuid) (required, uuid)
- id: string(uuid) (required, uuid)
- reason: string (required)
- revocation_reason: unknown (required)
- revoked_at: unknown (required)
- revoked_by_user_id: unknown (required)
- user_id: string(uuid) (required, uuid)

### CapabilityAssignmentRevoke
- reason: string (required)

### CohortCell
- cell: AggregateCell (required)
- offset: integer (required)
- pct: unknown

### CohortRow
- cells: CohortCell[]
- cohort_start: string(date) (required)
- size: AggregateCell (required)

### CollectorState
- diagnostic: unknown
- last_observation_at: unknown
- status: AnalyticsSourceStatus (required)

### CommercialSummary
- commission_invoiced_usd: unknown
- commission_pending_usd: unknown
- realized_gmv_usd: DecimalMetricValue (required)
- realized_revenue_usd: DecimalMetricValue (required)

### CommissionResponse
- amount_usd: string (required)
- created_at: string(date-time) (required)
- id: string(uuid) (required, uuid)
- invoice_date: unknown
- invoice_number: unknown
- match_id: string(uuid) (required, uuid)
- payment_date: unknown
- status: CommissionStatus (required)

### CommissionSummary
- invoiced_count: integer (required)
- paid_count: integer (required)
- pending_count: integer (required)
- total_invoiced_usd: string (required)
- total_paid_usd: string (required)
- total_pending_usd: string (required)

### CommissionUpdate
- invoice_date: unknown
- invoice_number: unknown
- notes: unknown
- payment_date: unknown
- status: unknown

### ComplianceScoreResponse
- cii: CIIResponse (required)
- eu_ets: EUETSResponse (required)
- fueleu: FuelEUResponse (required)
- overall_score: integer (required)
- recommendations: string[] (required)
- status: string (required)
- traffic_light: string (required)
- vessel_id: string (required)
- vessel_name: string (required)

### CtaMatrixRow
- clicks: unknown
- cta: string (required)
- placement: string (required)
- share_pct: unknown
- suppressed: boolean

### DailyEventPoint
- date: string (required)
- event: string (required)
- value: integer (required)

### DailyStat
- commission_usd: number (required)
- date: string(date) (required)
- gmv_usd: number (required)
- orders_placed: integer (required)
- trades_executed: integer (required)
- volume_mt: number (required)

### DailyVisitorPoint
- date: string (required)
- value: integer (required)

### DecimalMetricValue
- previous: unknown
- suppressed: boolean
- value: unknown

### DeliveryPointResponse
- id: string(uuid) (required, uuid)
- is_active: boolean (required)
- name: string (required)
- region: string (required)
- timezone: unknown

### DemandSignal
- availability_window_code: unknown
- bid_count: integer (required)
- created_at: string(date-time) (required)
- delivery_point_id: unknown
- delivery_point_name: unknown
- demo_status: MarketDemoStatus
- earliest_delivery: string (required)
- fuel_type: string (required)
- market_product_code: unknown
- max_price_per_mt: unknown (required)
- region: string (required)
- scope: MarketScope
- source_kind: MarketSourceKind
- unknown_count: integer
- urgency: UrgencyLevel (required)
- volume_mt: unknown (required)

### DurationBucket
- bucket: string (required)
- cell: AggregateCell (required)

### DurationDistribution
- buckets: DurationBucket[]
- median_hours: unknown
- sample_size: unknown
- suppressed: boolean

### EUETSResponse
- calculation_status: string (required)
- estimated_cost_eur: unknown (required)
- ets_price_per_tonne_eur: string (required)
- phase_in_pct: string (required)
- score: integer (required)
- total_co2_tonnes: unknown (required)

### EngagementKpis
- dau: MetricValue (required)
- mau: MetricValue (required)
- stickiness_pct: unknown
- wau: MetricValue (required)

### EngagementResponse
- active_members_trend: SeriesPoint[]
- feature_adoption: FeatureAdoptionRow[]
- kpis: EngagementKpis (required)
- meta: AnalyticsMeta (required)
- navigation_destinations: NavigationDestinationRow[]
- tutorial_steps: TutorialStepRow[]
- workflow_ratios: LabeledRatio[]

### ErrorDetail
- detail: string (required)

### ExecutionEligibilityResponse
- eligible: boolean
- grace_expires_at: unknown
- reason: string

### FameAskTerms-Input
- astm_grade: unknown
- batch_reference: unknown
- certificate_holder: string (required)
- certificate_reference: string (required)
- certificate_scope: unknown
- certificate_valid_until: string(date) (required)
- cfpp_c: unknown
- ci_basis: unknown
- ci_boundary: unknown
- ci_gco2e_mj: unknown
- ci_methodology: unknown
- cloud_point_c: unknown
- document_references: string[]
- en_climate_class: unknown
- evidence_due: string (required)
- evidence_status: string (required)
- feedstock_origin: unknown
- lhv_mj_kg: unknown
- neat_fame: boolean (required)
- nomination_status: string
- producing_site: unknown
- production_origin: unknown
- quality_evidence: unknown
- schema_version: integer
- shipping_location: unknown
- side: string (required)
- standard: string (required)
- standard_edition: string (required)
- sustainability_evidence: unknown
- sustainability_scheme: string (required)
- uco_mass_pct: integer (required)

### FameAskTerms-Output
- astm_grade: unknown
- batch_reference: unknown
- certificate_holder: string (required)
- certificate_reference: string (required)
- certificate_scope: unknown
- certificate_valid_until: string(date) (required)
- cfpp_c: unknown
- ci_basis: unknown
- ci_boundary: unknown
- ci_gco2e_mj: unknown
- ci_methodology: unknown
- cloud_point_c: unknown
- document_references: string[]
- en_climate_class: unknown
- evidence_due: string (required)
- evidence_status: string (required)
- feedstock_origin: unknown
- lhv_mj_kg: unknown
- neat_fame: boolean (required)
- nomination_status: string
- producing_site: unknown
- production_origin: unknown
- quality_evidence: unknown
- schema_version: integer
- shipping_location: unknown
- side: string (required)
- standard: string (required)
- standard_edition: string (required)
- sustainability_evidence: unknown
- sustainability_scheme: string (required)
- uco_mass_pct: integer (required)

### FameBidTerms-Input
- astm_grade: unknown
- ci_basis: unknown
- ci_boundary: unknown
- ci_methodology: unknown
- en_climate_class: unknown
- evidence_due: string
- max_cfpp_c: unknown
- max_ci_gco2e_mj: unknown
- max_cloud_point_c: unknown
- neat_fame: boolean (required)
- require_quality_evidence: boolean
- require_sustainability_evidence: boolean
- schema_version: integer
- side: string (required)
- standard: string (required)
- standard_edition: string (required)
- sustainability_scheme: string (required)

### FameBidTerms-Output
- astm_grade: unknown
- ci_basis: unknown
- ci_boundary: unknown
- ci_methodology: unknown
- en_climate_class: unknown
- evidence_due: string
- max_cfpp_c: unknown
- max_ci_gco2e_mj: unknown
- max_cloud_point_c: unknown
- neat_fame: boolean (required)
- require_quality_evidence: boolean
- require_sustainability_evidence: boolean
- schema_version: integer
- side: string (required)
- standard: string (required)
- standard_edition: string (required)
- sustainability_scheme: string (required)

### FameContractTerms-Input
- astm_grade: unknown
- claims_terms: string (required)
- delivery_basis: string (required)
- delivery_end: string(date) (required)
- delivery_start: string(date) (required)
- en_climate_class: unknown
- evidence_due: string (required)
- inspection_terms: string (required)
- max_cfpp_c: unknown
- max_ci_gco2e_mj: unknown
- min_fill_mt: unknown (required)
- named_location: string (required)
- neat_fame: boolean (required)
- payment_terms: string (required)
- quantity_tolerance_pct: unknown (required)
- schema_version: integer
- standard: string (required)
- standard_edition: string (required)
- sustainability_scheme: string (required)
- title_risk_terms: string (required)

### FameContractTerms-Output
- astm_grade: unknown
- claims_terms: string (required)
- delivery_basis: string (required)
- delivery_end: string(date) (required)
- delivery_start: string(date) (required)
- en_climate_class: unknown
- evidence_due: string (required)
- inspection_terms: string (required)
- max_cfpp_c: unknown
- max_ci_gco2e_mj: unknown
- min_fill_mt: string (required)
- named_location: string (required)
- neat_fame: boolean (required)
- payment_terms: string (required)
- quantity_tolerance_pct: string (required)
- schema_version: integer
- standard: string (required)
- standard_edition: string (required)
- sustainability_scheme: string (required)
- title_risk_terms: string (required)

### FameFuelSummary
- astm_grade: unknown
- certificate_valid_until: string(date) (required)
- cfpp_c: unknown
- ci_basis: unknown
- ci_boundary: unknown
- ci_gco2e_mj: unknown
- ci_methodology: unknown
- cloud_point_c: unknown
- en_climate_class: unknown
- evidence_status: string (required)
- feedstock_origin: unknown
- lhv_mj_kg: unknown
- neat_fame: boolean
- nomination_status: string (required)
- production_origin: unknown
- quality_evidence: unknown
- schema_version: integer
- shipping_location: unknown
- standard: string (required)
- standard_edition: string (required)
- sustainability_evidence: unknown
- sustainability_scheme: string (required)
- uco_mass_pct: integer (required)

### FameListingFuelTerms-Input
- astm_grade: unknown
- batch_reference: unknown
- certificate_holder: string (required)
- certificate_reference: string (required)
- certificate_scope: unknown
- certificate_valid_until: string(date) (required)
- cfpp_c: unknown
- ci_basis: unknown
- ci_boundary: unknown
- ci_gco2e_mj: unknown
- ci_methodology: unknown
- cloud_point_c: unknown
- document_references: string[]
- en_climate_class: unknown
- evidence_status: string (required)
- feedstock_origin: unknown
- lhv_mj_kg: unknown
- neat_fame: boolean (required)
- nomination_status: string
- producing_site: unknown
- production_origin: unknown
- quality_evidence: unknown
- schema_version: integer
- shipping_location: unknown
- standard: string (required)
- standard_edition: string (required)
- sustainability_evidence: unknown
- sustainability_scheme: string (required)
- uco_mass_pct: integer (required)

### FameListingFuelTerms-Output
- astm_grade: unknown
- batch_reference: unknown
- certificate_holder: string (required)
- certificate_reference: string (required)
- certificate_scope: unknown
- certificate_valid_until: string(date) (required)
- cfpp_c: unknown
- ci_basis: unknown
- ci_boundary: unknown
- ci_gco2e_mj: unknown
- ci_methodology: unknown
- cloud_point_c: unknown
- document_references: string[]
- en_climate_class: unknown
- evidence_status: string (required)
- feedstock_origin: unknown
- lhv_mj_kg: unknown
- neat_fame: boolean (required)
- nomination_status: string
- producing_site: unknown
- production_origin: unknown
- quality_evidence: unknown
- schema_version: integer
- shipping_location: unknown
- standard: string (required)
- standard_edition: string (required)
- sustainability_evidence: unknown
- sustainability_scheme: string (required)
- uco_mass_pct: integer (required)

### FameOfferTerms-Input
- astm_grade: unknown
- available_quantity_mt: unknown (required)
- batch_reference: string (required)
- certificate_holder: string (required)
- certificate_reference: string (required)
- certificate_scope: unknown
- certificate_valid_until: string(date) (required)
- cfpp_c: unknown
- ci_basis: unknown
- ci_boundary: unknown
- ci_gco2e_mj: unknown
- ci_methodology: unknown
- cloud_point_c: unknown
- document_references: string[]
- en_climate_class: unknown
- evidence_status: string (required)
- feedstock_origin: string (required)
- lhv_mj_kg: unknown
- matches_contract_terms: boolean (required)
- neat_fame: boolean
- producing_site: string (required)
- production_origin: string (required)
- quality_evidence: unknown
- schema_version: integer
- shipping_location: string (required)
- standard: string (required)
- standard_edition: string (required)
- sustainability_evidence: unknown
- sustainability_scheme: string (required)
- uco_mass_pct: integer (required)

### FameOfferTerms-Output
- astm_grade: unknown
- available_quantity_mt: string (required)
- batch_reference: string (required)
- certificate_holder: string (required)
- certificate_reference: string (required)
- certificate_scope: unknown
- certificate_valid_until: string(date) (required)
- cfpp_c: unknown
- ci_basis: unknown
- ci_boundary: unknown
- ci_gco2e_mj: unknown
- ci_methodology: unknown
- cloud_point_c: unknown
- document_references: string[]
- en_climate_class: unknown
- evidence_status: string (required)
- feedstock_origin: string (required)
- lhv_mj_kg: unknown
- matches_contract_terms: boolean (required)
- neat_fame: boolean
- producing_site: string (required)
- production_origin: string (required)
- quality_evidence: unknown
- schema_version: integer
- shipping_location: string (required)
- standard: string (required)
- standard_edition: string (required)
- sustainability_evidence: unknown
- sustainability_scheme: string (required)
- uco_mass_pct: integer (required)

### FamePublicAskTerms
- astm_grade: unknown
- certificate_valid_until: string(date) (required)
- cfpp_c: unknown
- ci_basis: unknown
- ci_boundary: unknown
- ci_gco2e_mj: unknown
- ci_methodology: unknown
- cloud_point_c: unknown
- en_climate_class: unknown
- evidence_due: string (required)
- evidence_status: string (required)
- feedstock_origin: unknown
- lhv_mj_kg: unknown
- neat_fame: boolean
- nomination_status: string (required)
- production_origin: unknown
- quality_evidence: unknown
- schema_version: integer
- shipping_location: unknown
- side: string (required)
- standard: string (required)
- standard_edition: string (required)
- sustainability_evidence: unknown
- sustainability_scheme: string (required)
- uco_mass_pct: integer (required)

### FamePublicTradeSnapshot
- ask: FamePublicAskTerms (required)
- bid: FameBidTerms-Output (required)
- schema_version: integer

### FameQualityEvidence-Input
- batch_reference: unknown
- laboratory: unknown
- reference: unknown
- results: FameQualityResult-Input[]
- sampled_on: unknown
- status: string (required)
- tested_on: unknown

### FameQualityEvidence-Output
- batch_reference: unknown
- laboratory: unknown
- reference: unknown
- results: FameQualityResult-Output[]
- sampled_on: unknown
- status: string (required)
- tested_on: unknown

### FameQualityResult-Input
- method: unknown
- property: string (required)
- value: unknown (required)

### FameQualityResult-Output
- method: unknown
- property: string (required)
- value: string (required)

### FameQualitySummary
- results: FameQualityResult-Output[] (required)
- status: string (required)

### FameSustainabilityEvidence-Input
- document_type: string (required)
- due: string (required)
- issuer: unknown
- quantity_mt: unknown
- reference: unknown
- status: string (required)
- supply_date: unknown

### FameSustainabilityEvidence-Output
- document_type: string (required)
- due: string (required)
- issuer: unknown
- quantity_mt: unknown
- reference: unknown
- status: string (required)
- supply_date: unknown

### FameSustainabilitySummary
- document_type: string (required)
- due: string (required)
- status: string (required)

### FameTradeSnapshot
- ask: FameAskTerms-Output (required)
- bid: FameBidTerms-Output (required)
- schema_version: integer

### FeatureAdoptionRow
- events: unknown
- family: string (required)
- suppressed: boolean

### FeeScheduleResponse
- buyer_fee_per_mt_usd: string
- currency: string
- seller_fee_per_mt_usd: object (required)

### FeedbackCreated
- created_at: string(date-time) (required)
- id: string(uuid) (required, uuid)

### FeedbackSubmission
- message: string (required)
- page: unknown

### FleetComplianceSummary
- amber_count: integer (required)
- average_score: number (required)
- green_count: integer (required)
- red_count: integer (required)
- total_vessels: integer (required)
- vessels: ComplianceScoreResponse[] (required)

### FleetDemandEntry
- avg_consumption_mt: integer (required)
- color: string (required)
- delivered_vessels: integer (required)
- fuel: string (required)
- ordered_vessels: integer (required)

### FleetDemandResponse
- entries: FleetDemandEntry[] (required)
- last_updated: string (required)
- sources: string[] (required)

### ForgotPasswordRequest
- email: string (required)

### ForwardCurveBoardCell
- availability_window: string (required)
- benchmark_mid: unknown
- benchmark_observed_at: unknown
- benchmark_source: unknown
- benchmark_source_kind: MarketSourceKind
- best_ask: unknown
- best_ask_source_kind: MarketSourceKind
- best_bid: unknown
- best_bid_source_kind: MarketSourceKind
- delivery_point_id: string(uuid) (required, uuid)
- delivery_point_name: string (required)
- demo_best_ask: unknown
- demo_best_bid: unknown
- demo_order_count: integer
- demo_status: MarketDemoStatus
- fair_price_band: unknown
- fair_price_band_provenance: ForwardCurveSignalProvenance
- indication_summary: ForwardCurveBoardIndicationSummary
- is_demo_benchmark: boolean
- market_product: string (required)
- order_count: integer
- order_observed_at: unknown
- order_source_kind: MarketSourceKind
- physical_stem_summary: ForwardCurveBoardPhysicalStemSummary
- product_id: string(uuid) (required, uuid)
- product_name: string (required)
- real_best_ask: unknown
- real_best_bid: unknown
- real_order_count: integer
- region: string (required)
- scope: MarketScope
- spread: unknown
- unknown_order_count: integer
- volume_mt: string

### ForwardCurveBoardDepthLevel
- demo_order_count: integer
- demo_status: MarketDemoStatus
- order_count: integer (required)
- price_per_mt_usd: string (required)
- quantity_mt: string (required)
- real_order_count: integer
- source_kind: MarketSourceKind
- unknown_order_count: integer

### ForwardCurveBoardFairPriceBand
- high_price_per_mt_usd: unknown
- low_price_per_mt_usd: unknown
- mid_price_per_mt_usd: unknown
- provenance: ForwardCurveSignalProvenance (required)

### ForwardCurveBoardFocus
- availability_window: string (required)
- curve: ForwardCurveBoardCell[] (required)
- delivery_point_id: string(uuid) (required, uuid)
- delivery_point_name: string (required)
- depth_asks: ForwardCurveBoardDepthLevel[] (required)
- depth_bids: ForwardCurveBoardDepthLevel[] (required)
- fair_price_band: unknown
- fair_price_band_provenance: ForwardCurveSignalProvenance
- indications: ForwardCurveBoardIndication[]
- market_product: string (required)
- physical_stems: ForwardCurveBoardPhysicalStem[]
- product_id: string(uuid) (required, uuid)
- product_name: string (required)
- region: string (required)

### ForwardCurveBoardIndication
- price_per_mt_usd: string (required)
- provenance: ForwardCurveSignalProvenance (required)
- quantity_mt: unknown
- side: ForwardCurveIndicationSide (required)

### ForwardCurveBoardIndicationSummary
- indication_count: integer
- latest_ask_price_per_mt_usd: unknown
- latest_bid_price_per_mt_usd: unknown
- latest_mid_price_per_mt_usd: unknown
- provenance: ForwardCurveSignalProvenance
- total_quantity_mt: unknown

### ForwardCurveBoardPhysicalStem
- provenance: ForwardCurveSignalProvenance (required)
- quantity_mt: string (required)
- status: ForwardCurvePhysicalStemStatus (required)
- stem_end: unknown
- stem_start: unknown

### ForwardCurveBoardPhysicalStemSummary
- available_quantity_mt: unknown
- earliest_stem_start: unknown
- latest_stem_end: unknown
- provenance: ForwardCurveSignalProvenance
- stem_count: integer
- tentative_quantity_mt: unknown

### ForwardCurveBoardPort
- cells: ForwardCurveBoardCell[] (required)
- delivery_point_id: string(uuid) (required, uuid)
- delivery_point_name: string (required)
- region: string (required)

### ForwardCurveBoardProduct
- market_product: string (required)
- product_id: string(uuid) (required, uuid)
- product_name: string (required)

### ForwardCurveBoardResponse
- availability_window: string (required)
- focus: ForwardCurveBoardFocus (required)
- generated_at: string(date-time) (required)
- ports: ForwardCurveBoardPort[] (required)
- products: ForwardCurveBoardProduct[] (required)

### ForwardCurveLabelPolicy
- allowed_terms: string[]
- disclaimer: unknown
- forbidden_terms: string[]
- public_label: string (required)
- tooltip: unknown

### ForwardCurveLatestSignal
- availability_window: string (required)
- delivery_point_id: string(uuid) (required, uuid)
- delivery_point_name: string (required)
- demo_status: MarketDemoStatus
- market_product: string (required)
- observed_at: unknown
- primary_signal_type: MarketSignalType
- primary_source_kind: MarketSourceKind
- primary_value: unknown
- public_source_label: string
- staleness_status: ForwardCurveStalenessStatus

### ForwardCurveMarketCell
- availability_window: string (required)
- benchmark_mid: unknown
- benchmark_observed_at: unknown
- benchmark_source_kind: MarketSourceKind
- best_ask: unknown
- best_bid: unknown
- delivery_point_id: string(uuid) (required, uuid)
- delivery_point_name: string (required)
- demo_best_ask: unknown
- demo_best_bid: unknown
- demo_order_count: integer
- demo_status: MarketDemoStatus
- fair_price_band: unknown
- fair_price_band_provenance: ForwardCurveSignalProvenance
- generated_at: string(date-time) (required)
- indication_summary: ForwardCurveBoardIndicationSummary
- is_executable: boolean
- is_reference: boolean
- label_policy: ForwardCurveLabelPolicy
- market_product: string (required)
- observed_at: unknown
- order_count: integer
- physical_stem_summary: ForwardCurveBoardPhysicalStemSummary
- primary_signal_type: MarketSignalType
- primary_source_kind: MarketSourceKind
- primary_value: unknown
- product_count: integer
- product_name: string (required)
- public_source_label: string
- real_best_ask: unknown
- real_best_bid: unknown
- real_order_count: integer
- region: string (required)
- representative_product_id: string(uuid) (required, uuid)
- scope: MarketScope
- spread: unknown
- staleness_status: ForwardCurveStalenessStatus
- unknown_order_count: integer
- volume_mt: string

### ForwardCurvePoint
- availability_window: string (required)
- best_ask: unknown
- best_bid: unknown
- mid_price: unknown
- order_count: integer (required)
- spread: unknown
- volume_mt: string (required)

### ForwardCurveResponse
- curve: ForwardCurvePoint[] (required)
- delivery_point_id: unknown
- generated_at: string(date-time) (required)
- product_id: string(uuid) (required, uuid)

### ForwardCurveSignalProvenance
- demo_count: integer
- demo_status: MarketDemoStatus
- generated_at: string(date-time) (required)
- observed_at: unknown
- real_count: integer
- scope: MarketScope
- signal_source_kind: ForwardCurveSignalSourceKind
- signal_type: MarketSignalType (required)
- unknown_count: integer

### ForwardCurveSliceEvidencePoint
- demo_status: MarketDemoStatus
- high_price_per_mt_usd: unknown
- layer: ForwardCurveEvidenceLayer (required)
- low_price_per_mt_usd: unknown
- observed_at: unknown
- price_per_mt_usd: unknown
- public_source_label: string (required)
- quantity_mt: unknown
- side: unknown
- source_kind: MarketSourceKind

### ForwardCurveSliceResponse
- cell: ForwardCurveMarketCell (required)
- depth_asks: ForwardCurveBoardDepthLevel[]
- depth_bids: ForwardCurveBoardDepthLevel[]
- disclaimer: string
- evidence_points: ForwardCurveSliceEvidencePoint[]
- fair_price_band: unknown
- generated_at: string(date-time) (required)
- indications: ForwardCurveBoardIndication[]
- next_window: unknown
- physical_stems: ForwardCurveBoardPhysicalStem[]
- previous_window: unknown
- trades: ForwardCurveSliceTrade[]

### ForwardCurveSliceTrade
- confirmed_at: string(date-time) (required)
- demo_status: MarketDemoStatus (required)
- price_per_mt_usd: string (required)
- quantity_mt: string (required)
- source_kind: MarketSourceKind (required)

### ForwardCurveTableCell
- best_ask: unknown
- best_bid: unknown
- demo_order_count: integer
- demo_status: MarketDemoStatus
- is_executable: boolean
- is_reference: boolean
- observed_at: unknown
- order_count: integer
- primary_signal_type: MarketSignalType
- primary_source_kind: MarketSourceKind
- primary_value: unknown
- public_source_label: string
- real_order_count: integer
- spread: unknown
- staleness_status: ForwardCurveStalenessStatus
- unknown_order_count: integer
- volume_mt: string

### ForwardCurveTableColumn
- availability_window: string (required)
- display_label: string (required)
- group: string (required)

### ForwardCurveTableResponse
- columns: ForwardCurveTableColumn[] (required)
- disclaimer: string
- generated_at: string(date-time) (required)
- latest_signals: ForwardCurveLatestSignal[]
- rows: ForwardCurveTableRow[] (required)

### ForwardCurveTableRow
- cells: object (required)
- delivery_point_id: string(uuid) (required, uuid)
- delivery_point_name: string (required)
- market_product: string (required)
- product_count: integer
- product_name: string (required)
- region: string (required)
- representative_product_id: string(uuid) (required, uuid)
- row_key: string (required)

### FrontendErrorPanel
- by_category: AggregateCell[]
- by_route_family: AggregateCell[]
- total: MetricValue (required)

### FuelEUResponse
- compliance_balance_gco2: string (required)
- estimated_penalty_eur: string (required)
- ghg_intensity_gco2_mj: string (required)
- reduction_pct: string (required)
- score: integer (required)
- target_intensity_gco2_mj: string (required)

### FunnelStage
- conversion_from_previous_pct: unknown
- name: string (required)
- value: integer (required)

### HTTPValidationError
- detail: ValidationError[]

### InventoryCreate
- carbon_intensity_gco2_mj: unknown
- carbon_intensity_method: unknown
- certification_declared: boolean
- certification_scheme: unknown
- current_stock_mt: unknown (required)
- energy_density_mj_kg: unknown
- fame_terms: unknown
- feedstock: unknown
- fuel_type: FuelType (required)
- incoming_stock_mt: unknown
- is_certified: boolean
- lng_terms: unknown
- msds_available: boolean
- off_spec: boolean
- off_spec_notes: unknown
- origin: unknown
- port_id: string (required)
- price_per_mt_usd: unknown
- product_name: unknown
- specification_standard: unknown

### InventoryItemUpdate
- carbon_intensity_gco2_mj: unknown
- carbon_intensity_method: unknown
- certification_declared: unknown
- certification_scheme: unknown
- current_stock_mt: unknown
- fame_terms: unknown
- feedstock: unknown
- incoming_stock_mt: unknown
- lng_terms: unknown
- msds_available: unknown
- off_spec: unknown
- off_spec_notes: unknown
- origin: unknown
- price_per_mt_usd: unknown
- product_name: unknown
- specification_standard: unknown

### InventoryResponse
- carbon_intensity_gco2_mj: unknown
- carbon_intensity_method: unknown
- certification_declared: boolean
- certification_scheme: unknown
- current_stock_mt: string (required)
- energy_density_mj_kg: unknown
- fame_terms: unknown
- feedstock: unknown
- fuel_type: FuelType (required)
- id: string(uuid) (required, uuid)
- incoming_stock_mt: string
- is_certified: boolean
- lng_terms: unknown
- msds_available: boolean
- off_spec: boolean
- off_spec_notes: unknown
- origin: unknown
- port_id: string (required)
- price_per_mt_usd: unknown
- product_name: unknown
- reserved_stock_mt: string (required)
- specification_standard: unknown
- supplier_id: string(uuid) (required, uuid)
- updated_at: string(date-time) (required)

### InvitationAcceptRequest
- accept_terms: boolean (required)
- new_password: string (required)
- token: string (required)

### InvitationOrganizationListResponse
- items: InvitationOrganizationResponse[] (required)

### InvitationOrganizationResponse
- domain: unknown (required)
- id: string(uuid) (required, uuid)
- name: string (required)
- provenance: OrganizationProvenance (required)
- type: string (required)

### InvitationResolveResponse
- email: string (required)
- expires_at: string(date-time) (required)
- first_name: unknown (required)
- invited_by_name: unknown (required)
- last_name: unknown (required)
- organization_name: string (required)
- role: string (required)

### InvitationTokenRequest
- token: string (required)

### JoinReviewBody
- review_note: string (required)

### JourneyStage
- buyer: unknown
- key: string (required)
- source: JourneySource (required)
- supplier: unknown
- total: AggregateCell (required)

### LabeledRatio
- key: string (required)
- kind: ConversionRatioKind (required)
- ratio: RatioValue (required)

### LeaderboardEntry
- organization_name: unknown
- rank: integer (required)
- referral_count: integer (required)
- user_name: string (required)

### LifecycleStage
- count: unknown
- coverage: AnalyticsSourceStatus (required)
- detail_tab: string (required)
- key: LifecycleStageKey (required)
- previous: unknown

### LiquiditySummary
- crossed_slices: unknown
- median_hours_to_first_fill: unknown
- median_open_order_age_hours: unknown
- median_spread_bps: unknown
- median_spread_usd_per_mt: unknown
- one_sided_slices: unknown
- slices: SliceLiquidityRow[]
- two_sided_slices: unknown

### ListingOverlay
- ci_basis: string (required)
- ci_gco2_mj: string (required)
- financial_benefit_status: string
- lcv_basis: string (required)
- lcv_mj_kg: string (required)
- penalty_avoided_eur_per_mt: string (required)
- penalty_avoided_usd_per_mt: string (required)
- tco2e_avoided_per_mt: string (required)

### LngAskTerms-Input
- bio_share_pct: unknown (required)
- chain_of_custody: string (required)
- delivery_method: string (required)
- feedstock: string (required)
- feedstock_detail: unknown
- min_methane_number: unknown
- physical_supplier_name: string (required)
- pos_timing: string (required)
- production_country: string (required)
- scheme_certificate_number: string (required)
- seller_is_physical_supplier: boolean (required)
- side: string (required)
- specification_standard: string (required)
- sustainability_scheme: string (required)
- union_database_registered: boolean (required)
- wtt_ghg_gco2e_mj: unknown (required)

### LngAskTerms-Output
- bio_share_pct: string (required)
- chain_of_custody: string (required)
- delivery_method: string (required)
- feedstock: string (required)
- feedstock_detail: unknown
- min_methane_number: unknown
- physical_supplier_name: string (required)
- pos_timing: string (required)
- production_country: string (required)
- scheme_certificate_number: string (required)
- seller_is_physical_supplier: boolean (required)
- side: string (required)
- specification_standard: string (required)
- sustainability_scheme: string (required)
- union_database_registered: boolean (required)
- wtt_ghg_gco2e_mj: string (required)

### LngBidTerms-Input
- accept_resellers: boolean
- accepted_custody: string[]
- accepted_delivery_methods: string[] (required)
- accepted_schemes: string[] (required)
- max_wtt_ghg_gco2e_mj: unknown
- min_bio_share_pct: unknown
- min_methane_number: unknown
- require_union_database: boolean
- side: string (required)

### LngBidTerms-Output
- accept_resellers: boolean
- accepted_custody: string[]
- accepted_delivery_methods: string[] (required)
- accepted_schemes: string[] (required)
- max_wtt_ghg_gco2e_mj: unknown
- min_bio_share_pct: unknown
- min_methane_number: unknown
- require_union_database: boolean
- side: string (required)

### LngPublicAskTerms
- bio_share_pct: string (required)
- chain_of_custody: string (required)
- delivery_method: string (required)
- feedstock: string (required)
- feedstock_detail: unknown
- fueleu_recognition: string (required)
- min_methane_number: unknown
- pos_timing: string (required)
- production_country: string (required)
- reseller: boolean (required)
- seller_is_physical_supplier: boolean (required)
- side: string (required)
- specification_standard: string (required)
- sustainability_scheme: string (required)
- union_database_registered: boolean (required)
- wtt_ghg_gco2e_mj: string (required)

### LngPublicTradeSnapshot
- ask: LngPublicAskTerms (required)
- bid: LngBidTerms-Output (required)

### LngTradeSnapshot
- ask: LngAskTerms-Output (required)
- bid: LngBidTerms-Output (required)

### LoginFailurePanel
- categories: AggregateCell[]
- total: MetricValue (required)
- trend: SeriesPoint[]

### LoginResponse
- access_token: string (required)
- profile: UserResponse (required)
- token_type: string (required)

### MapCompactMarketResponse
- ask_max_price: unknown
- ask_min_price: unknown
- ask_order_count: integer
- ask_total_quantity: string
- bid_max_price: unknown
- bid_min_price: unknown
- bid_order_count: integer
- bid_total_quantity: string
- delivery_point_id: string(uuid) (required, uuid)
- delivery_point_name: string (required)
- demo_status: MarketDemoStatus (required)
- evidence_class: string (required)
- fuel_type: string (required)
- market_product: string (required)
- observed_at: string(date-time) (required)
- product_id: string(uuid) (required, uuid)
- product_name: string (required)
- region: string (required)
- scope: MarketScope
- source_kind: MarketSourceKind (required)
- spot_best_ask: unknown
- spot_best_bid: unknown

### MapCompactSummaryResponse
- demo_groups: AggregatedOrderbookResponse[] (required)
- markets: MapCompactMarketResponse[] (required)
- recent_asks: MapRecentAskResponse[] (required)

### MapRecentAskResponse
- created_at: string(date-time) (required)
- delivery_point_id: string(uuid) (required, uuid)
- delivery_point_name: string (required)
- demo_status: MarketDemoStatus (required)
- evidence_class: string (required)
- fuel_type: string (required)
- market_product: string (required)
- price_per_mt_usd: string (required)
- product_id: string(uuid) (required, uuid)
- product_name: string (required)
- region: string (required)
- remaining_quantity_mt: string (required)
- scope: MarketScope
- source_kind: MarketSourceKind (required)

### MapSummaryResponse
- groups: AggregatedOrderbookResponse[] (required)
- recent_asks: MapRecentAskResponse[] (required)

### MarketActivitySection
- balance_trend: MarketBalanceTrend (required)
- concentration: OrganizationConcentration (required)
- kpis: MarketplaceKpis (required)
- liquidity: LiquiditySummary (required)
- order_status_distribution: AggregateCell[]
- product_port_matrix: ProductPortCell[]
- trade_status_distribution: AggregateCell[]
- window_distribution: RankedRow[]

### MarketBalanceTrend
- asks: SeriesPoint[]
- bids: SeriesPoint[]
- buyer_organizations: SeriesPoint[]
- supplier_organizations: SeriesPoint[]

### MarketSupportContextCreate
- confirm_replacement: boolean
- organization_id: string(uuid) (required, uuid)
- support_reference: string (required)

### MarketSupportContextResponse
- actor: MarketSupportPrincipal (required)
- actor_user_id: string(uuid) (required, uuid)
- ended_at: unknown (required)
- expires_at: string(date-time) (required)
- id: string(uuid) (required, uuid)
- organization: MarketSupportOrganization (required)
- organization_id: string(uuid) (required, uuid)
- scope: MarketSupportContextScope (required)
- started_at: string(date-time) (required)
- status: MarketSupportContextStatus (required)
- support_reference: string (required)
- version: integer (required)

### MarketSupportEntryResponse
- eligible: boolean
- organization: MarketSupportOrganization (required)
- reason: unknown

### MarketSupportFinalConfirmation
- acknowledge_exact_terms: boolean (required)
- acknowledge_executable_standing_order: boolean (required)
- evidence_excerpt: unknown
- external_instruction_reference: string (required)
- instruction_at: string(date-time) (required)

### MarketSupportOrganization
- domain: unknown (required)
- id: string(uuid) (required, uuid)
- name: string (required)
- type: string (required)

### MarketSupportPrincipal
- email: string (required)
- id: string(uuid) (required, uuid)
- name: string (required)

### MarketplaceBalance
- ask_orders: MetricValue (required)
- bid_orders: MetricValue (required)
- buyer_organizations: MetricValue (required)
- supplier_organizations: MetricValue (required)

### MarketplaceKpis
- confirmed_trades: MetricValue (required)
- confirmed_volume_mt: DecimalMetricValue (required)
- execution_rate: RatioValue (required)
- open_asks: MetricValue (required)
- open_bids: MetricValue (required)
- participating_organizations: MetricValue (required)

### MarketplaceResponse
- commercial: unknown
- demo: unknown
- live: unknown
- meta: AnalyticsMeta (required)
- reference: unknown
- unknown: unknown

### MetricEntry
- name: string (required)
- value: integer (required)

### MetricValue
- previous: unknown
- suppressed: boolean
- value: unknown

### NavigationDestinationRow
- buyer: unknown
- destination: NavigationDestination (required)
- supplier: unknown
- total: AggregateCell (required)

### NavigationLatencyRow
- buckets: AggregateCell[]
- destination: unknown (required)

### NeedsAttentionItem
- count: unknown
- rule: NeedsAttentionRule (required)

### NegotiationCounterRequest
- notes: unknown
- proposed_price: unknown (required)

### NegotiationCreateRequest
- ask_order_id: unknown
- availability_window: string
- bid_order_id: unknown
- certification_declared: boolean
- counterparty_org_id: string(uuid) (required, uuid)
- delivery_point_id: string(uuid) (required, uuid)
- expires_in_hours: integer
- fame_terms: unknown
- lng_terms: unknown
- msds_available: boolean
- notes: unknown
- product_id: string(uuid) (required, uuid)
- proposed_price: unknown (required)
- quantity_mt: unknown (required)

### NegotiationListResponse
- items: NegotiationResponse[] (required)
- total: integer (required)

### NegotiationResponse
- accepted_by_user_id: unknown
- ask_order_id: unknown
- availability_window: string
- bid_order_id: unknown
- counterparty_org_id: string(uuid) (required, uuid)
- counterparty_org_name: unknown
- counterparty_user_id: unknown
- created_at: string(date-time) (required)
- current_price: string (required)
- delivery_point_id: unknown
- expires_at: string(date-time) (required)
- fame_terms_snapshot: unknown
- id: string(uuid) (required, uuid)
- initiator_org_id: string(uuid) (required, uuid)
- initiator_org_name: unknown
- initiator_side: string (required)
- initiator_user_id: unknown
- last_actor_org_id: string(uuid) (required, uuid)
- lng_terms_snapshot: unknown
- product_id: string(uuid) (required, uuid)
- product_name: unknown
- quantity_mt: string (required)
- rounds: NegotiationRoundResponse[]
- status: string (required)
- trade_id: unknown
- updated_at: string(date-time) (required)

### NegotiationRoundResponse
- created_at: string(date-time) (required)
- id: string(uuid) (required, uuid)
- notes: unknown
- proposed_price: string (required)
- proposer_org_id: string(uuid) (required, uuid)
- proposer_org_name: unknown
- proposer_user_id: unknown
- round_number: integer (required)

### NotificationResponse
- created_at: string(date-time) (required)
- data: unknown (required)
- id: string(uuid4) (required)
- is_read: boolean (required)
- message: string (required)
- title: string (required)
- type: string (required)

### OrderBookSnapshotResponse
- asks: OrderResponse[]
- availability_window: string (required)
- bids: OrderResponse[]
- delivery_point_id: string(uuid) (required, uuid)
- demo_status: MarketDemoStatus (required)
- generated_at: string(date-time) (required)
- market_product: string (required)
- scope: MarketScope
- source_kind: MarketSourceKind (required)

### OrderCancelRequest
- reason: string (required)

### OrderCreate-Input
- availability_window: string
- carbon_intensity_gco2_mj: unknown
- carbon_intensity_method: unknown
- certification_declared: boolean
- certification_scheme: unknown
- certifications: string[]
- delivery_point_id: string(uuid) (required, uuid)
- expires_at: unknown
- fame_terms: unknown
- feedstock: unknown
- is_anonymous: boolean
- lng_terms: unknown
- msds_available: boolean
- off_spec: boolean
- off_spec_notes: unknown
- origin: unknown
- port_id: unknown
- price_per_mt_usd: unknown (required)
- product_id: string(uuid) (required, uuid)
- quantity_mt: unknown (required)
- side: OrderSide (required)
- specification_standard: unknown
- support_confirmation: unknown
- vessel_id: unknown

### OrderCreate-Output
- availability_window: string
- carbon_intensity_gco2_mj: unknown
- carbon_intensity_method: unknown
- certification_declared: boolean
- certification_scheme: unknown
- certifications: string[]
- delivery_point_id: string(uuid) (required, uuid)
- expires_at: unknown
- fame_terms: unknown
- feedstock: unknown
- is_anonymous: boolean
- lng_terms: unknown
- msds_available: boolean
- off_spec: boolean
- off_spec_notes: unknown
- origin: unknown
- port_id: unknown
- price_per_mt_usd: string (required)
- product_id: string(uuid) (required, uuid)
- quantity_mt: string (required)
- side: OrderSide (required)
- specification_standard: unknown
- support_confirmation: unknown
- vessel_id: unknown

### OrderMyResponse
- availability_window: string (required)
- benchmark_price_per_mt_usd: unknown
- benchmark_source: unknown
- carbon_intensity_gco2_mj: unknown
- carbon_intensity_method: unknown
- certification_declared: boolean
- certification_scheme: unknown
- certifications: string[]
- created_at: string(date-time) (required)
- creation_method: string
- delivery_point_id: unknown
- delivery_point_name: unknown
- demo_status: MarketDemoStatus
- etag: unknown
- expires_at: unknown
- fame_terms: unknown
- feedstock: unknown
- fuel_grade: string
- fuel_type: string
- id: string(uuid) (required, uuid)
- is_crossed: boolean
- is_demo_listing: boolean
- is_verdaxis_verified: boolean (required)
- lng_terms: unknown
- market_product: unknown
- msds_available: boolean
- off_spec: boolean
- off_spec_notes: unknown
- organization_id: string(uuid) (required, uuid)
- origin: unknown
- port_id: unknown
- premium_discount_per_mt_usd: unknown
- price_per_mt_usd: string (required)
- product_id: string(uuid) (required, uuid)
- product_name: string
- quantity_mt: string (required)
- region: string
- remaining_quantity_mt: string (required)
- scope: MarketScope
- side: OrderSide (required)
- source_kind: MarketSourceKind
- specification_standard: unknown
- status: OrderBookStatus (required)
- terms_digest: string (required)
- tier_label: TierLabel
- trade_count: integer
- unknown_count: integer
- updated_at: string(date-time) (required)
- version: integer
- vessel_id: unknown

### OrderResponse
- availability_window: string (required)
- benchmark_price_per_mt_usd: unknown
- benchmark_source: unknown
- carbon_intensity_gco2_mj: unknown
- carbon_intensity_method: unknown
- certification_declared: boolean
- certification_scheme: unknown
- certifications: string[]
- created_at: string(date-time) (required)
- delivery_point_id: unknown
- delivery_point_name: unknown
- demo_status: MarketDemoStatus
- expires_at: unknown
- fame_terms: unknown
- feedstock: unknown
- fuel_grade: string
- fuel_type: string
- id: string(uuid) (required, uuid)
- is_crossed: boolean
- is_demo_listing: boolean
- is_verdaxis_verified: boolean (required)
- lng_terms: unknown
- market_product: unknown
- msds_available: boolean
- off_spec: boolean
- off_spec_notes: unknown
- origin: unknown
- port_id: unknown
- premium_discount_per_mt_usd: unknown
- price_per_mt_usd: string (required)
- product_id: string(uuid) (required, uuid)
- product_name: string
- quantity_mt: string (required)
- region: string
- remaining_quantity_mt: string (required)
- scope: MarketScope
- side: OrderSide (required)
- source_kind: MarketSourceKind
- specification_standard: unknown
- status: OrderBookStatus (required)
- terms_digest: string (required)
- tier_label: TierLabel
- unknown_count: integer
- version: integer

### OrderResponseWithCI
- availability_window: string (required)
- benchmark_price_per_mt_usd: unknown
- benchmark_source: unknown
- carbon_intensity_gco2_mj: unknown
- carbon_intensity_method: unknown
- certification_declared: boolean
- certification_scheme: unknown
- certifications: string[]
- ci_adjusted_price: unknown
- created_at: string(date-time) (required)
- delivery_point_id: unknown
- delivery_point_name: unknown
- demo_status: MarketDemoStatus
- expires_at: unknown
- fame_terms: unknown
- feedstock: unknown
- fuel_grade: string
- fuel_type: string
- id: string(uuid) (required, uuid)
- is_crossed: boolean
- is_demo_listing: boolean
- is_verdaxis_verified: boolean (required)
- lng_terms: unknown
- market_product: unknown
- msds_available: boolean
- off_spec: boolean
- off_spec_notes: unknown
- origin: unknown
- port_id: unknown
- premium_discount_per_mt_usd: unknown
- price_per_mt_usd: string (required)
- product_id: string(uuid) (required, uuid)
- product_name: string
- quantity_mt: string (required)
- region: string
- remaining_quantity_mt: string (required)
- scope: MarketScope
- side: OrderSide (required)
- source_kind: MarketSourceKind
- specification_standard: unknown
- status: OrderBookStatus (required)
- terms_digest: string (required)
- tier_label: TierLabel
- unknown_count: integer
- version: integer

### OrderUpdate
- availability_window: unknown
- carbon_intensity_gco2_mj: unknown
- carbon_intensity_method: unknown
- certification_declared: unknown
- certification_scheme: unknown
- certifications: unknown
- expires_at: unknown
- fame_terms: unknown
- feedstock: unknown
- lng_terms: unknown
- msds_available: unknown
- off_spec: unknown
- off_spec_notes: unknown
- origin: unknown
- price_per_mt_usd: unknown
- quantity_mt: unknown
- specification_standard: unknown

### OrganizationConcentration
- hhi_band: unknown
- suppressed: boolean

### OrganizationCreate
- country_code: unknown
- name: string (required)
- tax_id: unknown
- type: OrgType (required)

### OverlayAssumptions
- eur_usd_rate: string (required)
- excluded_factors: string[] (required)
- fleet_intensity_basis: string (required)
- fleet_vessel_count: integer (required)
- ghgie_actual_gco2_mj: string (required)
- penalty_eur_per_tonne: string (required)
- vlsfo_baseline_gco2_mj: string (required)
- year: integer (required)
- year_target: string (required)

### OverviewKpis
- active_members: MetricValue (required)
- confirmed_trades: MetricValue (required)
- live_orders: MetricValue (required)
- participating_organizations: MetricValue (required)
- qualified_organizations: MetricValue (required)

### PaginatedResponse_AssistedListingResponse_
- items: AssistedListingResponse[] (required)
- limit: integer (required)
- skip: integer (required)
- total: integer (required)

### PaginatedResponse_AuthorizationResponse_
- items: AuthorizationResponse[] (required)
- limit: integer (required)
- skip: integer (required)
- total: integer (required)

### PaginatedResponse_MarketSupportOrganization_
- items: MarketSupportOrganization[] (required)
- limit: integer (required)
- skip: integer (required)
- total: integer (required)

### PaginatedResponse_OrderResponse_
- items: OrderResponse[] (required)
- limit: integer (required)
- skip: integer (required)
- total: integer (required)

### PaginatedResponse_TradeResponse_
- items: TradeResponse[] (required)
- limit: integer (required)
- skip: integer (required)
- total: integer (required)

### PasswordChangeRequest
- current_password: string (required)
- new_password: string (required)

### PinTargetCreate
- order_id: string(uuid) (required, uuid)
- target_type: string (required)

### PortFuelAvailability
- availability_level: AvailabilityLevel (required)
- avg_price_per_mt: unknown
- demo_status: MarketDemoStatus
- fuel_type: string (required)
- lat: number (required)
- lng: number (required)
- market_product_code: string (required)
- port_id: string (required)
- port_name: string (required)
- scope: MarketScope
- source_kind: MarketSourceKind
- supplier_count: integer (required)
- total_stock_mt: unknown (required)
- unknown_count: integer

### PortIntelligenceBase
- biofuel_price_avg: unknown
- captured_at: string(date-time) (required)
- congestion_level: unknown
- methanol_price_avg: unknown

### PortResponse
- country: string (required)
- id: string (required)
- intelligence: unknown
- is_active: boolean
- lat: unknown
- lng: unknown
- location: unknown
- name: string (required)
- timezone: unknown

### PriceDiscoveryResponse
- generated_at: string(date-time) (required)
- summaries: PriceSummary[] (required)

### PriceSummary
- availability_window: string
- avg_price_24h: unknown
- delivery_point_id: unknown
- delivery_point_name: unknown
- demo_status: MarketDemoStatus
- demo_trade_count_24h: integer
- fuel_type: string
- high_24h: unknown
- is_reference: boolean
- last_price: unknown
- last_trade_at: unknown
- low_24h: unknown
- market_product: unknown
- observed_at: unknown
- price_change_pct: unknown
- product_id: unknown
- product_name: string
- real_trade_count_24h: integer
- region: string
- scope: MarketScope
- source_kind: MarketSourceKind
- trade_count_24h: integer
- unknown_trade_count_24h: integer
- volume_24h: string

### PricingOverlayRequest
- order_ids: string[] (required)
- year: integer

### PricingOverlayResponse
- assumptions: OverlayAssumptions (required)
- overlays: object (required)

### ProducerProjectResponse
- capacity_kt_per_year: unknown
- carbon_intensity_gco2_mj: unknown
- cod_date: unknown
- cod_year: unknown
- country: string (required)
- created_at: string(date-time) (required)
- data_source: unknown
- feedstock: unknown
- fuel_type: string (required)
- gena_project_id: unknown
- id: string(uuid) (required, uuid)
- lat: unknown
- lng: unknown
- name: string (required)
- notes: unknown
- organization_id: unknown
- region: unknown
- status: string (required)
- technology: unknown

### ProductCountResponse
- counts: object (required)
- total: integer (required)

### ProductPortCell
- delivery_point_key: string (required)
- delivery_point_label: string (required)
- orders: AggregateCell (required)
- organizations: AggregateCell (required)
- product_key: string (required)
- product_label: string (required)

### ProductResponse
- available_delivery_point_ids: string[] (required)
- execution_mode: string (required)
- fuel_grade: string (required)
- fuel_type: string (required)
- id: string(uuid) (required, uuid)
- is_active: boolean (required)
- market_product: unknown
- min_lot_size: string (required)
- name: string (required)
- spec_description: unknown
- unit: string (required)

### ProductUsageResponse
- authoritative: AuthoritativeUsage (required)
- behavioral: BehavioralUsage (required)
- behavioral_status: string (required)
- days: integer (required)
- diagnostic: unknown (required)
- funnel: FunnelStage[] (required)
- observed_at: string(date-time) (required)
- period_end: string(date-time) (required)
- period_start: string(date-time) (required)

### RFQCreateRequest
- availability_window: string
- contract_terms: unknown
- delivery_point_id: string(uuid) (required, uuid)
- expected_source_offer_revision: unknown
- expires_in_hours: integer
- is_anonymous: boolean
- notes: unknown
- product_id: string(uuid) (required, uuid)
- quantity_mt: unknown (required)
- source_offer_id: unknown
- target_price_per_mt: unknown

### RFQListResponse
- items: RFQResponse[] (required)
- total: integer (required)

### RFQQuoteRequest
- expires_at: unknown
- notes: unknown
- offer_terms: unknown
- price_per_mt_usd: unknown (required)

### RFQQuoteResponse
- created_at: string(date-time) (required)
- execution_enabled: boolean
- expires_at: unknown
- id: string(uuid) (required, uuid)
- is_expired: boolean
- notes: unknown
- offer_terms: unknown
- price_per_mt_usd: string (required)
- revision: integer
- seller_org_id: string(uuid) (required, uuid)
- seller_org_name: unknown
- status: string (required)

### RFQQuoteRevisionRequest
- expected_revision: integer (required)
- expires_at: unknown
- notes: unknown
- offer_terms: unknown
- price_per_mt_usd: unknown (required)

### RFQResponse
- availability_window: string (required)
- buyer_org_id: unknown
- buyer_org_name: unknown
- can_cancel: boolean
- contract_terms: unknown
- created_at: string(date-time) (required)
- delivery_point_id: unknown
- delivery_point_name: unknown
- execution_enabled: boolean
- expires_at: string(date-time) (required)
- id: string(uuid) (required, uuid)
- is_anonymous: boolean (required)
- notes: unknown
- product_id: string(uuid) (required, uuid)
- product_name: unknown
- quantity_mt: string (required)
- quote_count: integer
- quotes: RFQQuoteResponse[]
- source_offer_id: unknown
- source_offer_snapshot: unknown
- status: string (required)
- target_price_per_mt: unknown
- target_supplier_org_id: unknown

### RankedRow
- count: unknown
- key: string (required)
- label: string (required)
- share_pct: unknown
- suppressed: boolean

### RatioValue
- cohort_complete: boolean
- denominator: unknown
- numerator: unknown
- rate_pct: unknown
- suppressed: boolean

### ReferenceCoverageRow
- availability_window: string (required)
- availability_window_label: string (required)
- benchmark_price_usd_per_mt: unknown
- coverage_status: string (required)
- delivery_point_key: string (required)
- delivery_point_label: string (required)
- generated_at: string(date-time) (required)
- observed_at: unknown
- product_key: string (required)
- product_label: string (required)
- scope: ReferenceScope (required)
- source_kind: ReferenceSourceKind (required)
- source_label: ReferenceSourceLabel (required)

### ReferenceCoverageSection
- rows: ReferenceCoverageRow[]

### ReferencePriceItem
- availability_window: string
- date: string(date) (required)
- delivery_point_id: unknown
- delivery_point_name: unknown
- demo_status: MarketDemoStatus
- fuel_type: string
- is_reference: boolean
- market_product: unknown
- product_id: unknown
- product_name: string
- region: string
- scope: MarketScope
- source_kind: MarketSourceKind
- total_volume_mt: string (required)
- trade_count: integer (required)
- visibility: string
- vwap_usd: string (required)

### ReferencePriceResponse
- generated_at: string(date-time) (required)
- prices: ReferencePriceItem[] (required)

### ReferralCodeResponse
- referral_code: string (required)
- referral_link: string (required)

### ReferralInviteRequest
- email: string(email) (required)

### ReferralListItem
- organization_name: unknown
- role: unknown
- signed_up_at: string(date-time) (required)
- status: string (required)

### ReferralStatsResponse
- active: integer (required)
- referrals: ReferralListItem[] (required)
- total: integer (required)
- verified: integer (required)

### RefreshRequest
- refresh_token: unknown

### RegisterWithOrgRequest
- organization: OrganizationCreate (required)
- registration_token: string (required)

### RegistrationResponse
- registration_token: unknown
- status: string (required)
- user: unknown

### ReliabilityResponse
- audit_activity: AuditActivityRow[]
- backend_unavailable: BackendUnavailablePanel (required)
- collector: CollectorState (required)
- frontend_errors: FrontendErrorPanel (required)
- login_failures: LoginFailurePanel (required)
- meta: AnalyticsMeta (required)
- navigation_latency: NavigationLatencyRow[]

### ResendVerificationRequest
- email: string (required)

### ResetPasswordRequest
- new_password: string (required)
- token: string (required)

### ResolveCodeResponse
- organization_name: unknown
- organization_type: unknown
- referrer_name: unknown
- valid: boolean (required)

### RetentionKpis
- dormant_approved_members: MetricValue (required)
- reactivated_organizations: MetricValue (required)
- retained_organizations: MetricValue (required)
- returning_members: MetricValue (required)

### RetentionResponse
- kpis: RetentionKpis (required)
- member_cohorts: CohortRow[]
- meta: AnalyticsMeta (required)
- organization_cohorts: CohortRow[]
- repeat_participation: AggregateCell[]

### ScenarioInput
- fuel_mix: object (required)
- vessel_id: string (required)
- year: integer

### SeriesPoint
- date: string(date) (required)
- value: unknown

### SliceLiquidityRow
- availability_window: string (required)
- availability_window_label: string (required)
- best_ask_depth_mt: unknown
- best_ask_usd_per_mt: unknown
- best_bid_depth_mt: unknown
- best_bid_usd_per_mt: unknown
- contributing_organizations: unknown
- crossed: unknown
- delivery_point_key: string (required)
- delivery_point_label: string (required)
- one_percent_ask_depth_mt: unknown
- one_percent_bid_depth_mt: unknown
- product_key: string (required)
- product_label: string (required)
- spread_bps: unknown
- spread_usd_per_mt: unknown
- suppressed: boolean

### SliceTargetCreate
- availability_window_code: string (required)
- delivery_point_id: string(uuid) (required, uuid)
- market_product_code: string (required)
- target_type: string (required)

### SubscriptionResponse
- expires_at: unknown
- id: string(uuid) (required, uuid)
- is_active: boolean (required)
- org_id: string(uuid) (required, uuid)
- seller_fee_per_mt_usd: unknown
- started_at: unknown
- tier: SubscriptionTier (required)

### SubscriptionUpdate
- seller_fee_per_mt_usd: unknown
- tier: SubscriptionTier (required)

### SupplierListingTemplateResponse
- availability_window: string (required)
- carbon_intensity_gco2_mj: unknown
- carbon_intensity_method: unknown
- certification_declared: boolean
- certification_scheme: unknown
- certifications: string[]
- delivery_point_id: string(uuid) (required, uuid)
- fame_terms: unknown
- feedstock: unknown
- lng_terms: unknown
- msds_available: boolean
- off_spec: boolean
- off_spec_notes: unknown
- origin: unknown
- price_per_mt_usd: string (required)
- product_id: string(uuid) (required, uuid)
- quantity_mt: string (required)
- specification_standard: unknown

### SupplierOfferCreate
- availability_window: string
- claims_terms: unknown
- delivery_basis: string (required)
- delivery_end: string(date) (required)
- delivery_point_id: string(uuid) (required, uuid)
- delivery_start: string(date) (required)
- evidence_due: string (required)
- expires_at: string(date-time) (required)
- fuel_terms: FameListingFuelTerms-Input (required)
- inspection_terms: unknown
- min_fill_mt: unknown (required)
- named_location: string (required)
- notes: unknown
- payment_terms: unknown
- price_per_mt_usd: unknown (required)
- product_id: string(uuid) (required, uuid)
- quantity_mt: unknown (required)
- quantity_tolerance_pct: unknown (required)
- title_risk_terms: unknown

### SupplierOfferList
- items: SupplierOfferResponse[] (required)
- limit: integer (required)
- skip: integer (required)
- total: integer (required)

### SupplierOfferPublicSnapshot
- availability_window: string (required)
- delivery_point_id: string(uuid) (required, uuid)
- expires_at: string(date-time) (required)
- listing_terms: SupplierOfferPublicTerms (required)
- min_fill_mt: string (required)
- offer_id: string(uuid) (required, uuid)
- price_per_mt_usd: string (required)
- product_id: string(uuid) (required, uuid)
- quantity_mt: string (required)
- revision: integer (required)

### SupplierOfferPublicTerms
- claims_terms: unknown
- delivery_basis: string (required)
- delivery_end: string(date) (required)
- delivery_start: string(date) (required)
- evidence_due: string (required)
- fuel_terms: FameFuelSummary (required)
- inspection_terms: unknown
- named_location: string (required)
- notes: unknown
- payment_terms: unknown
- quantity_tolerance_pct: string (required)
- title_risk_terms: unknown

### SupplierOfferResponse
- availability_window: string (required)
- can_edit: boolean
- can_request_quote: boolean
- can_withdraw: boolean
- claims_terms: unknown
- created_at: string(date-time) (required)
- delivery_basis: string (required)
- delivery_end: string(date) (required)
- delivery_point_id: string(uuid) (required, uuid)
- delivery_point_name: string
- delivery_start: string(date) (required)
- evidence_due: string (required)
- execution_enabled: boolean
- expires_at: string(date-time) (required)
- fuel_terms: unknown (required)
- id: string(uuid) (required, uuid)
- inspection_terms: unknown
- listing_kind: string
- market_product: string
- min_fill_mt: string (required)
- named_location: string (required)
- notes: unknown
- payment_terms: unknown
- price_per_mt_usd: string (required)
- product_id: string(uuid) (required, uuid)
- product_name: string
- quantity_mt: string (required)
- quantity_tolerance_pct: string (required)
- revision: integer (required)
- status: string (required)
- title_risk_terms: unknown
- updated_at: string(date-time) (required)

### SupplierOfferSnapshot
- availability_window: string (required)
- delivery_point_id: string(uuid) (required, uuid)
- expires_at: string(date-time) (required)
- listing_terms: SupplierOfferTerms (required)
- min_fill_mt: string (required)
- offer_id: string(uuid) (required, uuid)
- price_per_mt_usd: string (required)
- product_id: string(uuid) (required, uuid)
- quantity_mt: string (required)
- revision: integer (required)
- supplier_org_id: string(uuid) (required, uuid)

### SupplierOfferTerms
- claims_terms: unknown
- delivery_basis: string (required)
- delivery_end: string(date) (required)
- delivery_start: string(date) (required)
- evidence_due: string (required)
- fuel_terms: FameListingFuelTerms-Output (required)
- inspection_terms: unknown
- named_location: string (required)
- notes: unknown
- payment_terms: unknown
- quantity_tolerance_pct: string (required)
- title_risk_terms: unknown

### SupplierOfferUpdate
- availability_window: string
- claims_terms: unknown
- delivery_basis: string (required)
- delivery_end: string(date) (required)
- delivery_point_id: string(uuid) (required, uuid)
- delivery_start: string(date) (required)
- evidence_due: string (required)
- expected_revision: integer (required)
- expires_at: string(date-time) (required)
- fuel_terms: FameListingFuelTerms-Input (required)
- inspection_terms: unknown
- min_fill_mt: unknown (required)
- named_location: string (required)
- notes: unknown
- payment_terms: unknown
- price_per_mt_usd: unknown (required)
- product_id: string(uuid) (required, uuid)
- quantity_mt: unknown (required)
- quantity_tolerance_pct: unknown (required)
- title_risk_terms: unknown

### SupplierOfferWithdraw
- expected_revision: integer (required)

### SurveySubmission
- referral_source: unknown
- use_case: string (required)

### TradeCreate
- certification_declared: boolean
- expected_order_version: unknown
- expected_terms_digest: unknown
- fame_terms: unknown
- lng_terms: unknown
- msds_available: boolean
- order_id: string(uuid) (required, uuid)
- quantity_mt: unknown (required)

### TradeDeliverPayload
- final_price_per_mt: unknown (required)
- final_quantity_mt: unknown (required)

### TradeResponse
- ask_order_id: unknown
- availability_window: unknown
- bid_order_id: unknown
- buyer_id: unknown
- buyer_name: string
- commission_amount_usd: unknown
- commission_fee_per_mt_usd: unknown
- commission_payer: unknown
- commission_plan: unknown
- commission_rate_pct: string
- confirmed_at: unknown
- created_at: string(date-time) (required)
- delivered_at: unknown
- delivery_point_id: unknown
- delivery_point_name: unknown
- demo_status: MarketDemoStatus
- fame_terms_snapshot: unknown
- final_price_per_mt: unknown
- final_quantity_mt: unknown
- final_total_usd: unknown
- fuel_grade: string
- fuel_type: string
- id: string(uuid) (required, uuid)
- initiated_by: Initiator (required)
- is_anonymous: boolean
- lng_terms_snapshot: unknown
- market_product: unknown
- paid_at: unknown
- price_per_mt_usd: string (required)
- product_id: unknown
- product_name: string
- quantity_mt: string (required)
- region: string
- scope: MarketScope
- seller_id: unknown
- seller_name: string
- source_kind: MarketSourceKind
- status: TradeStatus (required)
- unknown_count: integer

### TradeSummaryResponse
- action_required_count: integer (required)
- awaiting_counterparty_count: integer (required)
- confirmed_count: integer (required)
- total_count: integer (required)

### TradeTapeEntry
- availability_window: string (required)
- confirmed_at: string(date-time) (required)
- delivery_point_id: unknown
- delivery_point_name: unknown
- demo_status: MarketDemoStatus
- fuel_grade: string (required)
- fuel_type: string (required)
- id: string (required)
- is_demo_trade: boolean
- market_product: unknown
- price_per_mt_usd: string (required)
- product_id: unknown
- provenance_kind: string
- quantity_mt: string (required)
- region: string (required)
- scope: string
- source_kind: MarketSourceKind
- total_usd: string (required)

### TradeTapeResponse
- items: TradeTapeEntry[] (required)
- market_hours: boolean (required)
- total: integer (required)

### TutorialStepRow
- completed: AggregateCell (required)
- skipped: AggregateCell (required)
- step: string (required)

### UserCreate
- email: string(email) (required)
- first_name: unknown
- last_name: unknown
- organization_id: unknown
- password: string (required)
- referral_code: unknown
- role: UserRole (required)

### UserResponse
- email: string (required)
- execution_eligibility: ExecutionEligibilityResponse
- first_name: unknown
- id: string(uuid) (required, uuid)
- kyc_status: string
- last_name: unknown
- must_change_password: boolean
- organization_id: unknown
- referral_code: unknown
- role: unknown
- status: UserStatus (required)

### UserUpdate
- first_name: unknown
- last_name: unknown
- role: unknown

### ValidationError
- ctx: object
- input: unknown
- loc: any[] (required)
- msg: string (required)
- type: string (required)

### VesselResponse
- cii_rating: unknown
- current_location: unknown
- dwt: unknown
- eu_ets_status: unknown
- flag_state: unknown
- fueleu_status: unknown
- id: string (required)
- imo_number: string (required)
- lat: unknown
- lng: unknown
- name: string (required)
- organization_id: unknown
- prev_lat: unknown
- prev_lng: unknown
- previous_location: unknown
- updated_at: string(date-time) (required)
- vessel_type: unknown

### WatchlistCreateRequest
- name: string (required)

### WatchlistDetailResponse
- created_at: string(date-time) (required)
- has_more_slices: boolean
- id: string(uuid) (required, uuid)
- kind: string (required)
- latest_event_at: unknown
- name: string (required)
- slices: WatchlistSliceResponse[]
- total_slice_count: integer
- unread_event_count: integer

### WatchlistEntryAddRequest
- delivery_point_id: unknown
- product_id: string(uuid) (required, uuid)

### WatchlistEntryResponse
- best_ask: unknown
- best_bid: unknown
- created_at: string(date-time) (required)
- delivery_point_id: unknown
- delivery_point_name: unknown
- id: string(uuid) (required, uuid)
- product_id: string(uuid) (required, uuid)
- product_name: unknown

### WatchlistEventResponse
- availability_window_code: unknown
- created_at: string(date-time) (required)
- delivery_point_id: unknown
- delivery_point_name: unknown
- demo_status: MarketDemoStatus
- event_payload: object
- event_type: string (required)
- id: string(uuid) (required, uuid)
- is_read: boolean (required)
- market_product_code: unknown
- observed_at: unknown
- order_id: unknown
- scope: MarketScope
- source_kind: MarketSourceKind
- target_type: string (required)
- watchlist_id: string(uuid) (required, uuid)
- watchlist_target_id: string(uuid) (required, uuid)

### WatchlistEventsPageResponse
- items: WatchlistEventResponse[]
- next_cursor: unknown

### WatchlistResponse
- created_at: string(date-time) (required)
- entries: WatchlistEntryResponse[]
- entry_count: integer
- id: string(uuid) (required, uuid)
- name: string (required)

### WatchlistSliceResponse
- active_order_count: integer
- availability_window_code: string (required)
- created_at: string(date-time) (required)
- delivery_point_id: string(uuid) (required, uuid)
- delivery_point_name: unknown
- id: string(uuid) (required, uuid)
- latest_event_at: unknown
- market_product_code: string (required)
- pins: WatchlistTargetResponse[]
- target_type: string
- unread_event_count: integer

### WatchlistSummaryResponse
- created_at: string(date-time) (required)
- has_more_slices: boolean
- id: string(uuid) (required, uuid)
- kind: string (required)
- latest_event_at: unknown
- name: string (required)
- slices: WatchlistSliceResponse[]
- total_slice_count: integer
- unread_event_count: integer

### WatchlistTargetResponse
- active_order_count: integer
- availability_window_code: unknown
- created_at: string(date-time) (required)
- delivery_point_id: unknown
- delivery_point_name: unknown
- id: string(uuid) (required, uuid)
- latest_event_at: unknown
- market_product_code: unknown
- order_id: unknown
- snapshot_availability_window: unknown
- snapshot_counterparty_label: unknown
- snapshot_delivery_point_name: unknown
- snapshot_market_product: unknown
- snapshot_price_per_mt_usd: unknown
- snapshot_quantity_mt: unknown
- snapshot_remaining_quantity_mt: unknown
- snapshot_side: unknown
- snapshot_status: unknown
- target_type: string (required)
- unread_event_count: integer

### app__routers__admin_analytics__OverviewResponse
- active_users_7d: integer (required)
- confirmed_trades: integer (required)
- open_orders: integer (required)
- total_gmv_usd: number (required)
- total_orders: integer (required)
- total_organizations: integer (required)
- total_revenue_usd: number (required)
- total_trades: integer (required)
- total_users: integer (required)
- total_volume_mt: number (required)

### app__schemas__product_analytics__OverviewResponse
- activity_trend: ActivityTrend (required)
- kpis: OverviewKpis (required)
- lifecycle: LifecycleStage[]
- marketplace_balance: MarketplaceBalance (required)
- meta: AnalyticsMeta (required)
- needs_attention: NeedsAttentionItem[]

---

# Libraries

- `alembic/env.py`
  - function run_migrations_offline: () -> None
  - function do_run_migrations: (connection) -> None
  - function configure_migrations: (connection, *, bootstrap) -> None
  - function run_migrations_online: () -> None
  - function run_async_migrations: () -> None
- `alembic/versions/148fa1ffb191_add_notifications_table.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/14b159f248ac_rename_quote_request_to_direct_order.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/377c3f6f9aef_initial_schema.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/411b38418aea_add_user_interaction_status.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/63cbf0775d4d_add_domain_to_organization.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/a1b2c3d4e5f6_unified_orderbook_migration.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ae_20260811_account_approval_email.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ai_20260831_admin_invite_real_orgs.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/alerts_2026_03_add_price_alerts.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/anon_trade_2026_03_add_trade_is_anonymous.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/auth_20261002_session_cutoff.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/auth_2026_07_add_must_change_password.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/b1c2d3e4f5g6_add_ci_fields_to_orderbook.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/blng_20261014_staging.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/bm_2026_04_benchmarks.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/c2d3e4f5g6h7_add_match_suggestions.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/c315709743c2_move_supplier_tier_to_organization.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/c31aa85e9739_make_user_role_nullable.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/c5e4af769a3a_add_requested_quantity_and_delivery_.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/c820b25c0e18_update_supplier_tier_values_to_match_.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/c8a8c986fb8c_add_quoteoffer_model.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/catalog_2026_03_add_product_and_delivery_point.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/catalog_2026_04_green_fuels_market_products.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ccef95a9c9a0_add_rfq_models.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/contracts_2026_03_add_contract_supply_demand.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/d3e4f5g6h7i8_add_producer_projects.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/e3b25629c526_make_supplier_tier_nullable_and_cleanup_.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/e4f5g6h7i8j9_widen_notification_type_column.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/f10cf6fa2019_rename_rfq_to_orders.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/f5g6h7i8j9k0_add_password_changed_at.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/fame_20260922_b100_orderbook.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/fame_20260922_catalog.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/fame_20260922_rfq_contract.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/fame_20260922_supplier_offers.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/fb_20260804_add_feedback_entries.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/fc_2026_06_monitor_signals.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/fee_20260912_seller_per_mt.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/fk_orderbook_2026_03_orderbook_product_dp_fks.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/g6h7i8j9k0l1_add_audit_logs_table.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/h7i8j9k0l1m2_add_email_verification_and_kyc_fields.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ix_2026_04_exec_watchlist_perf.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/kycd_20261015_staging.py` — function upgrade: (), function downgrade: ()
- `alembic/versions/kycg_20261014_staging.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/mi_20260720_market_integrity.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/miq_20260720_market_quarantine.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ms_20260723_assisted_listings.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ms_20260723_assisted_order_v2.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ms_20260723_organization_context.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/neg_2026_04_add_negotiations.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/neg_2026_04b_negotiation_fixes.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/news_2026_03_add_news_items.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/oa_20260910_auto_real_orgs.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ob_2026_04_availability_windows.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ob_2026_04_supplier_listing_metadata.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/obp_20261003_acceptance_priority.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/p0_20261016_union.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/pa_20260715_add_product_analytics_facts.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/prc_20261018_cache.py` — function upgrade: (), function downgrade: ()
- `alembic/versions/pref_20260709_add_user_preferences.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/pw_reset_2026_03_add_password_reset_fields.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/rcp_20261003_command_results.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ref_2026_03_add_referrals.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/rfq_2026_03_add_rfq_tables.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/rh_20260720_runtime_metadata.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/sb_2026_04_live_slice_benchmarks.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/sec_20260720_admission_boundaries.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/sec_20260720_device_sessions.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/sec_20260720_fresh_remediation.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/sec_20260720_identity_hardening.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/sse_20260720_market_event_stream.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/sub_2026_03_add_subscriptions.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/usr_2026_04_onboarding_survey.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/wl_2026_03_add_watchlists.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/wl_2026_04_market_radar.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/wl_2026_04_watchlist_entry_uniqueness.py` — function upgrade: () -> None, function downgrade: () -> None
- `deploy/external_monitor/verdaxis_autodiag.py`
  - function log: (message) -> None
  - function redact: (value, limit) -> str
  - function read_json: (path) -> dict[str, Any]
  - function write_json: (path, payload, Any]) -> None
  - function sync_codex_auth: () -> None
  - function run_command: (command, timeout) -> dict[str, Any]
  - _...11 more_
- `deploy/external_monitor/verdaxis_monitor.py`
  - function run: (cmd, timeout, env, str] | None) -> subprocess.CompletedProcess[str]
  - function log: (message) -> None
  - function load_state: (path) -> dict
  - function save_state: (path, state) -> None
  - function load_cached_expensive_checks: (now) -> tuple[list[str], dict] | None
  - function check_caddyfile: () -> list[str]
  - _...35 more_
- `deploy/external_monitor/verdaxis_recover.py`
  - function log: (message) -> None
  - function redact: (value, limit) -> str
  - function read_json: (path) -> dict[str, Any]
  - function write_json: (path, payload, Any]) -> None
  - function failure_fingerprint: (status, Any]) -> str
  - function requested_targets: (status, Any]) -> list[str]
  - _...17 more_
- `deploy/monitor/alert_dispatch.py`
  - function valid_alert_state: (payload, now) -> bool
  - function process_event: (event, state_directory, now, environ, str]) -> str
  - function process_reminder: (check, state_directory, now, environ, str]) -> str
  - function parse_args: (argv) -> argparse.Namespace
  - function main: (argv) -> int
- `deploy/monitor/backup_verify.py`
  - function verify_backups: (status_file, backup_directory, attempt_directory, *, now, max_age_seconds, future_tolerance_seconds, max_timestamp_skew_seconds, max_metadata_bytes, max_attempt_records, max_attempt_bytes, max_artifacts, max_artifact_bytes, max_uncompressed_bytes, total_timeout_seconds) -> list[dict]
  - function run_monitor: (args) -> dict
  - function parse_args: (argv) -> argparse.Namespace
  - function main: (argv) -> int
- `deploy/monitor/legacy_retirement.py`
  - function configured_receipt_matrix: (path) -> frozenset[tuple[str, str]]
  - function validate_evidence: (payload, *, required_receipts, str]], now) -> dict
  - function validate_evidence_file: (path, *, required_receipts, str]], now) -> dict
  - function guarded_retirement: (evidence_path, *, alert_config_path, alert_state_directory, health_status_path, backup_status_path, execute, confirmation, runner, None], timer_checker, bool], now, clock, datetime]) -> str
  - function parse_args: (argv) -> argparse.Namespace
  - function main: (argv) -> int
  - _...1 more_
- `deploy/monitor/local_health_check.py`
  - function validate_runtime_identities: (identities, tuple[str | None, str | None]]) -> None
  - function load_runtime_identities: (production_file, staging_file) -> dict[str, tuple[str, str]]
  - function validate_readiness_payload: (payload, expected_environment, expected_release_sha) -> None
  - function check_readiness: (name, url, expected_environment, expected_release_sha, timeout_seconds, max_body_bytes) -> dict
  - function check_filesystem: (name, path, warning_free_percent, critical_free_percent, *, disk_usage) -> dict
  - function check_directory_size: (name, path, max_bytes, deadline, *, max_entries) -> dict
  - _...4 more_
- `deploy/monitor/outbox_backlog_probe.py`
  - function run_backlog_query: (dsn, timeout) -> str
  - function parse_backlog: (output) -> tuple[int, int]
  - function evaluate: (pending_count, oldest_seconds, *, max_pending, max_age_seconds) -> dict
  - function parse_args: (argv) -> argparse.Namespace
  - function main: (argv, query_runner) -> int
  - class ProbeError
- `deploy/monitor/status_state.py`
  - function reject_duplicate_keys: (pairs, object]]) -> dict
  - function load_json: (path, max_bytes) -> JsonLoad
  - function valid_monitor_status: (value) -> bool
  - function load_monitor_status: (path, max_bytes) -> tuple[dict, bool]
  - function quarantine: (path) -> Path | None
  - function utc: (value) -> str
  - _...4 more_
- `deploy/monitor/verify_runtime_identity.py`
  - function verify_identity: (source_directory, expected_environment, expected_release_sha, environ, str], *, resolve_head, str]) -> bool
  - function run_attested_job: (source_directory, expected_environment, expected_release_sha, environ, str], *, python_executable, script, runtime_directory) -> int
  - function parse_args: (argv) -> argparse.Namespace
  - function main: (argv) -> int
- `scripts/apply_migration_checkpoint.py`
  - function load_environment_file: (path) -> Path
  - function activate_source_root: (source_root) -> Path
  - function require_module_from_source: (module, source_root) -> None
  - function parse_checkpoint_policy: (text) -> set[tuple[str, str]]
  - function validate_checkpoint_request: (*, policy, str]], source_sha, approved_source_sha, expected_current, target, current_heads, ...], script_directory) -> None
  - function read_committed_checkpoint_policy: (source_root, source_sha) -> str
  - _...4 more_
- `scripts/benchmark_product_analytics.py` — function main: () -> int
- `scripts/check_users.py` — function main: ()
- `scripts/converge_runtime_acls.py`
  - function resolve_acl_target: (environment, values, object]) -> RuntimeAclTarget
  - function build_psql_invocation: (bundle_root, target) -> tuple[list[str], dict[str, str]]
  - function main: () -> int
  - class RuntimeAclConvergenceError
  - class RuntimeAclTarget
- `scripts/explain_product_analytics.py` — function main: () -> int, function run: (days, output) -> int
- `scripts/export_openapi.py`
  - function pin_export_environment: () -> None
  - function normalize: (document) -> dict
  - function build_document: () -> dict
  - function render: (document) -> str
  - function main: (argv) -> int
- `scripts/import_gena_csv.py` — function import_csv: (file_path, fuel_type)
- `scripts/ingest_market_signals.py`
  - function assert_staging_runtime: () -> None
  - function assert_expected_database: (expected_name) -> None
  - function read_rows: (file_path) -> list[dict]
  - function print_report: (report) -> None
  - function main: () -> None
  - function run: (args) -> int
- `scripts/phase0/assert_host.py`
  - function require_outbox_drained: (*, producers_stopped, sequencer_stopped, pending_jobs, other_sessions, undispatched) -> None
  - function private_bytes: (path) -> bytes
  - function command: (arguments) -> str
  - function checkout_owner: (*, stat_path, lookup_user) -> str
  - function git_command: (arguments) -> str
  - function require_identity: (expected, actual) -> None
  - _...6 more_
- `scripts/phase0/bootstrap.py`
  - function migration_graph_digest: ()
  - function load_policy: ()
  - function attest_bootstrap_connection: (connection, identity)
  - function refuse_autocommit: ()
  - function verify_bootstrap_schema: (connection, policy)
  - function validate_manifest: (manifest)
  - _...4 more_
- `scripts/phase0/codecs.py`
  - function quote: (name) -> str
  - function relation: (name) -> str
  - function map_row: (row, object], mapping) -> dict[str, object]
  - function primary_key: (table) -> tuple[str, ...]
  - function select_text: (table, columns, ...] | None) -> str
  - function insert_statement: (table, names, ...])
  - _...6 more_
- `scripts/phase0/copy.py`
  - function read_private_file: (path, *, root_owned)
  - function private_json: (path, value)
  - function validate_url: (raw, execution)
  - function validate_manifest: (manifest)
  - function import_order: (catalog)
  - function main: ()
  - _...11 more_
- `scripts/phase0/database_fence.py` — function check_source_quiesced: (connection, identity), function check_target_fence: (connection, identity, expected_datacl)
- `scripts/phase0/host_authority.py`
  - function parse_json: (contents)
  - function read_root_private: (path)
  - function validate_identity: (identity, *, side)
  - function validate_contract: (manifest, *, tool)
  - function tree_digest: (root)
- `scripts/phase0/manifest.py`
  - function load_manifest: () -> dict
  - function digest: (value) -> str
  - function canonical_expression_text: (text_value) -> Any
  - function require_complete_inventory: (actual, declared) -> None
  - function inspect_catalog: (connection) -> CatalogManifest
  - class CatalogManifest
- `scripts/phase0/production.py`
  - function private_output: (path)
  - function load_authority: (manifest, tool, *, validation_only)
  - function secret_url: (path, identity)
  - function copy_contract: (authority, manifest)
  - function require_tool: (manifest, expected_tool)
  - function attest_endpoint: (connection, identity)
  - _...3 more_
- `scripts/phase0/refusal.py` — function coded_gate: (error), class Refusal
- `scripts/phase0/scratch_business_smoke.py`
  - function require_scratch_base_url: (value) -> str
  - function read_fixture_url: (path)
  - function side_name: (value) -> str
  - function run_cases: (base_url, fixture, report) -> None
  - function main: () -> int
  - function discover_port: (session) -> str
  - _...5 more_
- `scripts/phase0/start_fence.py` — function start_is_allowed: (control, environment, release_sha, database, *, owner_uid) -> bool, function main: () -> int
- `scripts/phase0/verify.py`
  - function publish_report: (path, value)
  - function main: ()
  - function require_unfiltered_reads: (connection)
  - function sequence_states: (connection, catalog)
  - function table_hash: (connection, table, columns)
  - function fingerprint_database: (connection, catalog)
  - _...6 more_
- `scripts/preflight_runtime.py` — function main: () -> int
- `scripts/prepare_rust_budget.py`
  - function derive_budget: (settings, rust_pool, cache_listener)
  - function read_regular: (path)
  - function effective_settings: (raw)
  - function artifact_for: (environment_file, release_sha, cache_listener)
  - function prepare: (environment_file, release_sha, output, cache_listener)
  - function verify: (environment_file, release_sha, artifact)
  - _...1 more_
- `scripts/prepare_rust_runtime.py`
  - function read_regular: (path) -> bytes
  - function load_policy: (source_root, environment) -> dict
  - function app_database_url: (values, environment) -> str
  - function kyc_execution_gate: (values) -> str
  - function phase2_settings: (values) -> dict
  - function project_runtime_environment: (environment_file, release_sha, migration_revision, policy) -> dict
  - _...4 more_
- `scripts/prune_product_analytics.py`
  - function parse_cli_args: (argv) -> argparse.Namespace
  - function assert_runtime_identity: (*, configured_environment, configured_release_sha, expected_environment, expected_release_sha) -> None
  - function compute_cutoff: (today) -> date
  - function cli: (argv) -> int
  - function prune_login_days: (session, *, today) -> int
  - function main: (*, expected_environment, expected_release_sha) -> int
- `scripts/remediate_market_data.py`
  - function parse_args: (argv) -> argparse.Namespace
  - function main: () -> None
  - function run: (args) -> dict[str, Any]
- `scripts/run_demo_activity.py` — function main: () -> None
- `scripts/rust_cache_authority.py`
  - function new_identity: () -> str
  - function parse_record: (text) -> Record
  - function render_record: (record) -> str
  - function read_record: (runtime_dir) -> Record | None
  - function publish_record: (runtime_dir, record) -> Path
  - function describe: (record) -> dict[str, object]
  - _...9 more_
- `scripts/rust_caddy_candidate.py`
  - function candidate: (original, environment, group) -> str
  - function staging_candidate: (original, environment, group) -> str
  - function reviewed_route_import: (original, match, label) -> None
  - function staging_crud_candidate: (original, environment, group) -> str
  - function production_crud_block: () -> str
  - function production_crud_region: (original) -> tuple[int, int, str] | None
  - _...14 more_
- `scripts/rust_contract/admission_probe.py` — function slice_cpu: () -> int, function run: () -> None
- `scripts/rust_contract/baseline.py` — function gate_config: () -> dict[str, Any], function load_python_baseline: (path, pin, *, successor) -> dict[str, Any]
- `scripts/rust_contract/check_assertions.py`
  - function discover: (root) -> list[dict[str, str]]
  - function file_digests: (root) -> dict[str, str]
  - function build_ledger: (root) -> dict
  - function check: (ledger, discovered, str]]) -> list[str]
  - function check_targets: (ledger, passed_groups) -> list[str]
  - function main: (argv) -> int
- `scripts/rust_contract/compare.py`
  - function parse_json: (raw) -> Any
  - function strip_headers: (headers, str]) -> dict[str, str]
  - function compare: (expected, Any], actual, Any]) -> list[str]
  - function compare_responses: (expected, actual, *, profile, route_id, rules_sha256, rules_path, request_id, diagnostic_context, request_path, live)
  - function compare_news_tie_groups: (expected, actual, *, request_path, rules_path)
  - function compare_price_summary_multiset: (expected, actual, *, request_path, rules_path)
  - _...7 more_
- `scripts/rust_contract/compare_values.py` — function main: ()
- `scripts/rust_contract/curve_fixtures.py` — function seed_curve_fixtures: (session)
- `scripts/rust_contract/explain_benchmark.py` — function main: (), class ExplainBenchmark
- `scripts/rust_contract/explain_lists.py` — function main: (), class Explain
- `scripts/rust_contract/export_mcp_http.py` — function vectors: (), function main: ()
- `scripts/rust_contract/export_mcp_protocol.py` — function vectors: ()
- `scripts/rust_contract/export_mcp_validation.py` — function vectors: ()
- `scripts/rust_contract/export_policy.py`
  - function recorded_source_sha: (root) -> str
  - function build_policy: (source_sha) -> dict
  - function build_mcp_catalog: (source_sha) -> dict
  - function validate_policy: (payload) -> None
  - function render: (payload) -> str
  - function build_catalog_vectors: (root) -> dict[str, str]
  - _...4 more_
- `scripts/rust_contract/export_terms_vectors.py` — function build_vectors: () -> dict[str, str], function main: (argv) -> int
- `scripts/rust_contract/export_voyage.py` — function main: ()
- `scripts/rust_contract/export_voyage_decimal.py` — function vectors: ()
- `scripts/rust_contract/export_voyage_vectors.py` — function vectors: ()
- `scripts/rust_contract/feedback_oracle.py` — function main: ()
- `scripts/rust_contract/feedback_scenario.py`
  - function setup: (connection)
  - function servers: (context)
  - function rows: (state)
  - function unchanged: (before, after, response, mail)
  - function inserted: (identifier, message, page)
  - function actions: (context)
  - _...1 more_
- `scripts/rust_contract/frozen_gate.py`
  - function validate_identity: (evidence, Any], reference, Any], *, successor) -> None
  - function route_floors: (reference, Any], declared_rates, offered) -> dict[str, int]
  - function performance_evidence: (size, evidence, Any] | None) -> dict[str, Any]
  - function frozen_verdict: (evidence, Any], *, successor) -> dict[str, Any]
- `scripts/rust_contract/image_identity.py` — function matches_pinned_image: (actual, expected) -> bool
- `scripts/rust_contract/index_experiment.py` — function main: (), class IndexExperiment
- `scripts/rust_contract/mcp_acceptance.py`
  - function privacy: (value, private_values)
  - function main: ()
  - function fixture_private_values: ()
  - function cancellation: (base_url)
- `scripts/rust_contract/mcp_record.py` — function read_mcp_response: (response, case, *, max_bytes)
- `scripts/rust_contract/meaning_runner.py`
  - function route_id: (request)
  - function raw_response: (recording)
  - function compare_recorded_case: (left, right, profile, digest)
  - function canonical_response: (request, response)
- `scripts/rust_contract/measure.py`
  - function adaptive_choice: (probes, Any]], max_rps) -> dict[str, Any]
  - function percentile: (sorted_values, q) -> float
  - function summarize: (samples) -> dict[str, float]
  - function weighted_mix: (per_route_p50, float], per_route_p95, float], per_route_p99, float], weights, int]) -> dict[str, float]
  - function coverage: (counts, int], required) -> dict[str, Any]
  - function parse_memory_stat: (text) -> dict[str, int]
  - _...7 more_
- `scripts/rust_contract/measure_lists.py`
  - function require_latency_parity: (summary)
  - function main: ()
  - class Measurement
- `scripts/rust_contract/notifications_oracle.py` — function with_response_loss: (self, scope, receive, send)
- `scripts/rust_contract/phase2_alert_dispatch.py` — function main: ()
- `scripts/rust_contract/phase2_alert_oracle.py` — function install: (app), function main: ()
- `scripts/rust_contract/phase2_alert_pd8.py` — function run_pd8: (race, cells), class PD8Race
- `scripts/rust_contract/phase2_alerts.py`
  - function alert_id: (number)
  - function setup: (connection)
  - function servers: (context)
  - function action: (name, method, number, actor, status, body, fault, barrier)
  - function actions: (_context)
  - function races: (_context)
  - _...4 more_
- `scripts/rust_contract/phase2_auth_vectors.py`
  - function specifications: ()
  - function export: ()
  - function main: ()
- `scripts/rust_contract/phase2_example.py` — function example_scenario: ()
- `scripts/rust_contract/phase2_mail.py` — class MailStub
- `scripts/rust_contract/phase2_market_vectors.py` — function build_spec: () -> dict[str, object], function main: (argv) -> int
- `scripts/rust_contract/phase2_meaning.py` — function compare_meaning: (expected, actual, *, route_id, rules_sha256, rules_path, request_id, profile, diagnostic_context)
- `scripts/rust_contract/phase2_notifications.py`
  - function token: (actor)
  - function rows: (state)
  - function assert_changes: (before, after, mail, *, actor, notification)
  - function scenario: ()
- `scripts/rust_contract/phase2_oracle.py` — function main: ()
- `scripts/rust_contract/phase2_order_write.py` — function order_write_scenario: ()
- `scripts/rust_contract/phase2_order_write_oracle.py` — function identity: (), function command: (request)
- `scripts/rust_contract/phase2_preferences.py`
  - function setup: (connection)
  - function servers: (context)
  - function headers: (index, user)
  - function rows: (state)
  - function assertion: (user, namespace, value, clock, unchanged)
  - function actions: (context)
  - _...2 more_
- `scripts/rust_contract/phase2_preferences_vectors.py`
  - function canonical: (value) -> bytes
  - function padded_ascii: (total, value)
  - function padded_multibyte: (total, value)
  - function notifications: (**changes)
  - function specifications: ()
  - function stub_rows: ()
  - _...3 more_
- `scripts/rust_contract/phase2_referrals.py`
  - function setup: (connection)
  - function headers: ()
  - function servers: (context)
  - function unchanged: (before, after, response, mail)
  - function actions: (context)
  - function races: (context)
  - _...1 more_
- `scripts/rust_contract/phase2_report.py`
  - function require: (condition, message)
  - function count: (value)
  - function finite: (value, *, positive)
  - function load: (path)
  - function exact_ids: (values, expected)
  - function verified_path: (directory, reference)
  - _...8 more_
- `scripts/rust_contract/phase2_routes.py` — function run_registered: (runner, args, selected), class Context
- `scripts/rust_contract/phase2_scenarios.py`
  - function register: (scenario)
  - function select_scenarios: (group, names)
  - function run_actions: (actions, cells)
  - function actor_order: (actors, seed)
  - function run_barrier: (actors, invoke, *, seed)
  - function run_race: (race, cells)
  - _...7 more_
- `scripts/rust_contract/phase2_seed.py`
  - function identifier: (name)
  - function snapshot: (connection)
  - function sequence_marks: (connection)
  - function digest: (value)
  - function connect: (port, database, *, app, autocommit)
  - class Rows
  - _...1 more_
- `scripts/rust_contract/phase2_state.py`
  - function expect_cache_invalidations: (expected, statements)
  - function validate_cells: (python, rust, token, target_url)
  - function compare_state: (python, rust)
  - function unexpected_user_changes: (before, after, *, route_id, actor_id, recipient_exists)
  - function apply_action: (cell, writes, *, fault)
  - function validate_action: (action)
  - _...2 more_
- `scripts/rust_contract/phase2_watchlist_asserts.py`
  - function uid: (group, number)
  - function rows: (state, table)
  - function unchanged_except: (before, after, *tables)
  - function created: (before, after, table)
  - function same_instant: (stored, expected)
  - function body_of: (response)
  - _...2 more_
- `scripts/rust_contract/phase2_watchlist_containers.py` — function watchlist_containers_scenario: ()
- `scripts/rust_contract/phase2_watchlist_fixtures.py`
  - function at: (days, minutes)
  - function bearer: (secret, user)
  - function insert_user: (connection, user_id, email, organization_id)
  - function insert_container: (connection, container_id, user_id, name, created_at, kind)
  - function insert_slice: (connection, target_id, container_id, product_code, point_id, window, created_at)
  - function insert_pin: (connection, target_id, container_id, order_id, point_id, created_at, window)
  - _...4 more_
- `scripts/rust_contract/phase2_watchlist_targets.py`
  - function watchlist_targets_scenario: ()
  - function watchlist_targets_gate_off_scenario: ()
  - function watchlist_targets_hidden_scenario: ()
- `scripts/rust_contract/phase2_watchlists.py` — function watchlist_reads_scenario: (), function watchlist_reads_gate_off_scenario: ()
- `scripts/rust_contract/profile_lists.py` — function main: (), class Profile
- `scripts/rust_contract/record.py`
  - function perform: (base_url, case, Any]) -> dict[str, Any]
  - function record: (base_url, token, cases, Any]]) -> list[dict[str, Any]]
  - function stable: (entries, Any]]) -> list[dict[str, Any]]
  - function differences: (first, Any]], second, Any]]) -> list[str]
  - function write_golden: (root, source_sha, entries, Any]], *, meta, Any]) -> None
- `scripts/rust_contract/record_mcp.py` — function main: (), class McpRecorder
- `scripts/rust_contract/referral_addresses.py`
  - function setup: (connection)
  - function servers: (context)
  - function rejected: (before, after, response, mail)
  - function code_only: (before, after)
  - function sent_to: (address, count)
  - function no_send: (count)
  - _...3 more_
- `scripts/rust_contract/referral_allocation.py`
  - function setup: (connection)
  - function auth: (actor)
  - function servers: (context)
  - function only_code: (before, after, response, mail)
  - function actions: (context)
  - function wait_for: (connection, sql, args, count)
  - _...2 more_
- `scripts/rust_contract/referral_dispatch.py` — function main: ()
- `scripts/rust_contract/referral_email_sweep.py`
  - function selected: (code)
  - function outcome: (value)
  - function main: ()
- `scripts/rust_contract/referral_email_tables.py`
  - function rust_string: (text)
  - function runs: (function)
  - function emit_array: (name, element_type, rows, per_line)
  - function emit_ranges: (name, ranges)
  - function intrange_pairs: (encoded)
  - function main: ()
- `scripts/rust_contract/referral_email_vectors.py`
  - function length_vectors: ()
  - function domain_vectors: ()
  - function local_vectors: ()
  - function quoted_and_display_vectors: ()
  - function vectors: ()
  - function has_surrogate: (value)
  - _...2 more_
- `scripts/rust_contract/referral_faults.py`
  - function setup: (connection)
  - function servers: (context)
  - function unchanged: (before, after, response, mail)
  - function retained: (before, after, response, mail)
  - function actions: (context)
  - function scenario: ()
- `scripts/rust_contract/referral_invite.py`
  - function setup: (connection)
  - function candidate_servers: (context)
  - function recipient: (sender)
  - function keeps_only_code: (sender, before, after)
  - function attempted: (sender, count, *, retained, sends)
  - function invite: (name, sender, status, check, *, expected_status, outcome, headers, recipient_exists, mail_status, mail_body, to)
  - _...8 more_
- `scripts/rust_contract/referral_mixed.py`
  - function mixed: (context)
  - function dispatch: (context)
  - function assert_mixed: (state, result)
  - function scenario: ()
- `scripts/rust_contract/referral_oracle.py` — function main: ()
- `scripts/rust_contract/referral_populated.py`
  - function referrer_id: (number)
  - function code: (number)
  - function setup: (connection)
  - function servers: (context)
  - function body_of: (response)
  - function unchanged: (before, after, response, mail)
  - _...5 more_
- `scripts/rust_contract/referral_timeout_relay.py` — function start: (target)
- `scripts/rust_contract/referral_timeout_server.py` — function stop_on_signal: (signal_number, frame), function main: ()
- `scripts/rust_contract/replay_mcp.py` — function compare_evidence: (entries, evidence, *, gate), function main: ()
- `scripts/rust_contract/report.py`
  - function sha256_file: (path) -> str
  - function sha256_bytes: (data) -> str
  - function evidence_digest: (evidence) -> str
  - function memory_table: (evidence, Any]) -> dict[str, dict[str, Any]]
  - function cpu_table: (evidence, Any]) -> dict[str, dict[str, Any]]
  - function latency_table: (evidence, Any]) -> dict[str, dict[str, dict[str, Any]]]
  - _...12 more_
- `scripts/rust_contract/run_disposable.py`
  - function ephemeral_range: () -> tuple[int, int]
  - function ephemeral_port: () -> int
  - function docker: (*args, check, input_text) -> str
  - function wait_for_port: (port, timeout) -> None
  - function attest: (base_url, token) -> None
  - function main: (argv) -> int
  - _...2 more_
- `scripts/rust_contract/seed_corpus.py`
  - function attest_target: (*, environment, expected_database, connected_database, connected_user, is_superuser, host) -> None
  - function fid: (name)
  - function validate_counts: (counts)
  - function matrix_module: ()
  - function main: (argv) -> int
  - function attest_session: (session)
  - _...3 more_
- `scripts/rust_contract/seed_integration.py` — function main: () -> None
- `scripts/rust_contract/shadow.py`
  - function load_config: (path) -> ShadowConfig
  - function validate_config: (config) -> None
  - function load_staging_cases: (path, *, require_complete) -> list[dict[str, Any]]
  - function round_robin_cases: (cases)
  - function memory_pressure: () -> bool
  - function is_production_url: (url) -> bool
  - _...16 more_
- `scripts/rust_contract/simulate.py` — function main: (argv) -> int, class Simulation
- `scripts/rust_contract/sqlx_offline_check.py` — function main: (), class Check
- `scripts/rust_contract/t14b_regressions.py` — function main: (), class Diagnose
- `scripts/rust_contract/t14c_probe.py` — function main: () -> int
- `scripts/rust_contract/t14c_shapes.py`
  - function send_case: (client, url, name, path) -> "httpx.Response"
  - function main: ()
  - class Diagnose
- `scripts/rust_contract/trade_tape_oracle.py` — function canonical_tape_page: (rows, filters, clock, skip, limit, *, snapshot_identity, projector), function read_canonical_tape_page: (session, filters, clock, skip, limit, *, snapshot_identity)
- `scripts/rust_contract/workload.py`
  - function load: (path) -> dict[str, Any]
  - function canonical_json: (value) -> str
  - function manifest_digest: (manifest, Any]) -> str
  - function validate: (manifest, Any]) -> None
  - function routes: (manifest, Any]) -> list[dict[str, Any]]
  - function tools: (manifest, Any]) -> list[dict[str, Any]]
  - _...7 more_
- `scripts/rust_contract/workload_seed.py` — function main: (argv) -> int, function seed_workload: (session, size, counts, int], seed) -> dict
- `scripts/rust_passive_no_harm.py`
  - function matcher_block: (group) -> str
  - function matches: (group, method, uri) -> bool
  - function p95: (values) -> float
  - function timestamp: (value) -> float
  - function evaluate: (rows, group, cutover, seconds, ready, before_errors, after_errors)
  - function error_classes: (rows) -> set[str]
  - _...4 more_
- `scripts/scrape_fleet_demand.py`
  - function run_batch: (commands, timeout) -> str
  - function extract_last_value: (raw) -> str
  - function get_afi_article_url: () -> str | None
  - function get_page_body: (url) -> str
  - function extract_int: (text, patterns) -> int | None
  - function scrape: ()
- `scripts/security_preflight.py` — function main: () -> int, function report: () -> int
- `scripts/seed.py` — function parse_args: () -> argparse.Namespace, function main: () -> None
- `scripts/seed_compliance_data.py` — function add_entry: (org_key, transaction_type, amount, currency, units, description, reference_id, created_at)
- `scripts/seed_forward_monitoring_demo.py` — function assert_catalog_ready: (db) -> None, function main: () -> None
- `scripts/seed_maersk_vessels.py` — function seed_vessels: ()
- `scripts/seed_port_inventory.py`
  - function get_region: (port_id)
  - function get_price_per_mt: (fuel_type, port_id)
  - function get_methanol_price_avg: (port_id)
  - function get_biofuel_price_avg: (port_id)
  - function get_stock_levels: (fuel_type)
  - function is_certified: (fuel_type)
  - _...4 more_
- `scripts/seed_realistic_orderbook.py`
  - function rand_quantity: (min_lot, max_lot)
  - function rand_price_around: (mid, spread_pct, side)
  - function random_created_at: ()
  - function build_order: (side, product_key, dp_key, mid, spread_pct, min_lot, max_lot)
- `scripts/seed_vessels_fleet.py`
  - function get_db_connection: ()
  - function generate_vessel_data: ()
  - function format_geography_point: (lng, lat)
  - function insert_vessels: (conn, vessels)
  - function main: ()
- `scripts/smoke_umami_product_analytics.py`
  - function validate_event_data_events: (payload) -> dict[str, Any]
  - function validate_event_data_properties: (payload) -> dict[str, Any]
  - function validate_event_data_values: (payload) -> dict[str, Any]
  - function validate_event_data_pivot_envelope: (payload) -> dict[str, Any]
  - function ensure_permitted_base_url: (base_url, *, allow_remote_readonly) -> None
  - function main: () -> int
  - _...2 more_
- `scripts/validate_health_response.py`
  - function validate_health_response: (payload, *, expected_environment, expected_release_sha) -> None
  - function main: () -> int
  - class HealthResponseError
- `scripts/verify_deploy_start_guard.py` — function main: (arguments) -> int
- `scripts/verify_migration_revision.py` — function main: () -> int
- `scripts/verify_rust_artifact.py`
  - function sha256_bytes: (content) -> str
  - function validate_metadata: (document) -> dict
  - function check_glibc_max: (objdump_text, maximum) -> str
  - function inspect_archive: (data, source_sha, target) -> dict
  - function verify_provenance: (record_bytes, signature, public_key_path, *, expected) -> dict
  - function run_attest: (args) -> int
  - _...9 more_
- `scripts/verify_rust_start.py` — function main: ()
- `scripts/verify_systemd_source.py`
  - function verify_source_provenance: (*, source_root, source_ref, environment) -> dict[str, str]
  - function build_verified_unit_archive: (*, source_root, source_ref, environment) -> bytes
  - function main: () -> int
  - class SourceProvenanceError
- `work/rust-phase2/watchlists-reference.py`
  - function encode_event_cursor: (created_at, event_id) -> str
  - function decode_event_cursor: (cursor) -> tuple[datetime, UUID]
  - function build_watchlist_event_response: (event, target, *, delivery_point_name) -> WatchlistEventResponse
  - function public_watchlist_orders: ()
  - function watchlist_target_visibility_clause: ()
  - function watchlist_event_visibility_clause: ()
  - _...5 more_

---

# Config

## Environment Variables

- `ADMIN_PASSWORD` (has default) — .env.example
- `ADMIN_SESSION_SECRET` (has default) — .env.example
- `ADMIN_USERNAME` (has default) — .env.example
- `ALERT_COOLDOWN_SECONDS` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `ANALYTICS_CANARY_ENABLED` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `ANALYTICS_DATA_DIR` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `ANALYTICS_ENABLED` (has default) — .env.example
- `ANALYTICS_MAX_DATA_BYTES` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `ANALYTICS_REQUEST_TIMEOUT_SECONDS` (has default) — .env.example
- `ANALYTICS_WEBSITE_ID` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `AUTH_INTEROP_ADMIN_URL` **required** — tests/rust_contract/phase2/auth_writer.py
- `AUTH_INTEROP_EXPECT_BLOCKED` **required** — tests/rust_contract/phase2/auth_writer.py
- `AUTH_INTEROP_SUBJECT` **required** — tests/rust_contract/phase2/auth_writer.py
- `AUTODIAG_COLLECT_COMMANDS` (has default) — deploy/external_monitor/verdaxis_autodiag.py
- `AUTODIAG_RECOVERY_STATE_DIR` (has default) — deploy/external_monitor/verdaxis_autodiag.py
- `AUTODIAG_SCHEMA_FILE` (has default) — deploy/external_monitor/verdaxis_autodiag.py
- `AUTODIAG_STATE_DIR` (has default) — deploy/external_monitor/verdaxis_autodiag.py
- `AUTODIAG_STATUS_FILE` (has default) — deploy/external_monitor/verdaxis_autodiag.py
- `AUTODIAG_WORKSPACE` (has default) — deploy/external_monitor/verdaxis_autodiag.py
- `BACKUP_DATABASE_URL` **required** — tests/postgres/test_runtime_role_policy.py
- `BACKUP_MAX_AGE_SECONDS` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `BACKUP_STATUS_FILE` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `CADDY_BIN` **required** — scripts/rust_contract/measure.py
- `CADDYFILE` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `CHROMEDRIVER_BIN` **required** — deploy/external_monitor/verdaxis_monitor.py
- `CHROMIUM_PROFILE_PARENT` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `CI` (has default) — tests/postgres/conftest.py
- `CODEX_AUTH_SOURCE` (has default) — deploy/external_monitor/verdaxis_autodiag.py
- `CODEX_AUTH_SYNC` (has default) — deploy/external_monitor/verdaxis_autodiag.py
- `CODEX_AUTODIAG_COOLDOWN_SECONDS` (has default) — deploy/external_monitor/verdaxis_autodiag.py
- `CODEX_AUTODIAG_MODEL` (has default) — deploy/external_monitor/verdaxis_autodiag.py
- `CODEX_AUTODIAG_TIMEOUT_SECONDS` (has default) — deploy/external_monitor/verdaxis_autodiag.py
- `CODEX_BIN` (has default) — deploy/external_monitor/verdaxis_autodiag.py
- `CODEX_HOME` (has default) — deploy/external_monitor/verdaxis_autodiag.py
- `CREDENTIALS_DIRECTORY` **required** — app/services/kyc_document_vault.py
- `DATABASE_HOST` (has default) — .env.example
- `DATABASE_NAME` (has default) — .env.example
- `DATABASE_PASSWORD` (has default) — .env.example
- `DATABASE_PORT` (has default) — .env.example
- `DATABASE_URL` (has default) — scripts/rust_contract/meaning_runner.py
- `DATABASE_USER` (has default) — .env.example
- `DB_IDLE_IN_TRANSACTION_SESSION_TIMEOUT_MS` (has default) — .env.example
- `DB_LOCK_TIMEOUT_MS` (has default) — .env.example
- `DB_MAX_CONNECTIONS` (has default) — .env.example
- `DB_MAX_OVERFLOW` (has default) — .env.example
- `DB_POOL_RECYCLE` (has default) — .env.example
- `DB_POOL_SIZE` (has default) — .env.example
- `DB_POOL_TIMEOUT` (has default) — .env.example
- `DB_RESERVED_CONNECTIONS` (has default) — .env.example
- `DB_SERVICE_COUNT` (has default) — .env.example
- `DB_STATEMENT_TIMEOUT_MS` (has default) — .env.example
- `DEMO_OUTPUT` **required** — tests/monitor/test_runtime_identity.py
- `DEMO_TRADE_CANARY_ENABLED` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `DISPOSABLE_ITEST_PASSWORD` **required** — scripts/rust_contract/seed_integration.py
- `DISPOSABLE_TEST_TOKEN` **required** — rs/read/tests/curves_canonical_oracle.py
- `ENABLE_AUTH_BYPASS` (has default) — .env.example
- `ENABLE_SQLADMIN` (has default) — .env.example
- `ENVIRONMENT` (has default) — .env.example
- `EXPENSIVE_CHECK_INTERVAL_SECONDS` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `FRONTEND_BUNDLE_CHECKS_ENABLED` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `FRONTEND_URL` (has default) — .env.example
- `GEMINI_API_KEY` **required** — .env.example
- `GIT_CONFIG_GLOBAL` **required** — tests/unit/test_migration_checkpoint.py
- `HEALTH_READINESS_TIMEOUT_SECONDS` (has default) — .env.example
- `HEALTHCHECKS_PING_URL` **required** — deploy/external_monitor/verdaxis_monitor.py
- `HTTP_RETRIES` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `HTTP_RETRY_DELAY_SECONDS` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `HTTP_TIMEOUT_SECONDS` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `ITEST_PASSWORD` (has default) — scripts/benchmark_product_analytics.py
- `JWT_ALGORITHM` (has default) — .env.example
- `JWT_AUDIENCE` (has default) — .env.example
- `JWT_ISSUER` (has default) — .env.example
- `JWT_SECRET` (has default) — .env.example
- `JWT_SECRET_PREVIOUS` **required** — .env.example
- `KYC_EXECUTION_GATE_ENABLED` **required** — rs/read/tests/legacy_curve_oracle.py
- `KYC_MAX_FILE_BYTES` (has default) — .env.example
- `KYC_MAX_TOTAL_BYTES` (has default) — .env.example
- `LANG` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `LC_ALL` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `MARKET_INTEGRITY_TEST_DATABASE_URL` (has default) — tests/postgres/test_market_integrity_concurrency.py
- `MARKET_REMEDIATION_DATABASE_URL` **required** — scripts/remediate_market_data.py
- `MARKET_SUPPORT_BOOTSTRAP_ADMIN_USER_IDS` **required** — .env.example
- `MARKET_SUPPORT_CONSENT_REFERENCE` **required** — .env.example
- `MARKET_SUPPORT_CONSENT_VERSION` **required** — .env.example
- `MARKET_SUPPORT_CONTEXT_TTL_MINUTES` (has default) — .env.example
- `MARKET_SUPPORT_ENABLED` (has default) — .env.example
- `MARKET_SUPPORT_MAX_TTL_HOURS` (has default) — .env.example
- `MEANING_CORPUS_DIR` **required** — scripts/rust_contract/meaning_runner.py
- `MEANING_RULES_SHA256` **required** — scripts/rust_contract/compare.py
- `MIGRATOR_DATABASE_URL` **required** — .env.example
- `MIGRATOR_IDLE_IN_TRANSACTION_SESSION_TIMEOUT_MS` (has default) — .env.example
- `MIGRATOR_LOCK_TIMEOUT_MS` (has default) — .env.example
- `MIGRATOR_STATEMENT_TIMEOUT_MS` (has default) — .env.example
- `MIN_CADDYFILE_BYTES` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `MIN_DISK_FREE_PERCENT` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `MONITOR_TOKEN` **required** — deploy/external_monitor/verdaxis_monitor.py
- `PATH` **required** — tests/phase0/http_runner.py
- `PGPASSFILE` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `PHASE0_CLUSTER_ID` **required** — scripts/phase0/bootstrap.py
- `PHASE0_PASSWORD` **required** — tests/phase0/build_fixtures.py
- `PHASE0_PORT` (has default) — scripts/phase0/bootstrap.py
- `PHASE0_PYTEST_K` (has default) — tests/phase0/linux_database.py
- `PHASE0_SMOKE_TRACEBACK` **required** — scripts/phase0/scratch_business_smoke.py
- `PHASE0_WORK` **required** — tests/phase0/build_fixtures.py
- `PHASE2_BIND` **required** — scripts/rust_contract/phase2_alert_oracle.py
- `PHASE2_DATABASE_URL` **required** — scripts/rust_contract/phase2_alert_dispatch.py
- `PHASE2_EMIT_EVENTS` **required** — tests/rust_contract/phase2/transport.py
- `PHASE2_MAIL_URL` (has default) — scripts/rust_contract/phase2_oracle.py
- `POSTGRES_ADMIN_TEST_DATABASE_URL` **required** — tests/postgres/test_market_integrity_concurrency.py
- `PYTHON_CONTRACT_URL` **required** — rs/read/tests/curves_canonical_oracle.py
- `PYTHONPATH` **required** — tests/unit/test_migration_checkpoint.py
- `RECOVERY_STATE_DIR` (has default) — deploy/external_monitor/verdaxis_recover.py
- `RECOVERY_STATUS_FILE` (has default) — deploy/external_monitor/verdaxis_recover.py
- `REFERRAL_CANDIDATE_MAP` (has default) — scripts/rust_contract/referral_oracle.py
- `REFERRAL_TEST_MAIL_KEY_ABSENT` **required** — scripts/rust_contract/referral_oracle.py
- `RELEASE_SHA` (has default) — .env.example
- `RENDER_RETRIES` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `RENDER_RETRY_DELAY_SECONDS` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `RENDER_TIMEOUT_SECONDS` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `RENDERED_PAGE_CHECKS_ENABLED` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `RUN_SHADOW_DISPOSABLE_REPLAY` **required** — tests/rust_contract/test_shadow_replay.py
- `RUNTIME_TEST_APP_ROLE` (has default) — tests/postgres/test_fame_rfq.py
- `RUNTIME_TEST_BACKUP_ROLE` (has default) — tests/postgres/test_runtime_role_policy.py
- `RUNTIME_TEST_DATABASE_NAME` (has default) — tests/postgres/test_runtime_role_policy.py
- `RUNTIME_TEST_MIGRATOR_ROLE` **required** — tests/postgres/test_runtime_migration_roundtrip.py
- `RUST_CONTRACT_CLOCK` **required** — rs/read/tests/legacy_curve_oracle.py
- `RUST_CONTRACT_PROFILE` **required** — rs/read/tests/prices_oracle.py
- `RUST_CONTRACT_URL` **required** — rs/read/tests/curves_canonical_oracle.py
- `RUST_CORPUS_MANIFEST` (has default) — scripts/rust_contract/meaning_runner.py
- `RUST_CORPUS_STAGE` **required** — scripts/rust_contract/seed_corpus.py
- `RUST_DATABASE_URL` **required** — rs/read/tests/orderbook_lists_oracle.py
- `RUST_MARKET_EXPECTED_DATABASE` **required** — rs/read/tests/legacy_curve_oracle.py
- `RUST_MARKET_TEST_EXPECTED` **required** — rs/read/tests/market_contract_oracle.py
- `RUST_MARKET_WRITER_URL` **required** — rs/read/tests/market_contract_oracle.py
- `SEED_DATABASE_URL` **required** — scripts/rust_contract/seed_corpus.py
- `SEED_TARGET_DATABASE` **required** — rs/read/tests/legacy_curve_oracle.py
- `SELLER_FEE_PILOT_PER_MT_USD` (has default) — .env.example
- `SELLER_FEE_PROFESSIONAL_PER_MT_USD` (has default) — .env.example
- `SIGNUP_CANARY_ENABLED` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `SIGNUP_CANARY_TARGETS` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `STATE_FILE` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `STATUS_FILE` (has default) — deploy/external_monitor/verdaxis_monitor.py
- `TELEGRAM_API_BASE` (has default) — app/cli/onboarding_attention.py
- `TELEGRAM_BOT_TOKEN` (has default) — app/cli/onboarding_attention.py
- `TELEGRAM_CHAT_ID` (has default) — app/cli/onboarding_attention.py
- `TELEGRAM_DISABLED` (has default) — deploy/external_monitor/verdaxis_autodiag.py
- `TEST_API_URL` **required** — tests/conftest.py
- `UMAMI_API_PASSWORD` **required** — .env.example
- `UMAMI_API_USERNAME` **required** — .env.example
- `UMAMI_BASE_URL` (has default) — .env.example
- `UMAMI_WEBSITE_ID` **required** — .env.example
- `UVICORN_BIND_PORT` (has default) — tests/rust_contract/server.py
- `UVICORN_PORT` (has default) — tests/rust_contract/server.py
- `UVICORN_WORKERS` (has default) — .env.example
- `VERDAXIS_RECOVERY_COOLDOWN_SECONDS` (has default) — deploy/external_monitor/verdaxis_recover.py
- `WEBZ_API_TIMEOUT_SECONDS` (has default) — .env.example
- `WEBZ_API_TOKEN` **required** — .env.example
- `WEBZ_API_URL` (has default) — .env.example

## Config Files

- `.env.example`
- `Dockerfile`
- `docker-compose.yml`
- `pyproject.toml`

---

# Middleware

## auth
- auth_20261002_session_cutoff — `alembic/versions/auth_20261002_session_cutoff.py`
- auth_2026_07_add_must_change_password — `alembic/versions/auth_2026_07_add_must_change_password.py`
- execution — `app/middleware/execution.py`
- preauth_rate_limit — `app/middleware/preauth_rate_limit.py`
- rbac — `app/middleware/rbac.py`
- auth-maintenance-timer — `docs/auth-maintenance-timer.md`
- auth_tokens — `rs/read/tests/auth_tokens.rs`
- auth_transaction_panic — `rs/read/tests/auth_transaction_panic.rs`
- auth_transactions — `rs/read/tests/auth_transactions.rs`
- auth_writer — `tests/rust_contract/phase2/auth_writer.py`
- test_preauth_rate_limit — `tests/unit/test_preauth_rate_limit.py`
- auth_limits — `work/t22-glm-draft/auth_limits.rs`

## custom
- market_support_scope — `app/middleware/market_support_scope.py`
- public_market_read_admission — `app/middleware/public_market_read_admission.py`
- subscription — `app/middleware/subscription.py`
- integrated-migration-cutover — `docs/runbooks/integrated-migration-cutover.md`
- verify_deploy_start_guard — `scripts/verify_deploy_start_guard.py`

## error-handler
- network_guard — `tests/phase0/network_guard.py`

## rate-limit
- test_disposable_rate_limits — `tests/unit/test_disposable_rate_limits.py`

## cors
- test_idempotency_cors — `tests/unit/test_idempotency_cors.py`

---

# Dependency Graph

## Most Imported Files (change these carefully)

- `app/models/user.py` — imported by **170** files
- `app/database.py` — imported by **125** files
- `app/models/orderbook.py` — imported by **112** files
- `app/market_catalog.py` — imported by **92** files
- `app/config.py` — imported by **70** files
- `app/models/catalog.py` — imported by **68** files
- `app/routers/auth_simple.py` — imported by **61** files
- `app/services/availability_windows.py` — imported by **38** files
- `app/main.py` — imported by **36** files
- `app/schemas/market_activity.py` — imported by **33** files
- `app/model_base.py` — imported by **32** files
- `app/services/execution_policy.py` — imported by **30** files
- `scripts/rust_contract/run_disposable.py` — imported by **30** files
- `app/core/security.py` — imported by **29** files
- `app/schemas/orderbook.py` — imported by **28** files
- `app/rate_limit.py` — imported by **25** files
- `app/services/market_data_eligibility.py` — imported by **25** files
- `app/routers/orderbook.py` — imported by **24** files
- `app/services/audit_actions.py` — imported by **24** files
- `scripts/rust_contract/compare.py` — imported by **24** files

## Import Map (who imports what)

- `app/models/user.py` ← `app/admin.py`, `app/cli/send_kyc_grace_notices.py`, `app/middleware/execution.py`, `app/middleware/rbac.py`, `app/models/__init__.py` +165 more
- `app/database.py` ← `alembic/env.py`, `app/admin.py`, `app/cli/auth_maintenance.py`, `app/cli/onboarding_attention.py`, `app/cli/order_expiry_reminders.py` +120 more
- `app/models/orderbook.py` ← `app/models/__init__.py`, `app/models/live_slice_benchmark.py`, `app/models/user.py`, `app/routers/admin_analytics.py`, `app/routers/compliance_api.py` +107 more
- `app/market_catalog.py` ← `app/market_constraints.py`, `app/mcp_server.py`, `app/models/catalog.py`, `app/models/orderbook.py`, `app/models/supplier_offer.py` +87 more
- `app/config.py` ← `alembic/env.py`, `app/admin.py`, `app/cli/onboarding_attention.py`, `app/cli/order_expiry_reminders.py`, `app/cli/send_kyc_grace_notices.py` +65 more
- `app/models/catalog.py` ← `app/models/__init__.py`, `app/routers/admin_analytics.py`, `app/routers/alerts.py`, `app/routers/availability.py`, `app/routers/catalog.py` +63 more
- `app/routers/auth_simple.py` ← `app/main.py`, `app/middleware/execution.py`, `app/middleware/rbac.py`, `app/middleware/subscription.py`, `app/routers/activity.py` +56 more
- `app/services/availability_windows.py` ← `app/mcp_server.py`, `app/models/orderbook.py`, `app/models/rfq.py`, `app/routers/admin_analytics.py`, `app/routers/curves.py` +33 more
- `app/main.py` ← `app/services/audit_service.py`, `scripts/export_openapi.py`, `scripts/rust_contract/feedback_oracle.py`, `scripts/rust_contract/phase2_alert_oracle.py`, `scripts/rust_contract/phase2_auth_vectors.py` +31 more
- `app/schemas/market_activity.py` ← `app/routers/availability.py`, `app/routers/curves.py`, `app/routers/demand.py`, `app/routers/orderbook.py`, `app/routers/price_discovery.py` +28 more

---

# Test Coverage

> **43%** of routes and models are covered by tests
> 457 test files found

## Covered Routes

- GET:/
- GET:/health
- GET:/health/live
- GET:/health/ready
- GET:/api/users
- GET:/api/admin/onboarding-attention
- GET:
- GET:/products
- GET:/api/v1
- POST:/api/feedback
- GET:/api/admin/feedback
- GET:/status
- POST:/organizations/{organization_id}/listings/{order_id}/cancel
- POST:
- GET:/snapshot
- GET:/bids
- GET:/asks
- GET:/with-ci
- GET:/aggregated
- GET:/product-counts
- GET:/map-summary
- GET:/map-summary/compact
- GET:/regions
- GET:/fuel-types
- GET:/api
- GET:/api/prices
- GET:/api/orderbook
- GET:/api/trades
- POST:/
- GET:/.well-known/verdaxis-disposable-test
- GET:/owner-cleanup-proof
- GET:/one
- GET:/many
- POST:/upload
- GET:/api/admin/analytics/overview
- GET:/__contract__/live-benchmarks
- POST:/__contract__/phase2/write
- GET:/api/alerts
- POST:/api/alerts
- DELETE:/api/alerts/{alert_id}
- GET:/api/users/me/preferences
- PUT:/api/users/me/preferences/{namespace}
- POST:/api/referrals/my-code
- POST:/api/referrals/invite
- GET:/api/referrals/my-referrals
- HEAD:/api/referrals/my-referrals
- GET:/api/referrals/leaderboard
- HEAD:/api/referrals/leaderboard
- GET:/api/referrals/resolve/{code}
- HEAD:/api/referrals/resolve/{code}
- GET:/api/watchlists
- POST:/api/watchlists
- POST:/api/watchlists/me
- GET:/api/watchlists/{watchlist_id}
- DELETE:/api/watchlists/{watchlist_id}
- POST:/api/watchlists/{watchlist_id}/targets
- DELETE:/api/watchlists/{watchlist_id}/targets/{target_id}
- GET:/api/watchlists/{watchlist_id}/events
- PATCH:/api/watchlists/{watchlist_id}/events/{event_id}
- GET:/api/catalog/products
- GET:/api/catalog/delivery-points
- GET:/api/curves/forward/table
- GET:/api/curves/forward/slice
- GET:/api/curves/forward/board
- GET:/api/curves/forward
- GET:/api/curves/forward/export
- GET:/api/news
- GET:/api/orderbook/bids
- GET:/api/orderbook/asks
- POST:/api/orderbook
- GET:/api/orderbook/with-ci
- GET:/api/orderbook/aggregated
- GET:/api/orderbook/product-counts
- GET:/api/orderbook/map-summary
- GET:/api/orderbook/map-summary/compact
- GET:/api/orderbook/products
- GET:/api/orderbook/regions
- GET:/api/orderbook/fuel-types
- GET:/api/ports
- GET:/api/ports/{port_id}
- GET:/api/prices/reference
- GET:/api/prices/reference/export
- GET:/api/trade-tape
- GET:/api/notifications/unread-count
- POST:/emails
- GET:/fixture
- GET:/ok
- GET:/api/admin/analytics
- GET:/unhandled
- GET:/handled
- GET:/busy

## Covered Models

- PriceAlert
- AuditLog
- Benchmark
- Product
- DeliveryPoint
- MarketCommandResult
- MarketSignalIngestionRun
- MarketIndication
- FairPriceBand
- PhysicalStem
- KYCDocument
- KYCExecutionGrace
- KYCGraceNotice
- LiveSliceBenchmark
- MarketEventOutbox
- MarketSupportContext
- StaffCapabilityAssignment
- MarketSupportAuthorization
- InventoryItem
- MatchSuggestion
- Negotiation
- NegotiationRound
- NewsItem
- Notification
- OrderBookOrder
- Trade
- Commission
- Port
- Vessel
- ProducerProject
- UserLoginDay
- UserStatusTransition
- Referral
- RefreshSession
- PendingRegistration
- OrganizationJoinRequest
- RFQ
- RFQQuote
- SeedRun
- Subscription
- SupplierOffer
- Organization
- User
- UserPreference
- Watchlist
- WatchlistEntry
- WatchlistTarget
- WatchlistEvent
- AIChatRequest
- ActivityTrend
- AdminApproveBody
- AdminDecisionBody
- AdminRejectBody
- AggregateCell
- AggregatedOrderbookResponse
- AlertCreate
- AlertResponse
- AnalyticsCoverage
- AnalyticsDataQuality
- AnalyticsMeta
- AnalyticsSourceCoverage
- AuthorizationCreate
- BehavioralUsage
- CIAdjustedPrice
- DeliveryPointResponse
- DemandSignal
- FamePublicAskTerms
- FameQualitySummary
- FameSustainabilitySummary
- FeeScheduleResponse
- FeedbackSubmission
- ForwardCurveBoardCell
- ForwardCurveBoardDepthLevel
- ForwardCurveBoardFairPriceBand
- ForwardCurveBoardFocus
- ForwardCurveBoardIndication
- ForwardCurveBoardIndicationSummary
- ForwardCurveBoardPort
- ForwardCurveBoardProduct
- ForwardCurveBoardResponse
- ForwardCurveMarketCell
- ForwardCurvePoint
- ForwardCurveResponse
- ForwardCurveSignalProvenance
- ForwardCurveSliceResponse
- ForwardCurveTableCell
- ForwardCurveTableResponse
- InventoryCreate
- InventoryItemUpdate
- JoinReviewBody
- LeaderboardEntry
- LngPublicAskTerms
- MapCompactSummaryResponse
- MapSummaryResponse
- MarketSupportContextCreate
- MarketSupportFinalConfirmation
- NegotiationCounterRequest
- NegotiationCreateRequest
- NotificationResponse
- OrderBookSnapshotResponse
- OrderMyResponse
- OrderResponse
- OrderResponseWithCI
- OrderUpdate
- OrganizationCreate
- PasswordChangeRequest
- PinTargetCreate
- PortFuelAvailability
- PortResponse
- PriceDiscoveryResponse
- PriceSummary
- PricingOverlayResponse
- ProducerProjectResponse
- ProductCountResponse
- ProductResponse
- ProductUsageResponse
- RFQCreateRequest
- RFQListResponse
- RFQQuoteRequest
- RFQQuoteResponse
- RFQQuoteRevisionRequest
- RFQResponse
- ReferenceCoverageRow
- ReferencePriceItem
- ReferencePriceResponse
- ReferralCodeResponse
- ReferralInviteRequest
- ReferralListItem
- RegistrationResponse
- ResolveCodeResponse
- SeriesPoint
- SliceTargetCreate
- SubscriptionResponse
- SubscriptionUpdate
- SupplierOfferCreate
- SupplierOfferTerms
- SurveySubmission
- TradeCreate
- TradeDeliverPayload
- TradeResponse
- TradeTapeEntry
- TradeTapeResponse
- UserCreate
- UserResponse
- UserUpdate
- ValidationError
- WatchlistEntryAddRequest
- WatchlistSummaryResponse

---

# CI/CD Pipelines

## GitHub Actions (2 workflows)

| Workflow | Triggers | Jobs | Deploy | Environments |
|---|---|---|---|---|
| Verdaxis Backend CI | push, pull_request | 3 | — | — |
| Rust read CI | pull_request, push, workflow_dispatch | 3 | — | — |

### Verdaxis Backend CI

> `.github/workflows/backend-ci.yml`

- **test** on `ubuntu-latest` — 6 steps
  - `actions/checkout@v4`
  - `actions/setup-python@v5`
- **postgres-analytics** on `ubuntu-latest` — 5 steps
  - `actions/checkout@v4`
  - `actions/setup-python@v5`
- **postgres-phase0** on `ubuntu-latest` — 4 steps
  - `actions/checkout@v4`
  - `actions/setup-python@v5`

### Rust read CI

> `.github/workflows/rust-read-ci.yml`

- **offline** on `ubuntu-22.04` — 15 steps
  - `actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09`
  - `actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830`
  - `actions/setup-python@ece7cb06caefa5fff74198d8649806c4678c61a1`
  - `actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02`
- **contract** on `ubuntu-22.04` — 16 steps (needs: offline)
  - `actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09`
  - `actions/setup-python@ece7cb06caefa5fff74198d8649806c4678c61a1`
  - `actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830`
  - `actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093`
  - `actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02`
- **publish** on `ubuntu-22.04` — 7 steps (needs: offline, contract)
  - `actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09`
  - `actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093`
  - `actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093`
  - `actions/attest-build-provenance@e8998f949152b193b063cb0ec769d69d929409be`
  - `actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02`

---
_Source: .github/workflows/backend-ci.yml, .github/workflows/rust-read-ci.yml_
_Generated by codesight-cicd-plugin_

---

_Generated by [codesight](https://github.com/Houseofmvps/codesight) — see your codebase clearly_