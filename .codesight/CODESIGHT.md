# be — AI Context Map

> **Stack:** fastapi | sqlalchemy | unknown | python

> 129 routes | 38 models | 0 components | 66 lib files | 43 env vars | 7 middleware | 27% test coverage
> **Token savings:** this file is ~10,600 tokens. Without it, AI exploration would cost ~111,700 tokens. **Saves ~101,000 tokens per conversation.**
> **Last scanned:** 2026-07-19 22:35 — re-run after significant changes

---

# Routes

## CRUD Resources

- **``** GET | POST | GET/:id | PUT/:id | DELETE/:id
- **`/inventory`** GET | POST | GET/:id | PATCH/:id | DELETE/:id → Inventory
- **`/admin/subscriptions`** GET | GET/:id | PUT/:id → Subscription

## Other Routes

- `GET` `/` params() [auth, db, cache] ✓
- `GET` `/health` params() [auth, db, cache] ✓
- `GET` `/health/live` params() [auth, db, cache] ✓
- `GET` `/health/ready` params() [auth, db, cache] ✓
- `POST` `/endpoint` → in: Annotated [auth]
- `GET` `/premium-feature` → in: Subscriptio [auth, db]
- `GET` `/activity` params() [auth, db, cache, queue]
- `GET` `/api/product-usage` params() → out: ProductUsageResponse [auth, db]
- `GET` `/api/overview` params() → out: ProductUsageResponse [auth, db]
- `GET` `/api/daily` params() → out: ProductUsageResponse [auth, db]
- `GET` `/api/users` params() → out: ProductUsageResponse [auth, db] ✓
- `PUT` `/api/users/{user_id}/reject` params(user_id) → out: ProductUsageResponse [auth, db]
- `GET` `/api/product-analytics/overview` params() → out: ProductUsageResponse [auth, db]
- `GET` `/api/product-analytics/acquisition` params() → out: ProductUsageResponse [auth, db]
- `GET` `/api/product-analytics/activation` params() → out: ProductUsageResponse [auth, db]
- `GET` `/api/product-analytics/engagement` params() → out: ProductUsageResponse [auth, db]
- `GET` `/api/product-analytics/marketplace` params() → out: ProductUsageResponse [auth, db]
- `GET` `/api/product-analytics/retention` params() → out: ProductUsageResponse [auth, db]
- `GET` `/api/product-analytics/reliability` params() → out: ProductUsageResponse [auth, db]
- `POST` `/ai/chat` params() [auth]
- `GET` `/admin/audit-logs` params() → in: Annotated, out: list [auth, db]
- `POST` `/api/login` params() → out: RegistrationResponse [auth, db, cache, email]
- `POST` `/api/refresh` params() → out: RegistrationResponse [auth, db, cache, email]
- `POST` `/api/logout` params() → out: RegistrationResponse [auth, db, cache, email]
- `GET` `/api/stream-token` params() → out: RegistrationResponse [auth, db, cache, email]
- `POST` `/api/register` params() → out: RegistrationResponse [auth, db, cache, email]
- `POST` `/api/register-with-org` params() → out: RegistrationResponse [auth, db, cache, email]
- `GET` `/api/verify-email` params() → out: RegistrationResponse [auth, db, cache, email]
- `POST` `/api/resend-verification` params() → out: RegistrationResponse [auth, db, cache, email]
- `POST` `/api/resend-verification-email` params() → out: RegistrationResponse [auth, db, cache, email]
- `GET` `/api/me` params() → out: RegistrationResponse [auth, db, cache, email]
- `PUT` `/api/me` params() → in: UserUpdate, out: RegistrationResponse [auth, db, cache, email]
- `PUT` `/api/me/password` params() → in: UserUpdate, out: RegistrationResponse [auth, db, cache, email]
- `POST` `/api/forgot-password` params() → out: RegistrationResponse [auth, db, cache, email]
- `POST` `/api/reset-password` params() → out: RegistrationResponse [auth, db, cache, email]
- `PUT` `/api/approve/{user_id}` params(user_id) → in: UserUpdate, out: RegistrationResponse [auth, db, cache, email]
- `PUT` `/api/switch-role/{target_role}` params(target_role) → in: UserUpdate, out: RegistrationResponse [auth, db, cache, email]
- `POST` `/api/survey` params() → out: RegistrationResponse [auth, db, cache, email]
- `GET` `/products` params() → in: AsyncSessio, out: list [auth, db]
- `GET` `/delivery-points` params() → in: AsyncSessio, out: list [auth, db]
- `GET` `/compliance/ledger` params() → in: Annotated, out: List [auth, db, upload]
- `POST` `/compliance/verify` params() → in: Annotated, out: List [auth, db, upload]
- `GET` `/api/vessels/{vessel_id}/score` params(vessel_id) → out: ComplianceScoreResponse [auth, db]
- `GET` `/api/fleet` params() → out: ComplianceScoreResponse [auth, db]
- `POST` `/api/scenario` params() → in: ScenarioInput, out: ComplianceScoreResponse [auth, db]
- `POST` `/api/pricing-overlay` params() → in: ScenarioInput, out: ComplianceScoreResponse [auth, db]
- `GET` `/api/fuels` params() → out: ComplianceScoreResponse [auth, db]
- `GET` `/api/v1/table` params() → in: Optional, out: ForwardCurveTableResponse [auth, db]
- `GET` `/api/v1/slice` params() → in: Optional, out: ForwardCurveTableResponse [auth, db]
- `GET` `/api/v1/board` params() → in: Optional, out: ForwardCurveTableResponse [auth, db]
- `GET` `/api/v1` params() → in: Optional, out: ForwardCurveTableResponse [auth, db] ✓
- `GET` `/api/v1/export` params() → in: Optional, out: ForwardCurveTableResponse [auth, db]
- `GET` `/logs` params() → out: SystemHealth
- `POST` `/inventory/{item_id}/publish` params(item_id) → in: InventoryCreate, out: List [auth, db]
- `GET` `/listings` params() → in: Annotated, out: List [auth, db]
- `GET` `/listings/my` params() → in: Annotated, out: List [auth, db]
- `POST` `/submit` params() [auth, db, upload]
- `GET` `/status` params() → in: Annotated [auth, db, upload]
- `PUT` `/admin/{user_id}/approve` params(user_id) → in: uuid [auth, db, upload]
- `PUT` `/admin/{user_id}/reject` params(user_id) → in: uuid [auth, db, upload]
- `GET` `/suggestions` params() → in: Annotated [auth, db]
- `PATCH` `/suggestions/{order_id}/dismiss` params(order_id) → in: UUID [auth, db]
- `POST` `/signup-canary-cleanup` params() → in: CanaryCleanupRequest, out: CanaryCleanupResponse [auth, db]
- `POST` `/{negotiation_id}/counter` params(negotiation_id) → out: NegotiationResponse [auth, db]
- `POST` `/{negotiation_id}/accept` params(negotiation_id) → out: NegotiationResponse [auth, db]
- `POST` `/{negotiation_id}/decline` params(negotiation_id) → out: NegotiationResponse [auth, db]
- `POST` `/refresh` params() → in: AsyncSessio [auth, db]
- `GET` `/unread-count` params() → in: in, out: List [auth, db]
- `PATCH` `/{notification_id}/read` params(notification_id) → in: uuid, out: List [auth, db]
- `PATCH` `/read-all` params() → in: uuid, out: List [auth, db]
- `GET` `/bids` params() → in: Optional, out: PaginatedResponse [auth, db, cache]
- `GET` `/asks` params() → in: Optional, out: PaginatedResponse [auth, db, cache]
- `GET` `/with-ci` params() → in: Optional, out: PaginatedResponse [auth, db, cache]
- `GET` `/my` params() → in: Optional, out: PaginatedResponse [auth, db, cache]
- `GET` `/my/latest-ask-template` params() → in: Optional, out: PaginatedResponse [auth, db, cache]
- `GET` `/aggregated` params() → in: Optional, out: PaginatedResponse [auth, db, cache]
- `GET` `/regions` params() → in: Optional, out: PaginatedResponse [auth, db, cache]
- `GET` `/fuel-types` params() → in: Optional, out: PaginatedResponse [auth, db, cache]
- `GET` `/admin/commissions` params() → in: Use, out: list [auth, db]
- `GET` `/admin/commissions/summary` params() → in: Use, out: list [auth, db]
- `PUT` `/admin/commissions/{commission_id}` params(commission_id) → in: UUID, out: list [auth, db]
- `GET` `/ports` params() → in: Annotated, out: List [auth, db]
- `GET` `/ports/{port_id}` params(port_id) → in: Annotated, out: List [auth, db]
- `GET` `/api` params() → in: Use [auth, db] ✓
- `PUT` `/api/{namespace}` params(namespace) [auth, db]
- `GET` `/reference` params() → out: PriceDiscoveryResponse [auth, db]
- `GET` `/reference/export` params() → out: PriceDiscoveryResponse [auth, db]
- `GET` `/my-code` params() → in: Annotated, out: ReferralCodeResponse [auth, db]
- `GET` `/my-referrals` params() → in: Annotated, out: ReferralCodeResponse [auth, db]
- `GET` `/leaderboard` params() → in: Annotated, out: ReferralCodeResponse [auth, db]
- `POST` `/invite` params() → out: ReferralCodeResponse [auth, db]
- `GET` `/resolve/{code}` params(code) → in: Annotated, out: ReferralCodeResponse [auth, db]
- `POST` `/{rfq_id}/quote` params(rfq_id) → out: RFQResponse [auth, db]
- `POST` `/{rfq_id}/accept/{quote_id}` params(rfq_id, quote_id) → out: RFQResponse [auth, db]
- `POST` `/{rfq_id}/cancel` params(rfq_id) → out: RFQResponse [auth, db]
- `GET` `/prices` params() [auth, cache, queue]
- `GET` `/orderbook` params() [auth, cache, queue]
- `GET` `/trades` params() [auth, cache, queue]
- `GET` `/subscriptions/me` params() → in: Annotated, out: SubscriptionResponse [auth, db]
- `POST` `/` params() → in: TradeCreate, out: TradeResponse [auth, db] ✓
- `PUT` `/{trade_id}/confirm` params(trade_id) → in: UUID, out: TradeResponse [auth, db]
- `PUT` `/{trade_id}/decline` params(trade_id) → in: UUID, out: TradeResponse [auth, db]
- `PUT` `/{trade_id}/deliver` params(trade_id) → in: UUID, out: TradeResponse [auth, db]
- `POST` `/{trade_id}/pay` params(trade_id) → in: TradeCreate, out: TradeResponse [auth, db]
- `GET` `/vessels` params() → in: Annotated, out: List [auth, db]
- `GET` `/vessels/{vessel_id}` params(vessel_id) → in: Annotated, out: List [auth, db]
- `GET` `/me` params() → in: Annotated, out: list [auth, db]
- `POST` `/{watchlist_id}/targets` params(watchlist_id) → in: WatchlistCreateRequest, out: list [auth, db]
- `DELETE` `/{watchlist_id}/targets/{target_id}` params(watchlist_id, target_id) → in: UUID, out: list [auth, db]
- `GET` `/{watchlist_id}/events` params(watchlist_id) → in: Annotated, out: list [auth, db]
- `PATCH` `/{watchlist_id}/events/{event_id}` params(watchlist_id, event_id) → in: UUID, out: list [auth, db]
- `POST` `/{watchlist_id}/entries` params(watchlist_id) → in: WatchlistCreateRequest, out: list [auth, db]
- `DELETE` `/{watchlist_id}/entries/{entry_id}` params(watchlist_id, entry_id) → in: UUID, out: list [auth, db]
- `GET` `/api/admin/analytics/overview` params() [auth, db] ✓

---

# Schema

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

### TraceabilityEvent
- id: UUID (pk, default)
- direct_order_id: unknown (fk)
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

### InventoryItem
- id: UUID (pk, default)
- supplier_id: unknown (fk)
- port_id: unknown (fk)
- fuel_type: Enum
- product_name: String
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
- initiator_side: String
- product_id: UUID (fk)
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
- organization_id: unknown (fk)
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
- created_at: DateTime (default)
- updated_at: DateTime (default)
- _relations_: organization: Organization, product: Product, delivery_point: DeliveryPoint, vessel: Vessel, bid_trades: Trade, ask_trades: Trade

### Trade
- id: UUID (pk, default)
- bid_order_id: unknown (fk, nullable)
- ask_order_id: unknown (fk, nullable)
- buyer_id: unknown (fk)
- seller_id: unknown (fk)
- initiated_by: Enum
- is_anonymous: Boolean (default)
- quantity_mt: Numeric
- price_per_mt_usd: Numeric
- status: Enum (default)
- final_quantity_mt: Numeric (nullable)
- final_price_per_mt: Numeric (nullable)
- final_total_usd: Numeric (nullable)
- commission_rate_pct: Numeric (default)
- commission_amount_usd: Numeric (nullable)
- confirmed_at: DateTime (nullable)
- delivered_at: DateTime (nullable)
- paid_at: DateTime (nullable)
- created_at: DateTime (default)
- _relations_: bid_order: OrderBookOrder, ask_order: OrderBookOrder, buyer: Organization, seller: Organization, commission: Commission

### Commission
- id: UUID (pk, default)
- match_id: unknown (fk, unique)
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

### RFQ
- id: UUID (pk, default)
- buyer_org_id: UUID (fk)
- product_id: UUID (fk)
- delivery_point_id: UUID (fk, nullable)
- quantity_mt: Numeric
- target_price_per_mt: Numeric (nullable)
- availability_window: String (default)
- notes: Text (nullable)
- is_anonymous: Boolean (default)
- status: Enum (default)
- expires_at: DateTime
- created_at: DateTime (default)
- _relations_: quotes: 

### RFQQuote
- id: UUID (pk, default)
- rfq_id: UUID (fk)
- seller_org_id: UUID (fk)
- price_per_mt_usd: Numeric
- notes: Text (nullable)
- status: Enum (default)
- created_at: DateTime (default)
- _relations_: rfq: 

### Subscription
- id: UUID (pk, default)
- org_id: UUID (fk, unique)
- tier: String (default)
- started_at: DateTime (nullable, default)
- expires_at: DateTime (nullable, default)
- is_active: Boolean (default)
- _relations_: organization: Organization

### Organization
- id: UUID (pk, default)
- name: String
- domain: String (unique, nullable)
- type: Enum
- supplier_tier: Enum (nullable, default)
- tax_id: String
- country_code: String
- verification_status: String (default)
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
- organization_id: unknown (fk)
- last_login: DateTime
- password_changed_at: DateTime (nullable)
- must_change_password: Boolean (default)
- created_at: DateTime (default)
- email_verified: Boolean (default)
- email_verification_token: String (nullable)
- kyc_status: String (default)
- kyc_rejection_reason: Text (nullable)
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

---

# Libraries

- `alembic/env.py`
  - function run_migrations_offline: () -> None
  - function do_run_migrations: (connection) -> None
  - function run_migrations_online: () -> None
  - function run_async_migrations: () -> None
- `alembic/versions/148fa1ffb191_add_notifications_table.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/14b159f248ac_rename_quote_request_to_direct_order.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/377c3f6f9aef_initial_schema.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/411b38418aea_add_user_interaction_status.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/63cbf0775d4d_add_domain_to_organization.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/a1b2c3d4e5f6_unified_orderbook_migration.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/alerts_2026_03_add_price_alerts.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/anon_trade_2026_03_add_trade_is_anonymous.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/auth_2026_07_add_must_change_password.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/b1c2d3e4f5g6_add_ci_fields_to_orderbook.py` — function upgrade: () -> None, function downgrade: () -> None
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
- `alembic/versions/fc_2026_06_monitor_signals.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/fk_orderbook_2026_03_orderbook_product_dp_fks.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/g6h7i8j9k0l1_add_audit_logs_table.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/h7i8j9k0l1m2_add_email_verification_and_kyc_fields.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ix_2026_04_exec_watchlist_perf.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/neg_2026_04_add_negotiations.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/neg_2026_04b_negotiation_fixes.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/news_2026_03_add_news_items.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ob_2026_04_availability_windows.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ob_2026_04_supplier_listing_metadata.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/pa_20260715_add_product_analytics_facts.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/pref_20260709_add_user_preferences.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/pw_reset_2026_03_add_password_reset_fields.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ref_2026_03_add_referrals.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/rfq_2026_03_add_rfq_tables.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/rh_20260720_runtime_metadata.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/sb_2026_04_live_slice_benchmarks.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/sub_2026_03_add_subscriptions.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/usr_2026_04_onboarding_survey.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/wl_2026_03_add_watchlists.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/wl_2026_04_market_radar.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/wl_2026_04_watchlist_entry_uniqueness.py` — function upgrade: () -> None, function downgrade: () -> None
- `scripts/benchmark_product_analytics.py` — function main: () -> int
- `scripts/check_users.py` — function main: ()
- `scripts/explain_product_analytics.py` — function main: () -> int, function run: (days, output) -> int
- `scripts/import_gena_csv.py` — function import_csv: (file_path, fuel_type)
- `scripts/ingest_market_signals.py`
  - function assert_staging_runtime: () -> None
  - function assert_expected_database: (expected_name) -> None
  - function read_rows: (file_path) -> list[dict]
  - function print_report: (report) -> None
  - function main: () -> None
  - function run: (args) -> int
- `scripts/prune_product_analytics.py`
  - function compute_cutoff: (today) -> date
  - function prune_login_days: (session, *, today) -> int
  - function main: () -> int
- `scripts/run_demo_activity.py` — function main: () -> None
- `scripts/scrape_fleet_demand.py`
  - function run_batch: (commands, timeout) -> str
  - function extract_last_value: (raw) -> str
  - function get_afi_article_url: () -> str | None
  - function get_page_body: (url) -> str
  - function extract_int: (text, patterns) -> int | None
  - function scrape: ()
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
- `scripts/seed_trade_tape.py`
  - function get_connection: ()
  - function clean_existing_trades: (conn)
  - function generate_trade_dates: (num_trades, start_date, end_date)
  - function get_status_for_date: (trade_date, end_date)
  - function get_price_with_drift: (product_name, min_price, max_price, trade_date, start_date, end_date)
  - function select_product: ()
  - _...4 more_
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

---

# Config

## Environment Variables

- `ADMIN_PASSWORD` (has default) — .env.example
- `ADMIN_SESSION_SECRET` (has default) — .env.example
- `ADMIN_USERNAME` (has default) — .env.example
- `ANALYTICS_ENABLED` (has default) — .env.example
- `ANALYTICS_REQUEST_TIMEOUT_SECONDS` (has default) — .env.example
- `BACKEND_CORS_ORIGINS` (has default) — .env.example
- `DATABASE_HOST` (has default) — .env.example
- `DATABASE_NAME` (has default) — .env.example
- `DATABASE_PASSWORD` (has default) — .env.example
- `DATABASE_PORT` (has default) — .env.example
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
- `ENABLE_AUTH_BYPASS` (has default) — .env.example
- `ENVIRONMENT` (has default) — .env.example
- `FRONTEND_URL` (has default) — .env.example
- `GEMINI_API_KEY` **required** — .env.example
- `HEALTH_READINESS_TIMEOUT_SECONDS` (has default) — .env.example
- `ITEST_PASSWORD` **required** — scripts/benchmark_product_analytics.py
- `JWT_ALGORITHM` (has default) — .env.example
- `JWT_SECRET` (has default) — .env.example
- `KYC_MAX_FILE_BYTES` (has default) — .env.example
- `KYC_MAX_TOTAL_BYTES` (has default) — .env.example
- `MIGRATOR_DATABASE_URL` **required** — .env.example
- `MIGRATOR_IDLE_IN_TRANSACTION_SESSION_TIMEOUT_MS` (has default) — .env.example
- `MIGRATOR_LOCK_TIMEOUT_MS` (has default) — .env.example
- `MIGRATOR_STATEMENT_TIMEOUT_MS` (has default) — .env.example
- `PATH` **required** — tests/unit/test_runtime_hardening.py
- `PYTHONPATH` (has default) — .env.example
- `RELEASE_SHA` (has default) — .env.example
- `UMAMI_API_PASSWORD` **required** — .env.example
- `UMAMI_API_USERNAME` **required** — .env.example
- `UMAMI_BASE_URL` (has default) — .env.example
- `UMAMI_WEBSITE_ID` **required** — .env.example
- `UVICORN_WORKERS` (has default) — .env.example

## Config Files

- `.env.example`
- `Dockerfile`
- `docker-compose.yml`
- `pyproject.toml`

---

# Middleware

## auth
- auth_2026_07_add_must_change_password — `alembic/versions/auth_2026_07_add_must_change_password.py`
- preauth_rate_limit — `app/middleware/preauth_rate_limit.py`
- rbac — `app/middleware/rbac.py`
- auth_simple — `app/routers/auth_simple.py`
- test_preauth_rate_limit — `tests/unit/test_preauth_rate_limit.py`

## custom
- subscription — `app/middleware/subscription.py`

## rate-limit
- rate_limit — `app/rate_limit.py`

---

# Test Coverage

> **27%** of routes and models are covered by tests
> 101 test files found

## Covered Routes

- GET:/
- GET:/health
- GET:/health/live
- GET:/health/ready
- GET:/api/users
- GET:
- GET:/api/v1
- POST:
- GET:/api
- POST:/
- GET:/api/admin/analytics/overview

## Covered Models

- PriceAlert
- AuditLog
- Benchmark
- Product
- DeliveryPoint
- MarketSignalIngestionRun
- MarketIndication
- FairPriceBand
- PhysicalStem
- LiveSliceBenchmark
- InventoryItem
- MatchSuggestion
- Negotiation
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
- RFQ
- RFQQuote
- Subscription
- Organization
- User
- UserPreference
- Watchlist
- WatchlistEntry
- WatchlistTarget
- WatchlistEvent

---

# CI/CD Pipelines

## GitHub Actions (1 workflow)

| Workflow | Triggers | Jobs | Deploy | Environments |
|---|---|---|---|---|
| Verdaxis Backend CI | push, pull_request | 2 | — | — |

### Verdaxis Backend CI

> `.github/workflows/backend-ci.yml`

- **test** on `ubuntu-latest` — 5 steps
  - `actions/checkout@v4`
  - `actions/setup-python@v5`
- **postgres-analytics** on `ubuntu-latest` — 4 steps
  - `actions/checkout@v4`
  - `actions/setup-python@v5`

---
_Source: .github/workflows/backend-ci.yml_
_Generated by codesight-cicd-plugin_

---

_Generated by [codesight](https://github.com/Houseofmvps/codesight) — see your codebase clearly_