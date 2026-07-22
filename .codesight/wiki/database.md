# Database

> **Navigation aid.** Schema shapes and field types extracted via AST. Read the actual schema source files before writing migrations or query logic.

**sqlalchemy** — 47 models

### PriceAlert

pk: `id` (UUID) · fk: org_id, product_id, delivery_point_id

- `id`: UUID _(pk, default)_
- `org_id`: UUID _(fk)_
- `product_id`: UUID _(fk)_
- `delivery_point_id`: UUID _(fk, nullable)_
- `direction`: String
- `threshold_usd`: Numeric
- `is_active`: Boolean _(default)_
- `triggered_at`: DateTime _(nullable)_
- `created_at`: DateTime

### AuditLog

pk: `id` (UUID) · fk: user_id

- `id`: UUID _(pk, default)_
- `user_id`: UUID _(fk, nullable)_
- `action`: String _(index)_
- `resource_type`: String _(index)_
- `resource_id`: String _(nullable)_
- `changes`: unknown _(nullable)_
- `ip_address`: String _(nullable)_
- `request_id`: String _(nullable)_
- `timestamp`: DateTime _(default, index)_

### Benchmark

pk: `id` (UUID) · fk: delivery_point_id

- `id`: UUID _(pk, default)_
- `market_product`: String
- `delivery_point_id`: UUID _(fk)_
- `availability_window`: String
- `price_per_mt_usd`: Numeric
- `source`: String _(default)_
- `created_at`: DateTime _(default)_
- `updated_at`: DateTime _(default)_

### Product

pk: `id` (UUID)

- `id`: UUID _(pk, default)_
- `name`: String _(unique)_
- `fuel_type`: String
- `fuel_grade`: String
- `unit`: String _(default)_
- `min_lot_size`: Numeric _(default)_
- `spec_description`: String _(nullable)_
- `is_active`: Boolean _(default)_
- `created_at`: DateTime

### DeliveryPoint

pk: `id` (UUID)

- `id`: UUID _(pk, default)_
- `name`: String _(unique)_
- `region`: String
- `timezone`: String _(nullable)_
- `is_active`: Boolean _(default)_
- `created_at`: DateTime

### TraceabilityEvent

pk: `id` (UUID)

- `id`: UUID _(pk, default)_
- `direct_order_id`: UUID
- `stage`: String
- `location_name`: String
- `timestamp`: DateTime
- `verification_type`: String
- `verification_doc_url`: String
- `verification_hash`: String
- `is_verified`: Boolean _(default)_

### ComplianceLedger

pk: `id` (UUID) · fk: organization_id

- `id`: UUID _(pk, default)_
- `organization_id`: unknown _(fk)_
- `transaction_type`: String
- `amount`: Numeric
- `currency`: String _(default)_
- `units`: Numeric
- `description`: String
- `reference_id`: String
- `created_at`: DateTime _(default)_

### MarketSignalIngestionRun

pk: `id` (UUID)

- `id`: UUID _(pk, default)_
- `signal_family`: String
- `source`: String
- `source_kind`: String
- `started_at`: DateTime _(default)_
- `verified_at`: DateTime _(nullable)_
- `created_at`: DateTime _(default)_

### MarketIndication

pk: `id` (UUID) · fk: delivery_point_id, trusted_ingestion_run_id

- `id`: UUID _(pk, default)_
- `market_product`: String
- `delivery_point_id`: UUID _(fk)_
- `availability_window`: String
- `side`: String
- `price_per_mt_usd`: Numeric
- `quantity_mt`: Numeric _(nullable)_
- `source`: String
- `source_record_id`: String _(nullable)_
- `source_event_id`: String _(nullable)_
- `trusted_ingestion_run_id`: UUID _(fk, nullable)_
- `is_demo`: Boolean _(default)_
- `is_verified_real`: Boolean _(default)_
- `verified_real_at`: DateTime _(nullable)_
- `observed_at`: DateTime
- `created_at`: DateTime _(default)_

### FairPriceBand

pk: `id` (UUID) · fk: delivery_point_id, trusted_ingestion_run_id

- `id`: UUID _(pk, default)_
- `market_product`: String
- `delivery_point_id`: UUID _(fk)_
- `availability_window`: String
- `low_price_per_mt_usd`: Numeric
- `mid_price_per_mt_usd`: Numeric
- `high_price_per_mt_usd`: Numeric
- `model_name`: String
- `model_version`: String _(nullable)_
- `source`: String
- `source_event_id`: String _(nullable)_
- `trusted_ingestion_run_id`: UUID _(fk, nullable)_
- `is_demo`: Boolean _(default)_
- `is_verified_real`: Boolean _(default)_
- `verified_real_at`: DateTime _(nullable)_
- `observed_at`: DateTime
- `created_at`: DateTime _(default)_

### PhysicalStem

pk: `id` (UUID) · fk: delivery_point_id, trusted_ingestion_run_id

- `id`: UUID _(pk, default)_
- `market_product`: String
- `delivery_point_id`: UUID _(fk)_
- `availability_window`: String
- `quantity_mt`: Numeric
- `stem_start`: DateTime _(nullable)_
- `stem_end`: DateTime _(nullable)_
- `status`: String
- `source`: String
- `stem_uid`: String
- `source_record_id`: String _(nullable)_
- `source_event_id`: String _(nullable)_
- `trusted_ingestion_run_id`: UUID _(fk, nullable)_
- `is_demo`: Boolean _(default)_
- `is_verified_real`: Boolean _(default)_
- `verified_real_at`: DateTime _(nullable)_
- `observed_at`: DateTime
- `created_at`: DateTime _(default)_

### LiveSliceBenchmark

pk: `id` (UUID) · fk: delivery_point_id

- `id`: UUID _(pk, default)_
- `side`: Enum
- `market_product`: String
- `delivery_point_id`: UUID _(fk)_
- `availability_window`: String
- `benchmark_price_per_mt_usd`: Numeric
- `total_remaining_quantity_mt`: Numeric
- `order_count`: Integer _(default)_
- `source`: String _(default)_
- `created_at`: DateTime _(default)_
- `updated_at`: DateTime _(default)_

### MarketEventOutbox

pk: `id` (UUID)

- `id`: UUID _(pk, default)_
- `event_type`: String
- `aggregate_type`: String
- `aggregate_id`: String
- `participant_org_ids`: JSON
- `payload`: JSON
- `created_at`: DateTime _(default)_
- `dispatched_at`: DateTime _(nullable)_
- `stream_seq`: BigInteger _(nullable)_
- `delivery_attempts`: Integer _(default)_
- `last_error`: Text _(nullable)_

### StaffCapabilityAssignment

pk: `id` (UUID) · fk: user_id, granted_by_user_id, revoked_by_user_id

- `id`: UUID _(pk, default)_
- `user_id`: UUID _(fk)_
- `capability`: Enum
- `reason`: String
- `granted_by_user_id`: UUID _(fk)_
- `granted_at`: DateTime _(default)_
- `expires_at`: DateTime
- `revoked_at`: DateTime
- `revoked_by_user_id`: UUID _(fk)_
- `revocation_reason`: String

### MarketSupportAuthorization

pk: `id` (UUID) · fk: organization_id, accountable_user_id, product_id, delivery_point_id, created_by_actor_user_id, revoked_by_actor_user_id

- `id`: UUID _(pk, default)_
- `organization_id`: UUID _(fk)_
- `accountable_user_id`: UUID _(fk)_
- `status`: Enum _(default)_
- `product_id`: UUID _(fk)_
- `delivery_point_id`: UUID _(fk)_
- `availability_window`: String
- `quantity_mt`: Numeric
- `price_per_mt_usd`: Numeric
- `authorization_expires_at`: DateTime
- `order_expires_at`: DateTime
- `is_anonymous`: Boolean _(default)_
- `certifications`: JSON _(default)_
- `certification_declared`: Boolean
- `certification_scheme`: String
- `specification_standard`: String
- `msds_available`: Boolean
- `carbon_intensity_gco2_mj`: Numeric
- `carbon_intensity_method`: String
- `feedstock`: String
- `origin`: String
- `off_spec`: Boolean
- `off_spec_notes`: Text
- `terms_digest`: String
- `evidence_reference`: String
- `evidence_sha256`: String
- `commercial_consent_version`: String
- `commercial_consent_reference`: String
- `support_case_reference`: String
- `idempotency_key`: String
- `idempotency_request_hash`: String
- `created_by_actor_user_id`: UUID _(fk)_
- `created_at`: DateTime _(default)_
- `consumed_at`: DateTime
- `revoked_at`: DateTime
- `revoked_by_actor_user_id`: UUID _(fk)_
- `revocation_reason`: String

### InventoryItem

pk: `id` (UUID) · fk: supplier_id, owner_user_id, port_id

- `id`: UUID _(pk, default)_
- `supplier_id`: unknown _(fk)_
- `owner_user_id`: unknown _(fk, nullable)_
- `port_id`: unknown _(fk)_
- `fuel_type`: Enum
- `product_name`: String
- `current_stock_mt`: Numeric
- `incoming_stock_mt`: Numeric _(default)_
- `reserved_stock_mt`: Numeric _(default)_
- `price_per_mt_usd`: Numeric
- `energy_density_mj_kg`: Numeric
- `is_certified`: Boolean _(default)_
- `certification_declared`: Boolean _(default)_
- `certification_scheme`: String
- `specification_standard`: String
- `msds_available`: Boolean _(default)_
- `carbon_intensity_gco2_mj`: Numeric
- `carbon_intensity_method`: String
- `feedstock`: String
- `origin`: String
- `off_spec`: Boolean _(default)_
- `off_spec_notes`: Text
- `updated_at`: DateTime _(default)_
- _relations_: port: Port, supplier: Organization

### MatchSuggestion

pk: `id` (UUID) · fk: bid_order_id, ask_order_id, recipient_org_id

- `id`: UUID _(pk, default)_
- `bid_order_id`: unknown _(fk)_
- `ask_order_id`: unknown _(fk)_
- `score`: Numeric
- `match_reasons`: JSON _(default)_
- `status`: Enum _(default)_
- `recipient_org_id`: unknown _(fk)_
- `created_at`: DateTime _(default)_
- _relations_: bid_order: OrderBookOrder, ask_order: OrderBookOrder, recipient_org: Organization

### Negotiation

pk: `id` (UUID) · fk: bid_order_id, ask_order_id, initiator_org_id, counterparty_org_id, initiator_user_id, counterparty_user_id, accepted_by_user_id, product_id, delivery_point_id, last_actor_org_id, trade_id

- `id`: UUID _(pk, default)_
- `bid_order_id`: UUID _(fk, nullable)_
- `ask_order_id`: UUID _(fk, nullable)_
- `initiator_org_id`: UUID _(fk)_
- `counterparty_org_id`: UUID _(fk)_
- `initiator_user_id`: UUID _(fk, nullable, index)_
- `counterparty_user_id`: UUID _(fk, nullable, index)_
- `accepted_by_user_id`: UUID _(fk, nullable)_
- `initiator_side`: String
- `product_id`: UUID _(fk)_
- `delivery_point_id`: UUID _(fk, nullable)_
- `availability_window`: String _(default)_
- `quantity_mt`: Numeric
- `current_price`: Numeric
- `status`: Enum _(default)_
- `last_actor_org_id`: UUID _(fk)_
- `trade_id`: UUID _(fk, nullable)_
- `expires_at`: DateTime
- `created_at`: DateTime _(default)_
- `updated_at`: DateTime _(default)_
- _relations_: rounds: 

### NegotiationRound

pk: `id` (UUID) · fk: negotiation_id, proposer_org_id, proposer_user_id

- `id`: UUID _(pk, default)_
- `negotiation_id`: UUID _(fk)_
- `round_number`: Integer
- `proposer_org_id`: UUID _(fk)_
- `proposer_user_id`: UUID _(fk, nullable)_
- `proposed_price`: Numeric
- `notes`: Text _(nullable)_
- `created_at`: DateTime _(default)_
- _relations_: negotiation: 

### NewsItem

pk: `id` (UUID)

- `id`: UUID _(pk, default)_
- `title`: String
- `summary`: Text _(nullable)_
- `source`: String
- `source_url`: String
- `url`: String _(unique)_
- `category`: String _(default)_
- `relevance`: Integer _(default)_
- `published_at`: DateTime
- `fetched_at`: DateTime _(default)_

### Notification

pk: `id` (UUID) · fk: recipient_id

- `id`: UUID _(pk, default)_
- `recipient_id`: unknown _(fk)_
- `type`: Enum
- `title`: String
- `message`: Text
- `data`: JSON _(default)_
- `is_read`: Boolean _(default)_
- `created_at`: DateTime _(default)_
- _relations_: recipient: User

### OrderBookOrder

pk: `id` (UUID) · fk: organization_id, owner_user_id, created_by_actor_user_id, support_authorization_id, inventory_item_id, product_id, delivery_point_id, vessel_id

- `id`: UUID _(pk, default)_
- `organization_id`: unknown _(fk)_
- `owner_user_id`: unknown _(fk, nullable, index)_
- `created_by_actor_user_id`: unknown _(fk, nullable)_
- `creation_method`: Enum _(default)_
- `support_authorization_id`: UUID _(fk, nullable)_
- `version`: Integer _(default)_
- `inventory_item_id`: unknown _(fk, nullable, index)_
- `provenance`: Enum _(default)_
- `side`: Enum
- `product_id`: UUID _(fk)_
- `delivery_point_id`: UUID _(fk, nullable)_
- `port_id`: String _(nullable)_
- `vessel_id`: unknown _(fk, nullable)_
- `quantity_mt`: Numeric
- `remaining_quantity_mt`: Numeric
- `price_per_mt_usd`: Numeric
- `availability_window`: String _(default)_
- `delivery_window_start`: Date _(nullable)_
- `delivery_window_end`: Date _(nullable)_
- `certifications`: JSON _(default)_
- `certification_declared`: Boolean _(default)_
- `certification_scheme`: String _(nullable)_
- `specification_standard`: String _(nullable)_
- `msds_available`: Boolean _(default)_
- `is_verdaxis_verified`: Boolean _(default)_
- `carbon_intensity_gco2_mj`: Numeric _(nullable)_
- `carbon_intensity_method`: String _(nullable)_
- `energy_density_mj_kg`: Numeric _(nullable)_
- `feedstock`: String _(nullable)_
- `origin`: String _(nullable)_
- `off_spec`: Boolean _(default)_
- `off_spec_notes`: Text _(nullable)_
- `status`: Enum _(default)_
- `expires_at`: DateTime _(nullable)_
- `idempotency_key`: String _(nullable)_
- `idempotency_operation`: String _(nullable)_
- `idempotency_request_hash`: String _(nullable)_
- `created_at`: DateTime _(default)_
- `updated_at`: DateTime _(default)_
- _relations_: organization: Organization, product: Product, delivery_point: DeliveryPoint, vessel: Vessel, inventory_item: InventoryItem, bid_trades: Trade, ask_trades: Trade

### Trade

pk: `id` (UUID) · fk: bid_order_id, ask_order_id, buyer_id, seller_id, buyer_user_id, seller_user_id, initiator_org_id, product_id, delivery_point_id

- `id`: UUID _(pk, default)_
- `bid_order_id`: unknown _(fk, nullable)_
- `ask_order_id`: unknown _(fk, nullable)_
- `buyer_id`: unknown _(fk)_
- `seller_id`: unknown _(fk)_
- `buyer_user_id`: unknown _(fk, nullable, index)_
- `seller_user_id`: unknown _(fk, nullable, index)_
- `initiator_org_id`: unknown _(fk)_
- `buyer_provenance`: Enum _(default)_
- `seller_provenance`: Enum _(default)_
- `initiated_by`: Enum
- `is_anonymous`: Boolean _(default)_
- `product_id`: UUID _(fk, nullable)_
- `product_name`: String _(nullable)_
- `fuel_type`: String _(nullable)_
- `fuel_grade`: String _(nullable)_
- `market_product`: String _(nullable)_
- `delivery_point_id`: UUID _(fk, nullable)_
- `delivery_point_name`: String _(nullable)_
- `delivery_point_region`: String _(nullable)_
- `availability_window`: String _(nullable)_
- `market_snapshot_version`: SmallInteger _(nullable, default)_
- `quantity_mt`: Numeric
- `price_per_mt_usd`: Numeric
- `status`: Enum _(default)_
- `final_quantity_mt`: Numeric _(nullable)_
- `final_price_per_mt`: Numeric _(nullable)_
- `final_total_usd`: Numeric _(nullable)_
- `commission_rate_pct`: Numeric _(default)_
- `commission_amount_usd`: Numeric _(nullable)_
- `confirmed_at`: DateTime _(nullable)_
- `delivered_at`: DateTime _(nullable)_
- `paid_at`: DateTime _(nullable)_
- `created_at`: DateTime _(default)_
- `idempotency_key`: String _(nullable)_
- `idempotency_operation`: String _(nullable)_
- `idempotency_request_hash`: String _(nullable)_
- _relations_: bid_order: OrderBookOrder, ask_order: OrderBookOrder, buyer: Organization, seller: Organization, commission: Commission

### Commission

pk: `id` (UUID) · fk: trade_id

- `id`: UUID _(pk, default)_
- `match_id`: unknown _(unique)_
- `trade_id`: unknown _(fk, nullable)_
- `amount_usd`: Numeric
- `status`: Enum _(default)_
- `invoice_number`: String
- `invoice_date`: Date
- `payment_date`: Date
- `notes`: String
- `created_at`: DateTime _(default)_
- `updated_at`: DateTime _(default)_
- _relations_: trade: Trade

### Port

pk: `id` (String)

- `id`: String _(pk)_
- `name`: String
- `country`: String
- `location`: Geography
- `timezone`: String
- `is_active`: Boolean _(default)_
- _relations_: intelligence: PortIntelligence, inventory_items: InventoryItem

### PortIntelligence

pk: `id` (UUID) · fk: port_id

- `id`: UUID _(pk, default)_
- `port_id`: str _(fk)_
- `congestion_level`: Enum
- `methanol_price_avg`: Numeric
- `biofuel_price_avg`: Numeric
- `captured_at`: DateTime _(default)_
- _relations_: port: 

### Vessel

pk: `id` (UUID) · fk: organization_id

- `id`: UUID _(pk, default)_
- `organization_id`: unknown _(fk)_
- `name`: String
- `imo_number`: String _(unique)_
- `vessel_type`: String
- `flag_state`: String
- `dwt`: Numeric
- `cii_rating`: String
- `eu_ets_status`: String
- `fueleu_status`: String
- `current_location`: unknown
- `previous_location`: unknown
- `updated_at`: DateTime _(default)_
- _relations_: organization: Organization

### ProducerProject

pk: `id` (UUID) · fk: organization_id

- `id`: UUID _(pk, default)_
- `name`: String
- `fuel_type`: String
- `capacity_kt_per_year`: Numeric
- `country`: String
- `region`: String
- `location`: unknown _(nullable)_
- `cod_date`: Date _(nullable)_
- `cod_year`: Integer _(nullable)_
- `status`: Enum _(default)_
- `data_source`: String
- `gena_project_id`: String _(unique, nullable)_
- `organization_id`: unknown _(fk, nullable)_
- `feedstock`: String
- `technology`: String
- `carbon_intensity_gco2_mj`: Numeric
- `notes`: Text
- `created_at`: DateTime _(default)_
- `updated_at`: DateTime _(default)_
- _relations_: organization: Organization

### UserLoginDay

pk: `id` (UUID) · fk: user_id

- `id`: UUID _(pk, default)_
- `activity_date`: Date
- `user_id`: unknown _(fk)_
- `organization_id`: UUID _(nullable)_
- `role`: Enum _(nullable)_
- `login_count`: Integer _(default)_
- `first_login_at`: DateTime
- `last_login_at`: DateTime

### UserStatusTransition

pk: `id` (UUID) · fk: user_id

- `id`: UUID _(pk, default)_
- `user_id`: unknown _(fk)_
- `organization_id`: UUID _(nullable)_
- `role`: Enum _(nullable)_
- `from_status`: Enum _(nullable)_
- `to_status`: Enum
- `effective_at`: DateTime _(default)_
- `provenance`: String _(default)_

### Referral

pk: `id` (UUID) · fk: referrer_id, referred_user_id

- `id`: UUID _(pk, default)_
- `referrer_id`: UUID _(fk)_
- `referred_user_id`: UUID _(fk, unique)_
- `referral_code_used`: String
- `status`: Enum _(default)_
- `created_at`: DateTime _(default)_
- `verified_at`: DateTime _(nullable)_
- `activated_at`: DateTime _(nullable)_
- _relations_: referrer: , referred_user: 

### RefreshSession

pk: `id` (UUID) · fk: user_id

- `id`: UUID _(pk, default)_
- `user_id`: unknown _(fk, index)_
- `family_id`: UUID _(index)_
- `jti_hash`: String _(unique)_
- `device_id_hash`: String _(nullable, index)_
- `replaced_by_jti_hash`: String _(nullable)_
- `rotation_grace_until`: DateTime _(nullable)_
- `revoked`: Boolean _(default)_
- `expires_at`: DateTime _(index)_
- `created_at`: DateTime _(default)_
- `last_used_at`: DateTime _(nullable)_

### PendingRegistration

pk: `id` (UUID)

- `id`: UUID _(pk, default)_
- `token_hash`: String _(unique)_
- `email`: String
- `password_hash`: String
- `first_name`: String _(nullable)_
- `last_name`: String _(nullable)_
- `role`: Enum _(nullable)_
- `referral_code`: String _(nullable)_
- `expires_at`: DateTime
- `used_at`: DateTime _(nullable)_
- `created_at`: DateTime _(default)_

### OrganizationJoinRequest

pk: `id` (UUID) · fk: user_id, organization_id, reviewed_by

- `id`: UUID _(pk, default)_
- `user_id`: unknown _(fk, index)_
- `organization_id`: unknown _(fk, index)_
- `status`: Enum _(default)_
- `reviewed_by`: unknown _(fk, nullable)_
- `reviewed_at`: DateTime _(nullable)_
- `review_note`: Text _(nullable)_
- `created_at`: DateTime _(default)_

### RFQ

pk: `id` (UUID) · fk: buyer_org_id, buyer_user_id, product_id, delivery_point_id, accepted_quote_id, trade_id

- `id`: UUID _(pk, default)_
- `buyer_org_id`: UUID _(fk)_
- `buyer_user_id`: unknown _(fk, nullable, index)_
- `product_id`: UUID _(fk)_
- `delivery_point_id`: UUID _(fk, nullable)_
- `quantity_mt`: Numeric
- `target_price_per_mt`: Numeric _(nullable)_
- `availability_window`: String _(default)_
- `notes`: Text _(nullable)_
- `is_anonymous`: Boolean _(default)_
- `status`: Enum _(default)_
- `accepted_quote_id`: UUID _(fk, nullable)_
- `trade_id`: UUID _(fk, nullable)_
- `expires_at`: DateTime
- `created_at`: DateTime _(default)_
- _relations_: quotes: 

### RFQQuote

pk: `id` (UUID) · fk: rfq_id, seller_org_id, seller_user_id

- `id`: UUID _(pk, default)_
- `rfq_id`: UUID _(fk)_
- `seller_org_id`: UUID _(fk)_
- `seller_user_id`: unknown _(fk, nullable, index)_
- `price_per_mt_usd`: Numeric
- `notes`: Text _(nullable)_
- `status`: Enum _(default)_
- `created_at`: DateTime _(default)_
- _relations_: rfq: 

### SeedRun

pk: `id` (UUID)

- `id`: UUID _(pk, default)_
- `seed_name`: String
- `environment`: String
- `run_metadata`: JSON _(nullable)_
- `created_at`: DateTime _(default)_
- `completed_at`: DateTime _(default)_

### MarketRowQuarantine

pk: `id` (UUID)

- `id`: UUID _(pk, default)_
- `source_table`: String
- `source_id`: UUID
- `original_row`: JSON
- `dependencies`: JSON
- `environment`: String
- `database_name`: String
- `reason`: Text
- `operator`: String
- `reference`: String
- `quarantined_at`: DateTime _(default)_

### OrganizationMarketApproval

pk: `organization_id` (UUID) · fk: organization_id

- `organization_id`: UUID _(fk, pk)_
- `previous_verification_status`: String
- `reviewed_snapshot`: JSON
- `environment`: String
- `database_name`: String
- `reason`: Text
- `operator`: String
- `reference`: String
- `approved_at`: DateTime _(default)_

### Subscription

pk: `id` (UUID) · fk: org_id

- `id`: UUID _(pk, default)_
- `org_id`: UUID _(fk, unique)_
- `tier`: String _(default)_
- `started_at`: DateTime _(nullable, default)_
- `expires_at`: DateTime _(nullable, default)_
- `is_active`: Boolean _(default)_
- _relations_: organization: Organization

### Organization

pk: `id` (UUID)

- `id`: UUID _(pk, default)_
- `name`: String
- `domain`: String _(unique, nullable)_
- `type`: Enum
- `supplier_tier`: Enum _(nullable, default)_
- `tax_id`: String
- `country_code`: String
- `verification_status`: String
- `provenance`: Enum _(default)_
- `created_at`: DateTime _(default)_
- _relations_: users: , vessels: , orderbook_orders: 

### User

pk: `id` (UUID) · fk: organization_id, kyc_organization_id, kyc_reviewed_by, referred_by_id

- `id`: UUID _(pk, default)_
- `email`: String _(unique)_
- `password_hash`: String
- `first_name`: String
- `last_name`: String
- `role`: Enum _(nullable)_
- `status`: Enum _(default)_
- `organization_id`: unknown _(fk)_
- `last_login`: DateTime
- `password_changed_at`: DateTime _(nullable)_
- `must_change_password`: Boolean _(default)_
- `created_at`: DateTime _(default)_
- `email_verified`: Boolean _(default)_
- `email_verification_token_hash`: String _(nullable, index)_
- `email_verification_token_expires_at`: DateTime _(nullable)_
- `kyc_status`: String _(default)_
- `kyc_organization_id`: UUID _(fk, nullable, index)_
- `kyc_rejection_reason`: Text _(nullable)_
- `kyc_external_evidence_reference`: String _(nullable)_
- `kyc_review_note`: Text _(nullable)_
- `kyc_reviewed_by`: UUID _(fk, nullable)_
- `kyc_reviewed_at`: DateTime _(nullable)_
- `password_reset_token_hash`: String _(nullable)_
- `password_reset_expires`: DateTime _(nullable)_
- `referral_code`: String _(unique, nullable)_
- `referred_by_id`: UUID _(fk, nullable)_
- `onboarding_use_case`: String _(nullable)_
- `onboarding_referral_source`: String _(nullable)_
- _relations_: organization: , referrals_made: , referral_received: 

### UserPreference

pk: `id` (UUID) · fk: user_id

- `id`: UUID _(pk, default)_
- `user_id`: UUID _(fk, index)_
- `namespace`: String
- `value`: JSON
- `updated_at`: DateTime

### Watchlist

pk: `id` (UUID) · fk: user_id

- `id`: UUID _(pk, default)_
- `user_id`: UUID _(fk)_
- `name`: String
- `kind`: Enum _(default)_
- `created_at`: DateTime _(default)_
- _relations_: entries: , targets: , events: 

### WatchlistEntry

pk: `id` (UUID) · fk: watchlist_id, product_id, delivery_point_id

- `id`: UUID _(pk, default)_
- `watchlist_id`: UUID _(fk)_
- `product_id`: UUID _(fk)_
- `delivery_point_id`: UUID _(fk, nullable)_
- `created_at`: DateTime _(default)_
- _relations_: watchlist: 

### WatchlistTarget

pk: `id` (UUID) · fk: watchlist_id, delivery_point_id, order_id

- `id`: UUID _(pk, default)_
- `watchlist_id`: UUID _(fk)_
- `target_type`: Enum
- `market_product_code`: String _(nullable)_
- `delivery_point_id`: UUID _(fk, nullable)_
- `availability_window_code`: String _(nullable)_
- `order_id`: UUID _(fk, nullable)_
- `snapshot_price_per_mt_usd`: unknown _(nullable)_
- `snapshot_quantity_mt`: unknown _(nullable)_
- `snapshot_remaining_quantity_mt`: unknown _(nullable)_
- `snapshot_status`: String _(nullable)_
- `snapshot_side`: String _(nullable)_
- `snapshot_market_product`: String _(nullable)_
- `snapshot_delivery_point_name`: String _(nullable)_
- `snapshot_availability_window`: String _(nullable)_
- `snapshot_counterparty_label`: String _(nullable)_
- `created_at`: DateTime _(default)_
- _relations_: watchlist: , delivery_point: DeliveryPoint, order: OrderBookOrder, events: 

### WatchlistEvent

pk: `id` (UUID) · fk: watchlist_id, watchlist_target_id

- `id`: UUID _(pk, default)_
- `watchlist_id`: UUID _(fk)_
- `watchlist_target_id`: UUID _(fk)_
- `event_type`: Enum
- `event_payload`: JSON _(default)_
- `is_read`: Boolean _(default)_
- `created_at`: DateTime _(default)_
- _relations_: watchlist: , target: 

## Schema Source Files

Search for ORM schema declarations:
- Drizzle: `pgTable` / `mysqlTable` / `sqliteTable`
- Prisma: `prisma/schema.prisma`
- TypeORM: `@Entity()` decorator
- SQLAlchemy: class inheriting `Base`

---
_Back to [overview.md](./overview.md)_