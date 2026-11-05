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
