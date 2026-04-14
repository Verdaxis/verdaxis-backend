# be — AI Context Map

> **Stack:** fastapi | sqlalchemy | unknown | python

> 109 routes | 30 models | 0 components | 56 lib files | 18 env vars | 4 middleware | 23% test coverage
> **Token savings:** this file is ~8,000 tokens. Without it, AI exploration would cost ~91,500 tokens. **Saves ~83,500 tokens per conversation.**
> **Last scanned:** 2026-04-14 06:31 — re-run after significant changes

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
- `GET` `/activity` params() [auth, cache, queue]
- `GET` `/overview` params() → in: Annotated, out: OverviewResponse [auth, db]
- `GET` `/daily` params() → in: Annotated, out: OverviewResponse [auth, db]
- `POST` `/ai/chat` params() [auth]
- `GET` `/admin/audit-logs` params() → in: Annotated, out: list [auth, db]
- `POST` `/api/login` params() → out: RegistrationResponse [auth, db, email]
- `POST` `/api/refresh` params() → out: RegistrationResponse [auth, db, email]
- `POST` `/api/logout` params() → out: RegistrationResponse [auth, db, email]
- `POST` `/api/register` params() → out: RegistrationResponse [auth, db, email]
- `POST` `/api/register-with-org` params() → out: RegistrationResponse [auth, db, email]
- `GET` `/api/verify-email` params() → out: RegistrationResponse [auth, db, email]
- `POST` `/api/resend-verification` params() → out: RegistrationResponse [auth, db, email]
- `POST` `/api/resend-verification-email` params() → out: RegistrationResponse [auth, db, email]
- `GET` `/api/me` params() → out: RegistrationResponse [auth, db, email]
- `PUT` `/api/me` params() → in: UserUpdate, out: RegistrationResponse [auth, db, email]
- `PUT` `/api/me/password` params() → in: UserUpdate, out: RegistrationResponse [auth, db, email]
- `POST` `/api/forgot-password` params() → out: RegistrationResponse [auth, db, email]
- `POST` `/api/reset-password` params() → out: RegistrationResponse [auth, db, email]
- `PUT` `/api/approve/{user_id}` params(user_id) → in: UserUpdate, out: RegistrationResponse [auth, db, email]
- `PUT` `/api/switch-role/{target_role}` params(target_role) → in: UserUpdate, out: RegistrationResponse [auth, db, email]
- `GET` `/products` params() → in: AsyncSessio, out: list [auth, db]
- `GET` `/delivery-points` params() → in: AsyncSessio, out: list [auth, db]
- `GET` `/compliance/ledger` params() → in: Annotated, out: List [auth, db, upload]
- `POST` `/compliance/verify` params() → in: Annotated, out: List [auth, db, upload]
- `GET` `/vessels/{vessel_id}/score` params(vessel_id) → out: ComplianceScoreResponse [auth, db]
- `GET` `/fleet` params() → out: ComplianceScoreResponse [auth, db]
- `POST` `/scenario` params() → in: ScenarioInput, out: ComplianceScoreResponse [auth, db]
- `GET` `/fuels` params() → out: ComplianceScoreResponse [auth, db]
- `GET` `/api/v1` params() → in: UUI, out: ForwardCurveResponse [auth, db] ✓
- `GET` `/api/v1/export` params() → in: UUI, out: ForwardCurveResponse [auth, db]
- `GET` `/logs` params() → out: SystemHealth
- `POST` `/inventory/{item_id}/publish` params(item_id) → in: InventoryCreate, out: List [auth, db]
- `GET` `/listings` params() → in: Annotated, out: List [auth, db]
- `GET` `/listings/my` params() → in: Annotated, out: List [auth, db]
- `POST` `/submit` params() → in: Annotated [auth, db, upload]
- `GET` `/status` params() → in: Annotated [auth, db, upload]
- `PUT` `/admin/{user_id}/approve` params(user_id) → in: uuid [auth, db, upload]
- `PUT` `/admin/{user_id}/reject` params(user_id) → in: uuid [auth, db, upload]
- `GET` `/suggestions` params() → in: Annotated [auth, db]
- `PATCH` `/suggestions/{order_id}/dismiss` params(order_id) → in: UUID [auth, db]
- `POST` `/{negotiation_id}/counter` params(negotiation_id) → out: NegotiationResponse [auth, db]
- `POST` `/{negotiation_id}/accept` params(negotiation_id) → out: NegotiationResponse [auth, db]
- `POST` `/{negotiation_id}/decline` params(negotiation_id) → out: NegotiationResponse [auth, db]
- `POST` `/refresh` params() → in: AsyncSessio [auth, db]
- `GET` `/unread-count` params() → in: in, out: List [auth, db]
- `PATCH` `/{notification_id}/read` params(notification_id) → in: uuid, out: List [auth, db]
- `PATCH` `/read-all` params() → in: uuid, out: List [auth, db]
- `GET` `/bids` params() → in: Optional, out: PaginatedResponse [auth, db]
- `GET` `/asks` params() → in: Optional, out: PaginatedResponse [auth, db]
- `GET` `/with-ci` params() → in: Optional, out: PaginatedResponse [auth, db]
- `GET` `/my` params() → in: Optional, out: PaginatedResponse [auth, db]
- `GET` `/my/latest-ask-template` params() → in: Optional, out: PaginatedResponse [auth, db]
- `GET` `/aggregated` params() → in: Optional, out: PaginatedResponse [auth, db]
- `GET` `/regions` params() → in: Optional, out: PaginatedResponse [auth, db]
- `GET` `/fuel-types` params() → in: Optional, out: PaginatedResponse [auth, db]
- `GET` `/admin/commissions` params() → in: Use, out: list [auth, db]
- `GET` `/admin/commissions/summary` params() → in: Use, out: list [auth, db]
- `PUT` `/admin/commissions/{commission_id}` params(commission_id) → in: UUID, out: list [auth, db]
- `GET` `/ports` params() → in: Annotated, out: List [auth, db]
- `GET` `/ports/{port_id}` params(port_id) → in: Annotated, out: List [auth, db]
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
- changes: JSONB (nullable)
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
- created_at: DateTime (default)
- email_verified: Boolean (default)
- email_verification_token: String (nullable)
- kyc_status: String (default)
- kyc_rejection_reason: Text (nullable)
- password_reset_token_hash: String (nullable)
- password_reset_expires: DateTime (nullable)
- referral_code: String (unique, nullable)
- referred_by_id: UUID (fk, nullable)
- _relations_: organization: , referrals_made: , referral_received: 

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
- `alembic/versions/fk_orderbook_2026_03_orderbook_product_dp_fks.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/g6h7i8j9k0l1_add_audit_logs_table.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/h7i8j9k0l1m2_add_email_verification_and_kyc_fields.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ix_2026_04_exec_watchlist_perf.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/neg_2026_04_add_negotiations.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/neg_2026_04b_negotiation_fixes.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/news_2026_03_add_news_items.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ob_2026_04_availability_windows.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ob_2026_04_supplier_listing_metadata.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/pw_reset_2026_03_add_password_reset_fields.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/ref_2026_03_add_referrals.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/rfq_2026_03_add_rfq_tables.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/sub_2026_03_add_subscriptions.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/wl_2026_03_add_watchlists.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/wl_2026_04_market_radar.py` — function upgrade: () -> None, function downgrade: () -> None
- `alembic/versions/wl_2026_04_watchlist_entry_uniqueness.py` — function upgrade: () -> None, function downgrade: () -> None
- `find_buyer.py` — function check_buyers: ()
- `fix_seller_status.py` — function fix_user: ()
- `reset_buyer_password.py` — function reset_password: ()
- `scripts/check_users.py` — function main: ()
- `scripts/import_gena_csv.py` — function import_csv: (file_path, fuel_type)
- `scripts/scrape_fleet_demand.py`
  - function run_batch: (commands, timeout) -> str
  - function extract_last_value: (raw) -> str
  - function get_afi_article_url: () -> str | None
  - function get_page_body: (url) -> str
  - function extract_int: (text, patterns) -> int | None
  - function scrape: ()
- `scripts/seed.py` — function main: () -> None
- `scripts/seed_compliance_data.py` — function add_entry: (org_key, transaction_type, amount, currency, units, description, reference_id, created_at)
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
- `scripts/test_purchase_flow.py` — function create_local_token: (email, role, user_id), function main: ()

---

# Config

## Environment Variables

- `ADMIN_PASSWORD` (has default) — .env.example
- `ADMIN_SESSION_SECRET` (has default) — .env.example
- `ADMIN_USERNAME` (has default) — .env.example
- `BACKEND_CORS_ORIGINS` (has default) — .env
- `DATABASE_HOST` (has default) — .env.example
- `DATABASE_NAME` (has default) — .env.example
- `DATABASE_PASSWORD` (has default) — .env.example
- `DATABASE_PORT` (has default) — .env.example
- `DATABASE_USER` (has default) — .env.example
- `ENABLE_AUTH_BYPASS` (has default) — .env.example
- `ENVIRONMENT` **required** — app/config.py
- `FRONTEND_URL` (has default) — .env
- `GEMINI_API_KEY` **required** — .env.example
- `JWT_ALGORITHM` (has default) — .env.example
- `JWT_SECRET` (has default) — .env.example
- `PYTHONPATH` (has default) — .env.example
- `RESEND_API_KEY` (has default) — .env
- `TEST_API_URL` **required** — tests/conftest.py

## Config Files

- `.env.example`
- `Dockerfile`
- `docker-compose.yml`
- `pyproject.toml`

---

# Middleware

## auth
- rbac — `app/middleware/rbac.py`
- auth_simple — `app/routers/auth_simple.py`

## custom
- subscription — `app/middleware/subscription.py`

## rate-limit
- rate_limit — `app/rate_limit.py`

---

# Test Coverage

> **23%** of routes and models are covered by tests
> 61 test files found

## Covered Routes

- GET:/
- GET:/health
- GET:/health/live
- GET:/health/ready
- GET:
- GET:/api/v1
- POST:
- POST:/

## Covered Models

- PriceAlert
- Benchmark
- Product
- DeliveryPoint
- InventoryItem
- MatchSuggestion
- NewsItem
- Notification
- OrderBookOrder
- Trade
- Commission
- Port
- Vessel
- ProducerProject
- Referral
- RFQ
- RFQQuote
- Subscription
- Organization
- User
- Watchlist
- WatchlistEntry
- WatchlistTarget
- WatchlistEvent

---

_Generated by [codesight](https://github.com/Houseofmvps/codesight) — see your codebase clearly_