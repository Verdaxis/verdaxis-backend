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
- status: Enum (default)
- product_id: UUID (fk)
- delivery_point_id: UUID (fk)
- availability_window: String
- quantity_mt: Numeric
- price_per_mt_usd: Numeric
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
- terms_digest: String
- evidence_reference: String
- evidence_sha256: String
- commercial_consent_version: String
- commercial_consent_reference: String
- support_case_reference: String
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
- _relations_: organization: Organization

### Organization
- id: UUID (pk, default)
- name: String
- domain: String (unique, nullable)
- type: Enum
- supplier_tier: Enum (nullable, default)
- tax_id: String
- country_code: String
- verification_status: String
- provenance: Enum (default)
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
