# Database

> **Navigation aid.** Schema shapes and field types extracted via AST. Read the actual schema source files before writing migrations or query logic.

**sqlalchemy, unknown** — 330 models

## sqlalchemy

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

### MarketCommandResult

pk: `id` (UUID) · fk: actor_user_id, effective_organization_id, support_context_id

- `id`: UUID _(pk, default)_
- `actor_user_id`: unknown _(fk)_
- `effective_organization_id`: unknown _(fk)_
- `support_context_id`: unknown _(fk, nullable)_
- `operation`: String
- `idempotency_key`: String
- `request_hash`: String
- `resource_type`: String
- `resource_id`: UUID
- `response_status`: SmallInteger
- `response_body`: unknown _(nullable)_
- `created_at`: DateTime _(default)_

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

### FeedbackEntry

pk: `id` (UUID) · fk: user_id

- `id`: UUID _(pk, default)_
- `user_id`: UUID _(fk, index)_
- `message`: String
- `page`: String _(nullable)_
- `created_at`: DateTime

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

### KYCDocument

pk: `id` (UUID) · fk: user_id, organization_id

- `id`: UUID _(pk)_
- `submission_id`: UUID
- `user_id`: UUID _(fk)_
- `organization_id`: UUID _(fk)_
- `kind`: Text
- `content_type`: Text
- `size_bytes`: int
- `key_id`: Text
- `wrapped_key`: LargeBinary
- `key_nonce`: LargeBinary
- `data_nonce`: LargeBinary
- `ciphertext`: LargeBinary
- `created_at`: DateTime

### KYCExecutionGrace

pk: `id` (UUID) · fk: user_id, organization_id

- `id`: UUID _(pk, default)_
- `user_id`: UUID _(fk)_
- `organization_id`: UUID _(fk)_
- `kyc_status_at_enrollment`: String
- `enrolled_at`: DateTime
- `expires_at`: DateTime
- `created_by`: String

### KYCGraceNotice

pk: `grace_id` (UUID) · fk: grace_id

- `grace_id`: UUID _(fk, pk)_
- `claimed_at`: DateTime
- `sent_at`: DateTime _(nullable)_

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

### MarketSupportContext

pk: `id` (UUID) · fk: actor_user_id, organization_id, accountable_user_id

- `id`: UUID _(pk, default)_
- `actor_user_id`: UUID _(fk)_
- `organization_id`: UUID _(fk)_
- `accountable_user_id`: UUID _(fk)_
- `support_reference`: String
- `scope`: Enum _(default)_
- `started_at`: DateTime _(default)_
- `expires_at`: DateTime
- `ended_at`: DateTime
- `status`: Enum _(default)_
- `version`: Integer _(default)_

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

pk: `id` (UUID) · fk: organization_id, accountable_user_id, market_support_context_id, product_id, delivery_point_id, created_by_actor_user_id, revoked_by_actor_user_id

- `id`: UUID _(pk, default)_
- `organization_id`: UUID _(fk)_
- `accountable_user_id`: UUID _(fk)_
- `market_support_context_id`: UUID _(fk, nullable)_
- `status`: Enum _(default)_
- `product_id`: UUID _(fk)_
- `delivery_point_id`: UUID _(fk)_
- `availability_window`: String
- `quantity_mt`: Numeric
- `price_per_mt_usd`: Numeric
- `order_side`: String _(default)_
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
- `fame_terms`: JSON _(nullable)_
- `lng_terms`: JSON _(nullable)_
- `terms_digest`: String
- `evidence_reference`: String
- `evidence_sha256`: String
- `commercial_consent_version`: String
- `commercial_consent_reference`: String
- `support_case_reference`: String
- `instruction_at`: DateTime
- `acknowledge_exact_terms`: Boolean
- `acknowledge_executable_standing_order`: Boolean
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
- `fame_terms`: JSON _(nullable)_
- `lng_terms`: JSON _(nullable)_
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
- `lng_terms_snapshot`: JSON _(nullable)_
- `fame_terms_snapshot`: JSON _(nullable)_
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
- `acceptance_ordinal`: BigInteger
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
- `fame_terms`: JSON _(nullable)_
- `lng_terms`: JSON _(nullable)_
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
- `fame_terms_snapshot`: JSON _(nullable)_
- `lng_terms_snapshot`: JSON _(nullable)_
- `quantity_mt`: Numeric
- `price_per_mt_usd`: Numeric
- `status`: Enum _(default)_
- `final_quantity_mt`: Numeric _(nullable)_
- `final_price_per_mt`: Numeric _(nullable)_
- `final_total_usd`: Numeric _(nullable)_
- `commission_rate_pct`: Numeric _(default)_
- `commission_fee_per_mt_usd`: Numeric _(nullable)_
- `commission_plan`: String _(nullable)_
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

### PublicReadCacheGeneration

pk: `domain` (Text)

- `domain`: Text _(pk)_
- `generation`: BigInteger

### PublicReadCacheEvent

pk: `domain` (Text)

- `domain`: Text _(pk)_
- `generation`: BigInteger _(pk)_

### PublicReadCacheIncarnation

pk: `singleton` (Boolean)

- `singleton`: Boolean _(pk)_
- `database_epoch`: Text
- `primary_identity`: Text
- `authority_revision`: BigInteger
- `active`: Boolean

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

pk: `id` (UUID) · fk: buyer_org_id, buyer_user_id, product_id, delivery_point_id, source_offer_id, target_supplier_org_id, accepted_quote_id, trade_id

- `id`: UUID _(pk, default)_
- `buyer_org_id`: UUID _(fk)_
- `buyer_user_id`: unknown _(fk, nullable, index)_
- `product_id`: UUID _(fk)_
- `delivery_point_id`: UUID _(fk, nullable)_
- `quantity_mt`: Numeric
- `target_price_per_mt`: Numeric _(nullable)_
- `availability_window`: String _(default)_
- `notes`: Text _(nullable)_
- `contract_terms`: JSON _(nullable)_
- `source_offer_id`: unknown _(fk, nullable)_
- `target_supplier_org_id`: unknown _(fk, nullable)_
- `source_offer_snapshot`: JSON _(nullable)_
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
- `offer_terms`: JSON _(nullable)_
- `expires_at`: DateTime _(nullable)_
- `revision`: Integer _(default)_
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
- `seller_fee_per_mt_usd`: Numeric _(nullable, default)_
- _relations_: organization: Organization

### SupplierOffer

pk: `id` (UUID) · fk: supplier_org_id, supplier_user_id, product_id, delivery_point_id

- `id`: UUID _(pk, default)_
- `supplier_org_id`: UUID _(fk)_
- `supplier_user_id`: UUID _(fk)_
- `product_id`: UUID _(fk)_
- `delivery_point_id`: UUID _(fk)_
- `quantity_mt`: Numeric
- `min_fill_mt`: Numeric
- `price_per_mt_usd`: Numeric
- `availability_window`: String
- `listing_terms`: JSON
- `status`: String _(default)_
- `revision`: Integer _(default)_
- `expires_at`: DateTime
- `created_at`: DateTime _(default)_
- `updated_at`: DateTime _(default)_
- `idempotency_key`: String
- `idempotency_request_hash`: String

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
- `provenance`: Enum
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
- `pending_approval_email_transition_id`: UUID _(nullable)_
- `pending_approval_email_payload`: unknown _(nullable)_
- `pending_approval_email_retry_at`: DateTime _(nullable)_
- `organization_id`: unknown _(fk)_
- `last_login`: DateTime
- `password_changed_at`: DateTime _(nullable)_
- `authentication_revoked_at`: DateTime _(nullable)_
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

## unknown

### AIChatRequest

- `history`: object[]
- `message`: string _(required)_

### AcquisitionKpis

- `average_session_duration_seconds`: DecimalMetricValue _(required)_
- `cta_clicks`: MetricValue _(required)_
- `pageviews`: MetricValue _(required)_
- `visitors`: MetricValue _(required)_
- `visits`: MetricValue _(required)_

### AcquisitionResponse

- `calculator`: CalculatorFunnel _(required)_
- `cta_matrix`: CtaMatrixRow[]
- `entry_pages`: RankedRow[]
- `kpis`: AcquisitionKpis _(required)_
- `languages`: RankedRow[]
- `meta`: AnalyticsMeta _(required)_
- `previous_visitors_trend`: SeriesPoint[]
- `referrers`: RankedRow[]
- `visitors_trend`: SeriesPoint[]
- `visits_trend`: SeriesPoint[]

### ActivationResponse

- `drop_off`: AggregateCell[]
- `journey`: JourneyStage[]
- `meta`: AnalyticsMeta _(required)_
- `ratios`: LabeledRatio[]
- `time_to_first_live_order`: DurationDistribution _(required)_
- `time_to_first_login`: DurationDistribution _(required)_

### ActivityTrend

- `active_members`: SeriesPoint[]
- `confirmed_trades`: SeriesPoint[]
- `orders`: SeriesPoint[]
- `visitors`: SeriesPoint[]

### AdminApproveBody

- `external_evidence_reference`: string _(required)_
- `review_note`: string _(required)_

### AdminDecisionBody

- `reason`: unknown

### AdminFeedbackEntry

- `created_at`: string(date-time) _(required)_
- `id`: string(uuid) _(required, uuid)_
- `message`: string _(required)_
- `org_name`: unknown _(required)_
- `page`: unknown _(required)_
- `user_email`: unknown _(required)_
- `user_name`: unknown _(required)_

### AdminFeedbackResponse

- `items`: AdminFeedbackEntry[] _(required)_
- `total`: integer _(required)_

### AdminInvitationCreate

- `email`: string(email) _(required)_
- `first_name`: string _(required)_
- `last_name`: unknown
- `new_organization`: unknown
- `organization_id`: unknown
- `role`: UserRole _(required)_

### AdminInvitationOrganizationCreate

- `country_code`: string _(required)_
- `name`: string _(required)_
- `tax_id`: unknown
- `type`: OrgType _(required)_

### AdminInvitationResponse

- `acceptance_url`: string _(required)_
- `email`: string _(required)_
- `expires_at`: string(date-time) _(required)_
- `organization_created`: boolean _(required)_
- `organization_name`: string _(required)_
- `reissued`: boolean _(required)_
- `role`: string _(required)_
- `user_id`: string(uuid) _(required, uuid)_

### AdminRejectBody

- `reason`: string _(required)_

### AdminUserEntry

- `created_at`: string(date-time) _(required)_
- `email`: string _(required)_
- `email_verified`: boolean
- `first_name`: unknown _(required)_
- `id`: string(uuid) _(required, uuid)_
- `last_login`: unknown
- `last_name`: unknown _(required)_
- `must_change_password`: boolean
- `org_has_orders`: boolean
- `org_name`: unknown _(required)_
- `org_provenance`: unknown _(required)_
- `org_type`: unknown _(required)_
- `organization_id`: unknown _(required)_
- `role`: string _(required)_
- `status`: string _(required)_

### AdminUsersResponse

- `items`: AdminUserEntry[] _(required)_
- `total`: integer _(required)_

### AggregateCell

- `count`: unknown
- `key`: string _(required)_
- `suppressed`: boolean

### AggregatedOrderbookResponse

- `availability_window`: string
- `delivery_point_id`: unknown
- `delivery_point_name`: unknown
- `demo_status`: MarketDemoStatus _(required)_
- `evidence_class`: string _(required)_
- `fuel_type`: string
- `market_product`: string _(required)_
- `max_price`: string _(required)_
- `min_price`: string _(required)_
- `observed_at`: string(date-time) _(required)_
- `order_count`: integer _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `product_name`: string
- `product_total_order_count`: integer _(required)_
- `region`: string
- `scope`: MarketScope
- `side`: OrderSide _(required)_
- `source_kind`: MarketSourceKind _(required)_
- `total_quantity`: string _(required)_

### AlertCreate

- `delivery_point_id`: unknown
- `direction`: string _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `threshold_usd`: unknown _(required)_

### AlertResponse

- `created_at`: string(date-time) _(required)_
- `delivery_point_id`: unknown _(required)_
- `direction`: string _(required)_
- `id`: string(uuid) _(required, uuid)_
- `is_active`: boolean _(required)_
- `market_product`: unknown
- `org_id`: string(uuid) _(required, uuid)_
- `product_id`: string(uuid) _(required, uuid)_
- `product_name`: unknown
- `threshold_usd`: string _(required)_
- `triggered_at`: unknown _(required)_

### AnalyticsCoverage

- `authoritative`: AnalyticsSourceCoverage _(required)_
- `behavioral`: AnalyticsSourceCoverage _(required)_
- `login_history`: AnalyticsSourceCoverage _(required)_
- `reference`: AnalyticsSourceCoverage _(required)_
- `status_history`: AnalyticsSourceCoverage _(required)_

### AnalyticsDataQuality

- `cohort_complete`: boolean
- `legacy_timestamp_fallback_count`: integer
- `missing_commission_payment_date_count`: integer
- `missing_paid_at_count`: integer
- `suppressed_cell_count`: integer

### AnalyticsMeta

- `coverage`: AnalyticsCoverage _(required)_
- `data_quality`: AnalyticsDataQuality _(required)_
- `end`: string(date-time) _(required)_
- `observed_at`: string(date-time) _(required)_
- `previous_end`: unknown _(required)_
- `previous_start`: unknown _(required)_
- `start`: string(date-time) _(required)_

### AnalyticsSourceCoverage

- `coverage_end`: unknown
- `coverage_start`: unknown
- `diagnostic`: unknown
- `observed_at`: unknown
- `status`: AnalyticsSourceStatus _(required)_

### AssistedListingCancel

- `reason`: string _(required)_

### AssistedListingCreate

- `acknowledge_executable_standing_order`: boolean _(required)_
- `authorization_id`: string(uuid) _(required, uuid)_

### AssistedListingResponse

- `accountable_user_id`: string(uuid) _(required, uuid)_
- `created_by_actor_user_id`: string(uuid) _(required, uuid)_
- `creation_method`: OrderCreationMethod _(required)_
- `etag`: string _(required)_
- `order`: OrderResponse _(required)_
- `support_authorization_id`: string(uuid) _(required, uuid)_
- `version`: integer _(required)_

### AttentionItem

- `email`: string _(required)_
- `last_login`: unknown _(required)_
- `name`: unknown _(required)_
- `organization_name`: unknown _(required)_
- `role`: unknown _(required)_
- `since`: string(date-time) _(required)_
- `stage`: string _(required)_

### AttentionResponse

- `generated_at`: string(date-time) _(required)_
- `items`: AttentionItem[] _(required)_

### AuditActivityRow

- `action`: string _(required)_
- `actor_role`: unknown
- `occurred_at`: string(date-time) _(required)_
- `resource_type`: unknown

### AuditLogResponse

- `action`: string _(required)_
- `changes`: unknown _(required)_
- `id`: string(uuid) _(required, uuid)_
- `ip_address`: unknown _(required)_
- `request_id`: unknown _(required)_
- `resource_id`: unknown _(required)_
- `resource_type`: string _(required)_
- `timestamp`: string(date-time) _(required)_
- `user_id`: unknown _(required)_

### AuthoritativeUsage

- `order_placing_organizations`: integer _(required)_
- `registrations`: integer _(required)_
- `users_logging_in`: integer _(required)_

### AuthorizationCreate

- `accountable_user_id`: string(uuid) _(required, uuid)_
- `authorization_expires_at`: string(date-time) _(required)_
- `commercial_consent_reference`: string _(required)_
- `commercial_consent_version`: string _(required)_
- `evidence_reference`: string _(required)_
- `evidence_sha256`: string _(required)_
- `order`: OrderCreate-Input _(required)_
- `support_case_reference`: unknown

### AuthorizationResponse

- `accountable_user_id`: string(uuid) _(required, uuid)_
- `acknowledge_exact_terms`: unknown _(required)_
- `acknowledge_executable_standing_order`: unknown _(required)_
- `authorization_expires_at`: string(date-time) _(required)_
- `availability_window`: string _(required)_
- `commercial_consent_reference`: string _(required)_
- `commercial_consent_version`: string _(required)_
- `consumed_at`: unknown _(required)_
- `created_at`: string(date-time) _(required)_
- `created_by_actor_user_id`: string(uuid) _(required, uuid)_
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `evidence_reference`: string _(required)_
- `evidence_sha256`: unknown _(required)_
- `fame_terms`: unknown
- `id`: string(uuid) _(required, uuid)_
- `instruction_at`: unknown _(required)_
- `lng_terms`: unknown
- `order`: OrderCreate-Output _(required)_
- `order_expires_at`: unknown _(required)_
- `organization_id`: string(uuid) _(required, uuid)_
- `price_per_mt_usd`: string _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `quantity_mt`: string _(required)_
- `revocation_reason`: unknown _(required)_
- `revoked_at`: unknown _(required)_
- `revoked_by_actor_user_id`: unknown _(required)_
- `status`: MarketSupportAuthorizationStatus _(required)_
- `support_case_reference`: unknown _(required)_
- `terms_digest`: string _(required)_

### AuthorizationRevoke

- `reason`: string _(required)_

### BackendUnavailablePanel

- `by_route_family`: AggregateCell[]
- `total`: MetricValue _(required)_

### BehavioralUsage

- `average_session_duration_seconds`: number _(required)_
- `daily_visitors`: DailyVisitorPoint[] _(required)_
- `event_series`: DailyEventPoint[] _(required)_
- `event_totals`: object _(required)_
- `pageviews`: integer _(required)_
- `top_entries`: MetricEntry[] _(required)_
- `top_referrers`: MetricEntry[] _(required)_
- `total_time_seconds`: integer _(required)_
- `visitors`: integer _(required)_
- `visits`: integer _(required)_

### BenchmarkQuote

- `availability_window`: string _(required)_
- `benchmark_price_per_mt_usd`: string _(required)_
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `delivery_point_name`: string _(required)_
- `generated_at`: string(date-time) _(required)_
- `market_product`: string _(required)_
- `observed_at`: unknown
- `source`: string _(required)_

### BenchmarkQuoteResponse

- `generated_at`: string(date-time) _(required)_
- `items`: BenchmarkQuote[] _(required)_

### Body_login_api_auth_login_post

- `client_id`: unknown
- `client_secret`: undefined(password)
- `grant_type`: unknown
- `password`: string(password) _(required)_
- `scope`: string
- `username`: string _(required)_

### Body_submit_kyc_api_kyc_submit_post

- `company_doc`: string _(required)_
- `declared_company_name`: unknown
- `passport`: string _(required)_
- `registration_number`: unknown

### CIAdjustedPrice

- `base_price_per_mt`: string _(required)_
- `carbon_intensity_gco2_mj`: string _(required)_
- `compliance_cost_per_mt`: string _(required)_
- `effective_price_per_mt`: string _(required)_
- `financial_benefit_status`: string
- `fueleu_ghg_intensity`: string _(required)_
- `ghg_reduction_pct`: string _(required)_

### CIIResponse

- `rating`: string _(required)_
- `score`: integer _(required)_

### CalculatorFunnel

- `completions`: MetricValue _(required)_
- `starts`: MetricValue _(required)_

### CapabilityAssignmentCreate

- `capability`: MarketSupportCapability _(required)_
- `expires_at`: unknown
- `reason`: string _(required)_
- `user_id`: string(uuid) _(required, uuid)_

### CapabilityAssignmentResponse

- `capability`: MarketSupportCapability _(required)_
- `expires_at`: unknown _(required)_
- `granted_at`: string(date-time) _(required)_
- `granted_by_user_id`: string(uuid) _(required, uuid)_
- `id`: string(uuid) _(required, uuid)_
- `reason`: string _(required)_
- `revocation_reason`: unknown _(required)_
- `revoked_at`: unknown _(required)_
- `revoked_by_user_id`: unknown _(required)_
- `user_id`: string(uuid) _(required, uuid)_

### CapabilityAssignmentRevoke

- `reason`: string _(required)_

### CohortCell

- `cell`: AggregateCell _(required)_
- `offset`: integer _(required)_
- `pct`: unknown

### CohortRow

- `cells`: CohortCell[]
- `cohort_start`: string(date) _(required)_
- `size`: AggregateCell _(required)_

### CollectorState

- `diagnostic`: unknown
- `last_observation_at`: unknown
- `status`: AnalyticsSourceStatus _(required)_

### CommercialSummary

- `commission_invoiced_usd`: unknown
- `commission_pending_usd`: unknown
- `realized_gmv_usd`: DecimalMetricValue _(required)_
- `realized_revenue_usd`: DecimalMetricValue _(required)_

### CommissionResponse

- `amount_usd`: string _(required)_
- `created_at`: string(date-time) _(required)_
- `id`: string(uuid) _(required, uuid)_
- `invoice_date`: unknown
- `invoice_number`: unknown
- `match_id`: string(uuid) _(required, uuid)_
- `payment_date`: unknown
- `status`: CommissionStatus _(required)_

### CommissionSummary

- `invoiced_count`: integer _(required)_
- `paid_count`: integer _(required)_
- `pending_count`: integer _(required)_
- `total_invoiced_usd`: string _(required)_
- `total_paid_usd`: string _(required)_
- `total_pending_usd`: string _(required)_

### CommissionUpdate

- `invoice_date`: unknown
- `invoice_number`: unknown
- `notes`: unknown
- `payment_date`: unknown
- `status`: unknown

### ComplianceScoreResponse

- `cii`: CIIResponse _(required)_
- `eu_ets`: EUETSResponse _(required)_
- `fueleu`: FuelEUResponse _(required)_
- `overall_score`: integer _(required)_
- `recommendations`: string[] _(required)_
- `status`: string _(required)_
- `traffic_light`: string _(required)_
- `vessel_id`: string _(required)_
- `vessel_name`: string _(required)_

### CtaMatrixRow

- `clicks`: unknown
- `cta`: string _(required)_
- `placement`: string _(required)_
- `share_pct`: unknown
- `suppressed`: boolean

### DailyEventPoint

- `date`: string _(required)_
- `event`: string _(required)_
- `value`: integer _(required)_

### DailyStat

- `commission_usd`: number _(required)_
- `date`: string(date) _(required)_
- `gmv_usd`: number _(required)_
- `orders_placed`: integer _(required)_
- `trades_executed`: integer _(required)_
- `volume_mt`: number _(required)_

### DailyVisitorPoint

- `date`: string _(required)_
- `value`: integer _(required)_

### DecimalMetricValue

- `previous`: unknown
- `suppressed`: boolean
- `value`: unknown

### DeliveryPointResponse

- `id`: string(uuid) _(required, uuid)_
- `is_active`: boolean _(required)_
- `name`: string _(required)_
- `region`: string _(required)_
- `timezone`: unknown

### DemandSignal

- `availability_window_code`: unknown
- `bid_count`: integer _(required)_
- `created_at`: string(date-time) _(required)_
- `delivery_point_id`: unknown
- `delivery_point_name`: unknown
- `demo_status`: MarketDemoStatus
- `earliest_delivery`: string _(required)_
- `fuel_type`: string _(required)_
- `market_product_code`: unknown
- `max_price_per_mt`: unknown _(required)_
- `region`: string _(required)_
- `scope`: MarketScope
- `source_kind`: MarketSourceKind
- `unknown_count`: integer
- `urgency`: UrgencyLevel _(required)_
- `volume_mt`: unknown _(required)_

### DurationBucket

- `bucket`: string _(required)_
- `cell`: AggregateCell _(required)_

### DurationDistribution

- `buckets`: DurationBucket[]
- `median_hours`: unknown
- `sample_size`: unknown
- `suppressed`: boolean

### EUETSResponse

- `calculation_status`: string _(required)_
- `estimated_cost_eur`: unknown _(required)_
- `ets_price_per_tonne_eur`: string _(required)_
- `phase_in_pct`: string _(required)_
- `score`: integer _(required)_
- `total_co2_tonnes`: unknown _(required)_

### EngagementKpis

- `dau`: MetricValue _(required)_
- `mau`: MetricValue _(required)_
- `stickiness_pct`: unknown
- `wau`: MetricValue _(required)_

### EngagementResponse

- `active_members_trend`: SeriesPoint[]
- `feature_adoption`: FeatureAdoptionRow[]
- `kpis`: EngagementKpis _(required)_
- `meta`: AnalyticsMeta _(required)_
- `navigation_destinations`: NavigationDestinationRow[]
- `tutorial_steps`: TutorialStepRow[]
- `workflow_ratios`: LabeledRatio[]

### ErrorDetail

- `detail`: string _(required)_

### ExecutionEligibilityResponse

- `eligible`: boolean
- `grace_expires_at`: unknown
- `reason`: string

### FameAskTerms-Input

- `astm_grade`: unknown
- `batch_reference`: unknown
- `certificate_holder`: string _(required)_
- `certificate_reference`: string _(required)_
- `certificate_scope`: unknown
- `certificate_valid_until`: string(date) _(required)_
- `cfpp_c`: unknown
- `ci_basis`: unknown
- `ci_boundary`: unknown
- `ci_gco2e_mj`: unknown
- `ci_methodology`: unknown
- `cloud_point_c`: unknown
- `document_references`: string[]
- `en_climate_class`: unknown
- `evidence_due`: string _(required)_
- `evidence_status`: string _(required)_
- `feedstock_origin`: unknown
- `lhv_mj_kg`: unknown
- `neat_fame`: boolean _(required)_
- `nomination_status`: string
- `producing_site`: unknown
- `production_origin`: unknown
- `quality_evidence`: unknown
- `schema_version`: integer
- `shipping_location`: unknown
- `side`: string _(required)_
- `standard`: string _(required)_
- `standard_edition`: string _(required)_
- `sustainability_evidence`: unknown
- `sustainability_scheme`: string _(required)_
- `uco_mass_pct`: integer _(required)_

### FameAskTerms-Output

- `astm_grade`: unknown
- `batch_reference`: unknown
- `certificate_holder`: string _(required)_
- `certificate_reference`: string _(required)_
- `certificate_scope`: unknown
- `certificate_valid_until`: string(date) _(required)_
- `cfpp_c`: unknown
- `ci_basis`: unknown
- `ci_boundary`: unknown
- `ci_gco2e_mj`: unknown
- `ci_methodology`: unknown
- `cloud_point_c`: unknown
- `document_references`: string[]
- `en_climate_class`: unknown
- `evidence_due`: string _(required)_
- `evidence_status`: string _(required)_
- `feedstock_origin`: unknown
- `lhv_mj_kg`: unknown
- `neat_fame`: boolean _(required)_
- `nomination_status`: string
- `producing_site`: unknown
- `production_origin`: unknown
- `quality_evidence`: unknown
- `schema_version`: integer
- `shipping_location`: unknown
- `side`: string _(required)_
- `standard`: string _(required)_
- `standard_edition`: string _(required)_
- `sustainability_evidence`: unknown
- `sustainability_scheme`: string _(required)_
- `uco_mass_pct`: integer _(required)_

### FameBidTerms-Input

- `astm_grade`: unknown
- `ci_basis`: unknown
- `ci_boundary`: unknown
- `ci_methodology`: unknown
- `en_climate_class`: unknown
- `evidence_due`: string
- `max_cfpp_c`: unknown
- `max_ci_gco2e_mj`: unknown
- `max_cloud_point_c`: unknown
- `neat_fame`: boolean _(required)_
- `require_quality_evidence`: boolean
- `require_sustainability_evidence`: boolean
- `schema_version`: integer
- `side`: string _(required)_
- `standard`: string _(required)_
- `standard_edition`: string _(required)_
- `sustainability_scheme`: string _(required)_

### FameBidTerms-Output

- `astm_grade`: unknown
- `ci_basis`: unknown
- `ci_boundary`: unknown
- `ci_methodology`: unknown
- `en_climate_class`: unknown
- `evidence_due`: string
- `max_cfpp_c`: unknown
- `max_ci_gco2e_mj`: unknown
- `max_cloud_point_c`: unknown
- `neat_fame`: boolean _(required)_
- `require_quality_evidence`: boolean
- `require_sustainability_evidence`: boolean
- `schema_version`: integer
- `side`: string _(required)_
- `standard`: string _(required)_
- `standard_edition`: string _(required)_
- `sustainability_scheme`: string _(required)_

### FameContractTerms-Input

- `astm_grade`: unknown
- `claims_terms`: string _(required)_
- `delivery_basis`: string _(required)_
- `delivery_end`: string(date) _(required)_
- `delivery_start`: string(date) _(required)_
- `en_climate_class`: unknown
- `evidence_due`: string _(required)_
- `inspection_terms`: string _(required)_
- `max_cfpp_c`: unknown
- `max_ci_gco2e_mj`: unknown
- `min_fill_mt`: unknown _(required)_
- `named_location`: string _(required)_
- `neat_fame`: boolean _(required)_
- `payment_terms`: string _(required)_
- `quantity_tolerance_pct`: unknown _(required)_
- `schema_version`: integer
- `standard`: string _(required)_
- `standard_edition`: string _(required)_
- `sustainability_scheme`: string _(required)_
- `title_risk_terms`: string _(required)_

### FameContractTerms-Output

- `astm_grade`: unknown
- `claims_terms`: string _(required)_
- `delivery_basis`: string _(required)_
- `delivery_end`: string(date) _(required)_
- `delivery_start`: string(date) _(required)_
- `en_climate_class`: unknown
- `evidence_due`: string _(required)_
- `inspection_terms`: string _(required)_
- `max_cfpp_c`: unknown
- `max_ci_gco2e_mj`: unknown
- `min_fill_mt`: string _(required)_
- `named_location`: string _(required)_
- `neat_fame`: boolean _(required)_
- `payment_terms`: string _(required)_
- `quantity_tolerance_pct`: string _(required)_
- `schema_version`: integer
- `standard`: string _(required)_
- `standard_edition`: string _(required)_
- `sustainability_scheme`: string _(required)_
- `title_risk_terms`: string _(required)_

### FameFuelSummary

- `astm_grade`: unknown
- `certificate_valid_until`: string(date) _(required)_
- `cfpp_c`: unknown
- `ci_basis`: unknown
- `ci_boundary`: unknown
- `ci_gco2e_mj`: unknown
- `ci_methodology`: unknown
- `cloud_point_c`: unknown
- `en_climate_class`: unknown
- `evidence_status`: string _(required)_
- `feedstock_origin`: unknown
- `lhv_mj_kg`: unknown
- `neat_fame`: boolean
- `nomination_status`: string _(required)_
- `production_origin`: unknown
- `quality_evidence`: unknown
- `schema_version`: integer
- `shipping_location`: unknown
- `standard`: string _(required)_
- `standard_edition`: string _(required)_
- `sustainability_evidence`: unknown
- `sustainability_scheme`: string _(required)_
- `uco_mass_pct`: integer _(required)_

### FameListingFuelTerms-Input

- `astm_grade`: unknown
- `batch_reference`: unknown
- `certificate_holder`: string _(required)_
- `certificate_reference`: string _(required)_
- `certificate_scope`: unknown
- `certificate_valid_until`: string(date) _(required)_
- `cfpp_c`: unknown
- `ci_basis`: unknown
- `ci_boundary`: unknown
- `ci_gco2e_mj`: unknown
- `ci_methodology`: unknown
- `cloud_point_c`: unknown
- `document_references`: string[]
- `en_climate_class`: unknown
- `evidence_status`: string _(required)_
- `feedstock_origin`: unknown
- `lhv_mj_kg`: unknown
- `neat_fame`: boolean _(required)_
- `nomination_status`: string
- `producing_site`: unknown
- `production_origin`: unknown
- `quality_evidence`: unknown
- `schema_version`: integer
- `shipping_location`: unknown
- `standard`: string _(required)_
- `standard_edition`: string _(required)_
- `sustainability_evidence`: unknown
- `sustainability_scheme`: string _(required)_
- `uco_mass_pct`: integer _(required)_

### FameListingFuelTerms-Output

- `astm_grade`: unknown
- `batch_reference`: unknown
- `certificate_holder`: string _(required)_
- `certificate_reference`: string _(required)_
- `certificate_scope`: unknown
- `certificate_valid_until`: string(date) _(required)_
- `cfpp_c`: unknown
- `ci_basis`: unknown
- `ci_boundary`: unknown
- `ci_gco2e_mj`: unknown
- `ci_methodology`: unknown
- `cloud_point_c`: unknown
- `document_references`: string[]
- `en_climate_class`: unknown
- `evidence_status`: string _(required)_
- `feedstock_origin`: unknown
- `lhv_mj_kg`: unknown
- `neat_fame`: boolean _(required)_
- `nomination_status`: string
- `producing_site`: unknown
- `production_origin`: unknown
- `quality_evidence`: unknown
- `schema_version`: integer
- `shipping_location`: unknown
- `standard`: string _(required)_
- `standard_edition`: string _(required)_
- `sustainability_evidence`: unknown
- `sustainability_scheme`: string _(required)_
- `uco_mass_pct`: integer _(required)_

### FameOfferTerms-Input

- `astm_grade`: unknown
- `available_quantity_mt`: unknown _(required)_
- `batch_reference`: string _(required)_
- `certificate_holder`: string _(required)_
- `certificate_reference`: string _(required)_
- `certificate_scope`: unknown
- `certificate_valid_until`: string(date) _(required)_
- `cfpp_c`: unknown
- `ci_basis`: unknown
- `ci_boundary`: unknown
- `ci_gco2e_mj`: unknown
- `ci_methodology`: unknown
- `cloud_point_c`: unknown
- `document_references`: string[]
- `en_climate_class`: unknown
- `evidence_status`: string _(required)_
- `feedstock_origin`: string _(required)_
- `lhv_mj_kg`: unknown
- `matches_contract_terms`: boolean _(required)_
- `neat_fame`: boolean
- `producing_site`: string _(required)_
- `production_origin`: string _(required)_
- `quality_evidence`: unknown
- `schema_version`: integer
- `shipping_location`: string _(required)_
- `standard`: string _(required)_
- `standard_edition`: string _(required)_
- `sustainability_evidence`: unknown
- `sustainability_scheme`: string _(required)_
- `uco_mass_pct`: integer _(required)_

### FameOfferTerms-Output

- `astm_grade`: unknown
- `available_quantity_mt`: string _(required)_
- `batch_reference`: string _(required)_
- `certificate_holder`: string _(required)_
- `certificate_reference`: string _(required)_
- `certificate_scope`: unknown
- `certificate_valid_until`: string(date) _(required)_
- `cfpp_c`: unknown
- `ci_basis`: unknown
- `ci_boundary`: unknown
- `ci_gco2e_mj`: unknown
- `ci_methodology`: unknown
- `cloud_point_c`: unknown
- `document_references`: string[]
- `en_climate_class`: unknown
- `evidence_status`: string _(required)_
- `feedstock_origin`: string _(required)_
- `lhv_mj_kg`: unknown
- `matches_contract_terms`: boolean _(required)_
- `neat_fame`: boolean
- `producing_site`: string _(required)_
- `production_origin`: string _(required)_
- `quality_evidence`: unknown
- `schema_version`: integer
- `shipping_location`: string _(required)_
- `standard`: string _(required)_
- `standard_edition`: string _(required)_
- `sustainability_evidence`: unknown
- `sustainability_scheme`: string _(required)_
- `uco_mass_pct`: integer _(required)_

### FamePublicAskTerms

- `astm_grade`: unknown
- `certificate_valid_until`: string(date) _(required)_
- `cfpp_c`: unknown
- `ci_basis`: unknown
- `ci_boundary`: unknown
- `ci_gco2e_mj`: unknown
- `ci_methodology`: unknown
- `cloud_point_c`: unknown
- `en_climate_class`: unknown
- `evidence_due`: string _(required)_
- `evidence_status`: string _(required)_
- `feedstock_origin`: unknown
- `lhv_mj_kg`: unknown
- `neat_fame`: boolean
- `nomination_status`: string _(required)_
- `production_origin`: unknown
- `quality_evidence`: unknown
- `schema_version`: integer
- `shipping_location`: unknown
- `side`: string _(required)_
- `standard`: string _(required)_
- `standard_edition`: string _(required)_
- `sustainability_evidence`: unknown
- `sustainability_scheme`: string _(required)_
- `uco_mass_pct`: integer _(required)_

### FamePublicTradeSnapshot

- `ask`: FamePublicAskTerms _(required)_
- `bid`: FameBidTerms-Output _(required)_
- `schema_version`: integer

### FameQualityEvidence-Input

- `batch_reference`: unknown
- `laboratory`: unknown
- `reference`: unknown
- `results`: FameQualityResult-Input[]
- `sampled_on`: unknown
- `status`: string _(required)_
- `tested_on`: unknown

### FameQualityEvidence-Output

- `batch_reference`: unknown
- `laboratory`: unknown
- `reference`: unknown
- `results`: FameQualityResult-Output[]
- `sampled_on`: unknown
- `status`: string _(required)_
- `tested_on`: unknown

### FameQualityResult-Input

- `method`: unknown
- `property`: string _(required)_
- `value`: unknown _(required)_

### FameQualityResult-Output

- `method`: unknown
- `property`: string _(required)_
- `value`: string _(required)_

### FameQualitySummary

- `results`: FameQualityResult-Output[] _(required)_
- `status`: string _(required)_

### FameSustainabilityEvidence-Input

- `document_type`: string _(required)_
- `due`: string _(required)_
- `issuer`: unknown
- `quantity_mt`: unknown
- `reference`: unknown
- `status`: string _(required)_
- `supply_date`: unknown

### FameSustainabilityEvidence-Output

- `document_type`: string _(required)_
- `due`: string _(required)_
- `issuer`: unknown
- `quantity_mt`: unknown
- `reference`: unknown
- `status`: string _(required)_
- `supply_date`: unknown

### FameSustainabilitySummary

- `document_type`: string _(required)_
- `due`: string _(required)_
- `status`: string _(required)_

### FameTradeSnapshot

- `ask`: FameAskTerms-Output _(required)_
- `bid`: FameBidTerms-Output _(required)_
- `schema_version`: integer

### FeatureAdoptionRow

- `events`: unknown
- `family`: string _(required)_
- `suppressed`: boolean

### FeeScheduleResponse

- `buyer_fee_per_mt_usd`: string
- `currency`: string
- `seller_fee_per_mt_usd`: object _(required)_

### FeedbackCreated

- `created_at`: string(date-time) _(required)_
- `id`: string(uuid) _(required, uuid)_

### FeedbackSubmission

- `message`: string _(required)_
- `page`: unknown

### FleetComplianceSummary

- `amber_count`: integer _(required)_
- `average_score`: number _(required)_
- `green_count`: integer _(required)_
- `red_count`: integer _(required)_
- `total_vessels`: integer _(required)_
- `vessels`: ComplianceScoreResponse[] _(required)_

### FleetDemandEntry

- `avg_consumption_mt`: integer _(required)_
- `color`: string _(required)_
- `delivered_vessels`: integer _(required)_
- `fuel`: string _(required)_
- `ordered_vessels`: integer _(required)_

### FleetDemandResponse

- `entries`: FleetDemandEntry[] _(required)_
- `last_updated`: string _(required)_
- `sources`: string[] _(required)_

### ForgotPasswordRequest

- `email`: string _(required)_

### ForwardCurveBoardCell

- `availability_window`: string _(required)_
- `benchmark_mid`: unknown
- `benchmark_observed_at`: unknown
- `benchmark_source`: unknown
- `benchmark_source_kind`: MarketSourceKind
- `best_ask`: unknown
- `best_ask_source_kind`: MarketSourceKind
- `best_bid`: unknown
- `best_bid_source_kind`: MarketSourceKind
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `delivery_point_name`: string _(required)_
- `demo_best_ask`: unknown
- `demo_best_bid`: unknown
- `demo_order_count`: integer
- `demo_status`: MarketDemoStatus
- `fair_price_band`: unknown
- `fair_price_band_provenance`: ForwardCurveSignalProvenance
- `indication_summary`: ForwardCurveBoardIndicationSummary
- `is_demo_benchmark`: boolean
- `market_product`: string _(required)_
- `order_count`: integer
- `order_observed_at`: unknown
- `order_source_kind`: MarketSourceKind
- `physical_stem_summary`: ForwardCurveBoardPhysicalStemSummary
- `product_id`: string(uuid) _(required, uuid)_
- `product_name`: string _(required)_
- `real_best_ask`: unknown
- `real_best_bid`: unknown
- `real_order_count`: integer
- `region`: string _(required)_
- `scope`: MarketScope
- `spread`: unknown
- `unknown_order_count`: integer
- `volume_mt`: string

### ForwardCurveBoardDepthLevel

- `demo_order_count`: integer
- `demo_status`: MarketDemoStatus
- `order_count`: integer _(required)_
- `price_per_mt_usd`: string _(required)_
- `quantity_mt`: string _(required)_
- `real_order_count`: integer
- `source_kind`: MarketSourceKind
- `unknown_order_count`: integer

### ForwardCurveBoardFairPriceBand

- `high_price_per_mt_usd`: unknown
- `low_price_per_mt_usd`: unknown
- `mid_price_per_mt_usd`: unknown
- `provenance`: ForwardCurveSignalProvenance _(required)_

### ForwardCurveBoardFocus

- `availability_window`: string _(required)_
- `curve`: ForwardCurveBoardCell[] _(required)_
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `delivery_point_name`: string _(required)_
- `depth_asks`: ForwardCurveBoardDepthLevel[] _(required)_
- `depth_bids`: ForwardCurveBoardDepthLevel[] _(required)_
- `fair_price_band`: unknown
- `fair_price_band_provenance`: ForwardCurveSignalProvenance
- `indications`: ForwardCurveBoardIndication[]
- `market_product`: string _(required)_
- `physical_stems`: ForwardCurveBoardPhysicalStem[]
- `product_id`: string(uuid) _(required, uuid)_
- `product_name`: string _(required)_
- `region`: string _(required)_

### ForwardCurveBoardIndication

- `price_per_mt_usd`: string _(required)_
- `provenance`: ForwardCurveSignalProvenance _(required)_
- `quantity_mt`: unknown
- `side`: ForwardCurveIndicationSide _(required)_

### ForwardCurveBoardIndicationSummary

- `indication_count`: integer
- `latest_ask_price_per_mt_usd`: unknown
- `latest_bid_price_per_mt_usd`: unknown
- `latest_mid_price_per_mt_usd`: unknown
- `provenance`: ForwardCurveSignalProvenance
- `total_quantity_mt`: unknown

### ForwardCurveBoardPhysicalStem

- `provenance`: ForwardCurveSignalProvenance _(required)_
- `quantity_mt`: string _(required)_
- `status`: ForwardCurvePhysicalStemStatus _(required)_
- `stem_end`: unknown
- `stem_start`: unknown

### ForwardCurveBoardPhysicalStemSummary

- `available_quantity_mt`: unknown
- `earliest_stem_start`: unknown
- `latest_stem_end`: unknown
- `provenance`: ForwardCurveSignalProvenance
- `stem_count`: integer
- `tentative_quantity_mt`: unknown

### ForwardCurveBoardPort

- `cells`: ForwardCurveBoardCell[] _(required)_
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `delivery_point_name`: string _(required)_
- `region`: string _(required)_

### ForwardCurveBoardProduct

- `market_product`: string _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `product_name`: string _(required)_

### ForwardCurveBoardResponse

- `availability_window`: string _(required)_
- `focus`: ForwardCurveBoardFocus _(required)_
- `generated_at`: string(date-time) _(required)_
- `ports`: ForwardCurveBoardPort[] _(required)_
- `products`: ForwardCurveBoardProduct[] _(required)_

### ForwardCurveLabelPolicy

- `allowed_terms`: string[]
- `disclaimer`: unknown
- `forbidden_terms`: string[]
- `public_label`: string _(required)_
- `tooltip`: unknown

### ForwardCurveLatestSignal

- `availability_window`: string _(required)_
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `delivery_point_name`: string _(required)_
- `demo_status`: MarketDemoStatus
- `market_product`: string _(required)_
- `observed_at`: unknown
- `primary_signal_type`: MarketSignalType
- `primary_source_kind`: MarketSourceKind
- `primary_value`: unknown
- `public_source_label`: string
- `staleness_status`: ForwardCurveStalenessStatus

### ForwardCurveMarketCell

- `availability_window`: string _(required)_
- `benchmark_mid`: unknown
- `benchmark_observed_at`: unknown
- `benchmark_source_kind`: MarketSourceKind
- `best_ask`: unknown
- `best_bid`: unknown
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `delivery_point_name`: string _(required)_
- `demo_best_ask`: unknown
- `demo_best_bid`: unknown
- `demo_order_count`: integer
- `demo_status`: MarketDemoStatus
- `fair_price_band`: unknown
- `fair_price_band_provenance`: ForwardCurveSignalProvenance
- `generated_at`: string(date-time) _(required)_
- `indication_summary`: ForwardCurveBoardIndicationSummary
- `is_executable`: boolean
- `is_reference`: boolean
- `label_policy`: ForwardCurveLabelPolicy
- `market_product`: string _(required)_
- `observed_at`: unknown
- `order_count`: integer
- `physical_stem_summary`: ForwardCurveBoardPhysicalStemSummary
- `primary_signal_type`: MarketSignalType
- `primary_source_kind`: MarketSourceKind
- `primary_value`: unknown
- `product_count`: integer
- `product_name`: string _(required)_
- `public_source_label`: string
- `real_best_ask`: unknown
- `real_best_bid`: unknown
- `real_order_count`: integer
- `region`: string _(required)_
- `representative_product_id`: string(uuid) _(required, uuid)_
- `scope`: MarketScope
- `spread`: unknown
- `staleness_status`: ForwardCurveStalenessStatus
- `unknown_order_count`: integer
- `volume_mt`: string

### ForwardCurvePoint

- `availability_window`: string _(required)_
- `best_ask`: unknown
- `best_bid`: unknown
- `mid_price`: unknown
- `order_count`: integer _(required)_
- `spread`: unknown
- `volume_mt`: string _(required)_

### ForwardCurveResponse

- `curve`: ForwardCurvePoint[] _(required)_
- `delivery_point_id`: unknown
- `generated_at`: string(date-time) _(required)_
- `product_id`: string(uuid) _(required, uuid)_

### ForwardCurveSignalProvenance

- `demo_count`: integer
- `demo_status`: MarketDemoStatus
- `generated_at`: string(date-time) _(required)_
- `observed_at`: unknown
- `real_count`: integer
- `scope`: MarketScope
- `signal_source_kind`: ForwardCurveSignalSourceKind
- `signal_type`: MarketSignalType _(required)_
- `unknown_count`: integer

### ForwardCurveSliceEvidencePoint

- `demo_status`: MarketDemoStatus
- `high_price_per_mt_usd`: unknown
- `layer`: ForwardCurveEvidenceLayer _(required)_
- `low_price_per_mt_usd`: unknown
- `observed_at`: unknown
- `price_per_mt_usd`: unknown
- `public_source_label`: string _(required)_
- `quantity_mt`: unknown
- `side`: unknown
- `source_kind`: MarketSourceKind

### ForwardCurveSliceResponse

- `cell`: ForwardCurveMarketCell _(required)_
- `depth_asks`: ForwardCurveBoardDepthLevel[]
- `depth_bids`: ForwardCurveBoardDepthLevel[]
- `disclaimer`: string
- `evidence_points`: ForwardCurveSliceEvidencePoint[]
- `fair_price_band`: unknown
- `generated_at`: string(date-time) _(required)_
- `indications`: ForwardCurveBoardIndication[]
- `next_window`: unknown
- `physical_stems`: ForwardCurveBoardPhysicalStem[]
- `previous_window`: unknown
- `trades`: ForwardCurveSliceTrade[]

### ForwardCurveSliceTrade

- `confirmed_at`: string(date-time) _(required)_
- `demo_status`: MarketDemoStatus _(required)_
- `price_per_mt_usd`: string _(required)_
- `quantity_mt`: string _(required)_
- `source_kind`: MarketSourceKind _(required)_

### ForwardCurveTableCell

- `best_ask`: unknown
- `best_bid`: unknown
- `demo_order_count`: integer
- `demo_status`: MarketDemoStatus
- `is_executable`: boolean
- `is_reference`: boolean
- `observed_at`: unknown
- `order_count`: integer
- `primary_signal_type`: MarketSignalType
- `primary_source_kind`: MarketSourceKind
- `primary_value`: unknown
- `public_source_label`: string
- `real_order_count`: integer
- `spread`: unknown
- `staleness_status`: ForwardCurveStalenessStatus
- `unknown_order_count`: integer
- `volume_mt`: string

### ForwardCurveTableColumn

- `availability_window`: string _(required)_
- `display_label`: string _(required)_
- `group`: string _(required)_

### ForwardCurveTableResponse

- `columns`: ForwardCurveTableColumn[] _(required)_
- `disclaimer`: string
- `generated_at`: string(date-time) _(required)_
- `latest_signals`: ForwardCurveLatestSignal[]
- `rows`: ForwardCurveTableRow[] _(required)_

### ForwardCurveTableRow

- `cells`: object _(required)_
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `delivery_point_name`: string _(required)_
- `market_product`: string _(required)_
- `product_count`: integer
- `product_name`: string _(required)_
- `region`: string _(required)_
- `representative_product_id`: string(uuid) _(required, uuid)_
- `row_key`: string _(required)_

### FrontendErrorPanel

- `by_category`: AggregateCell[]
- `by_route_family`: AggregateCell[]
- `total`: MetricValue _(required)_

### FuelEUResponse

- `compliance_balance_gco2`: string _(required)_
- `estimated_penalty_eur`: string _(required)_
- `ghg_intensity_gco2_mj`: string _(required)_
- `reduction_pct`: string _(required)_
- `score`: integer _(required)_
- `target_intensity_gco2_mj`: string _(required)_

### FunnelStage

- `conversion_from_previous_pct`: unknown
- `name`: string _(required)_
- `value`: integer _(required)_

### HTTPValidationError

- `detail`: ValidationError[]

### InventoryCreate

- `carbon_intensity_gco2_mj`: unknown
- `carbon_intensity_method`: unknown
- `certification_declared`: boolean
- `certification_scheme`: unknown
- `current_stock_mt`: unknown _(required)_
- `energy_density_mj_kg`: unknown
- `fame_terms`: unknown
- `feedstock`: unknown
- `fuel_type`: FuelType _(required)_
- `incoming_stock_mt`: unknown
- `is_certified`: boolean
- `lng_terms`: unknown
- `msds_available`: boolean
- `off_spec`: boolean
- `off_spec_notes`: unknown
- `origin`: unknown
- `port_id`: string _(required)_
- `price_per_mt_usd`: unknown
- `product_name`: unknown
- `specification_standard`: unknown

### InventoryItemUpdate

- `carbon_intensity_gco2_mj`: unknown
- `carbon_intensity_method`: unknown
- `certification_declared`: unknown
- `certification_scheme`: unknown
- `current_stock_mt`: unknown
- `fame_terms`: unknown
- `feedstock`: unknown
- `incoming_stock_mt`: unknown
- `lng_terms`: unknown
- `msds_available`: unknown
- `off_spec`: unknown
- `off_spec_notes`: unknown
- `origin`: unknown
- `price_per_mt_usd`: unknown
- `product_name`: unknown
- `specification_standard`: unknown

### InventoryResponse

- `carbon_intensity_gco2_mj`: unknown
- `carbon_intensity_method`: unknown
- `certification_declared`: boolean
- `certification_scheme`: unknown
- `current_stock_mt`: string _(required)_
- `energy_density_mj_kg`: unknown
- `fame_terms`: unknown
- `feedstock`: unknown
- `fuel_type`: FuelType _(required)_
- `id`: string(uuid) _(required, uuid)_
- `incoming_stock_mt`: string
- `is_certified`: boolean
- `lng_terms`: unknown
- `msds_available`: boolean
- `off_spec`: boolean
- `off_spec_notes`: unknown
- `origin`: unknown
- `port_id`: string _(required)_
- `price_per_mt_usd`: unknown
- `product_name`: unknown
- `reserved_stock_mt`: string _(required)_
- `specification_standard`: unknown
- `supplier_id`: string(uuid) _(required, uuid)_
- `updated_at`: string(date-time) _(required)_

### InvitationAcceptRequest

- `accept_terms`: boolean _(required)_
- `new_password`: string _(required)_
- `token`: string _(required)_

### InvitationOrganizationListResponse

- `items`: InvitationOrganizationResponse[] _(required)_

### InvitationOrganizationResponse

- `domain`: unknown _(required)_
- `id`: string(uuid) _(required, uuid)_
- `name`: string _(required)_
- `provenance`: OrganizationProvenance _(required)_
- `type`: string _(required)_

### InvitationResolveResponse

- `email`: string _(required)_
- `expires_at`: string(date-time) _(required)_
- `first_name`: unknown _(required)_
- `invited_by_name`: unknown _(required)_
- `last_name`: unknown _(required)_
- `organization_name`: string _(required)_
- `role`: string _(required)_

### InvitationTokenRequest

- `token`: string _(required)_

### JoinReviewBody

- `review_note`: string _(required)_

### JourneyStage

- `buyer`: unknown
- `key`: string _(required)_
- `source`: JourneySource _(required)_
- `supplier`: unknown
- `total`: AggregateCell _(required)_

### LabeledRatio

- `key`: string _(required)_
- `kind`: ConversionRatioKind _(required)_
- `ratio`: RatioValue _(required)_

### LeaderboardEntry

- `organization_name`: unknown
- `rank`: integer _(required)_
- `referral_count`: integer _(required)_
- `user_name`: string _(required)_

### LifecycleStage

- `count`: unknown
- `coverage`: AnalyticsSourceStatus _(required)_
- `detail_tab`: string _(required)_
- `key`: LifecycleStageKey _(required)_
- `previous`: unknown

### LiquiditySummary

- `crossed_slices`: unknown
- `median_hours_to_first_fill`: unknown
- `median_open_order_age_hours`: unknown
- `median_spread_bps`: unknown
- `median_spread_usd_per_mt`: unknown
- `one_sided_slices`: unknown
- `slices`: SliceLiquidityRow[]
- `two_sided_slices`: unknown

### ListingOverlay

- `ci_basis`: string _(required)_
- `ci_gco2_mj`: string _(required)_
- `financial_benefit_status`: string
- `lcv_basis`: string _(required)_
- `lcv_mj_kg`: string _(required)_
- `penalty_avoided_eur_per_mt`: string _(required)_
- `penalty_avoided_usd_per_mt`: string _(required)_
- `tco2e_avoided_per_mt`: string _(required)_

### LngAskTerms-Input

- `bio_share_pct`: unknown _(required)_
- `chain_of_custody`: string _(required)_
- `delivery_method`: string _(required)_
- `feedstock`: string _(required)_
- `feedstock_detail`: unknown
- `min_methane_number`: unknown
- `physical_supplier_name`: string _(required)_
- `pos_timing`: string _(required)_
- `production_country`: string _(required)_
- `scheme_certificate_number`: string _(required)_
- `seller_is_physical_supplier`: boolean _(required)_
- `side`: string _(required)_
- `specification_standard`: string _(required)_
- `sustainability_scheme`: string _(required)_
- `union_database_registered`: boolean _(required)_
- `wtt_ghg_gco2e_mj`: unknown _(required)_

### LngAskTerms-Output

- `bio_share_pct`: string _(required)_
- `chain_of_custody`: string _(required)_
- `delivery_method`: string _(required)_
- `feedstock`: string _(required)_
- `feedstock_detail`: unknown
- `min_methane_number`: unknown
- `physical_supplier_name`: string _(required)_
- `pos_timing`: string _(required)_
- `production_country`: string _(required)_
- `scheme_certificate_number`: string _(required)_
- `seller_is_physical_supplier`: boolean _(required)_
- `side`: string _(required)_
- `specification_standard`: string _(required)_
- `sustainability_scheme`: string _(required)_
- `union_database_registered`: boolean _(required)_
- `wtt_ghg_gco2e_mj`: string _(required)_

### LngBidTerms-Input

- `accept_resellers`: boolean
- `accepted_custody`: string[]
- `accepted_delivery_methods`: string[] _(required)_
- `accepted_schemes`: string[] _(required)_
- `max_wtt_ghg_gco2e_mj`: unknown
- `min_bio_share_pct`: unknown
- `min_methane_number`: unknown
- `require_union_database`: boolean
- `side`: string _(required)_

### LngBidTerms-Output

- `accept_resellers`: boolean
- `accepted_custody`: string[]
- `accepted_delivery_methods`: string[] _(required)_
- `accepted_schemes`: string[] _(required)_
- `max_wtt_ghg_gco2e_mj`: unknown
- `min_bio_share_pct`: unknown
- `min_methane_number`: unknown
- `require_union_database`: boolean
- `side`: string _(required)_

### LngPublicAskTerms

- `bio_share_pct`: string _(required)_
- `chain_of_custody`: string _(required)_
- `delivery_method`: string _(required)_
- `feedstock`: string _(required)_
- `feedstock_detail`: unknown
- `fueleu_recognition`: string _(required)_
- `min_methane_number`: unknown
- `pos_timing`: string _(required)_
- `production_country`: string _(required)_
- `reseller`: boolean _(required)_
- `seller_is_physical_supplier`: boolean _(required)_
- `side`: string _(required)_
- `specification_standard`: string _(required)_
- `sustainability_scheme`: string _(required)_
- `union_database_registered`: boolean _(required)_
- `wtt_ghg_gco2e_mj`: string _(required)_

### LngPublicTradeSnapshot

- `ask`: LngPublicAskTerms _(required)_
- `bid`: LngBidTerms-Output _(required)_

### LngTradeSnapshot

- `ask`: LngAskTerms-Output _(required)_
- `bid`: LngBidTerms-Output _(required)_

### LoginFailurePanel

- `categories`: AggregateCell[]
- `total`: MetricValue _(required)_
- `trend`: SeriesPoint[]

### LoginResponse

- `access_token`: string _(required)_
- `profile`: UserResponse _(required)_
- `token_type`: string _(required)_

### MapCompactMarketResponse

- `ask_max_price`: unknown
- `ask_min_price`: unknown
- `ask_order_count`: integer
- `ask_total_quantity`: string
- `bid_max_price`: unknown
- `bid_min_price`: unknown
- `bid_order_count`: integer
- `bid_total_quantity`: string
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `delivery_point_name`: string _(required)_
- `demo_status`: MarketDemoStatus _(required)_
- `evidence_class`: string _(required)_
- `fuel_type`: string _(required)_
- `market_product`: string _(required)_
- `observed_at`: string(date-time) _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `product_name`: string _(required)_
- `region`: string _(required)_
- `scope`: MarketScope
- `source_kind`: MarketSourceKind _(required)_
- `spot_best_ask`: unknown
- `spot_best_bid`: unknown

### MapCompactSummaryResponse

- `demo_groups`: AggregatedOrderbookResponse[] _(required)_
- `markets`: MapCompactMarketResponse[] _(required)_
- `recent_asks`: MapRecentAskResponse[] _(required)_

### MapRecentAskResponse

- `created_at`: string(date-time) _(required)_
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `delivery_point_name`: string _(required)_
- `demo_status`: MarketDemoStatus _(required)_
- `evidence_class`: string _(required)_
- `fuel_type`: string _(required)_
- `market_product`: string _(required)_
- `price_per_mt_usd`: string _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `product_name`: string _(required)_
- `region`: string _(required)_
- `remaining_quantity_mt`: string _(required)_
- `scope`: MarketScope
- `source_kind`: MarketSourceKind _(required)_

### MapSummaryResponse

- `groups`: AggregatedOrderbookResponse[] _(required)_
- `recent_asks`: MapRecentAskResponse[] _(required)_

### MarketActivitySection

- `balance_trend`: MarketBalanceTrend _(required)_
- `concentration`: OrganizationConcentration _(required)_
- `kpis`: MarketplaceKpis _(required)_
- `liquidity`: LiquiditySummary _(required)_
- `order_status_distribution`: AggregateCell[]
- `product_port_matrix`: ProductPortCell[]
- `trade_status_distribution`: AggregateCell[]
- `window_distribution`: RankedRow[]

### MarketBalanceTrend

- `asks`: SeriesPoint[]
- `bids`: SeriesPoint[]
- `buyer_organizations`: SeriesPoint[]
- `supplier_organizations`: SeriesPoint[]

### MarketSupportContextCreate

- `confirm_replacement`: boolean
- `organization_id`: string(uuid) _(required, uuid)_
- `support_reference`: string _(required)_

### MarketSupportContextResponse

- `actor`: MarketSupportPrincipal _(required)_
- `actor_user_id`: string(uuid) _(required, uuid)_
- `ended_at`: unknown _(required)_
- `expires_at`: string(date-time) _(required)_
- `id`: string(uuid) _(required, uuid)_
- `organization`: MarketSupportOrganization _(required)_
- `organization_id`: string(uuid) _(required, uuid)_
- `scope`: MarketSupportContextScope _(required)_
- `started_at`: string(date-time) _(required)_
- `status`: MarketSupportContextStatus _(required)_
- `support_reference`: string _(required)_
- `version`: integer _(required)_

### MarketSupportEntryResponse

- `eligible`: boolean
- `organization`: MarketSupportOrganization _(required)_
- `reason`: unknown

### MarketSupportFinalConfirmation

- `acknowledge_exact_terms`: boolean _(required)_
- `acknowledge_executable_standing_order`: boolean _(required)_
- `evidence_excerpt`: unknown
- `external_instruction_reference`: string _(required)_
- `instruction_at`: string(date-time) _(required)_

### MarketSupportOrganization

- `domain`: unknown _(required)_
- `id`: string(uuid) _(required, uuid)_
- `name`: string _(required)_
- `type`: string _(required)_

### MarketSupportPrincipal

- `email`: string _(required)_
- `id`: string(uuid) _(required, uuid)_
- `name`: string _(required)_

### MarketplaceBalance

- `ask_orders`: MetricValue _(required)_
- `bid_orders`: MetricValue _(required)_
- `buyer_organizations`: MetricValue _(required)_
- `supplier_organizations`: MetricValue _(required)_

### MarketplaceKpis

- `confirmed_trades`: MetricValue _(required)_
- `confirmed_volume_mt`: DecimalMetricValue _(required)_
- `execution_rate`: RatioValue _(required)_
- `open_asks`: MetricValue _(required)_
- `open_bids`: MetricValue _(required)_
- `participating_organizations`: MetricValue _(required)_

### MarketplaceResponse

- `commercial`: unknown
- `demo`: unknown
- `live`: unknown
- `meta`: AnalyticsMeta _(required)_
- `reference`: unknown
- `unknown`: unknown

### MetricEntry

- `name`: string _(required)_
- `value`: integer _(required)_

### MetricValue

- `previous`: unknown
- `suppressed`: boolean
- `value`: unknown

### NavigationDestinationRow

- `buyer`: unknown
- `destination`: NavigationDestination _(required)_
- `supplier`: unknown
- `total`: AggregateCell _(required)_

### NavigationLatencyRow

- `buckets`: AggregateCell[]
- `destination`: unknown _(required)_

### NeedsAttentionItem

- `count`: unknown
- `rule`: NeedsAttentionRule _(required)_

### NegotiationCounterRequest

- `notes`: unknown
- `proposed_price`: unknown _(required)_

### NegotiationCreateRequest

- `ask_order_id`: unknown
- `availability_window`: string
- `bid_order_id`: unknown
- `certification_declared`: boolean
- `counterparty_org_id`: string(uuid) _(required, uuid)_
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `expires_in_hours`: integer
- `fame_terms`: unknown
- `lng_terms`: unknown
- `msds_available`: boolean
- `notes`: unknown
- `product_id`: string(uuid) _(required, uuid)_
- `proposed_price`: unknown _(required)_
- `quantity_mt`: unknown _(required)_

### NegotiationListResponse

- `items`: NegotiationResponse[] _(required)_
- `total`: integer _(required)_

### NegotiationResponse

- `accepted_by_user_id`: unknown
- `ask_order_id`: unknown
- `availability_window`: string
- `bid_order_id`: unknown
- `counterparty_org_id`: string(uuid) _(required, uuid)_
- `counterparty_org_name`: unknown
- `counterparty_user_id`: unknown
- `created_at`: string(date-time) _(required)_
- `current_price`: string _(required)_
- `delivery_point_id`: unknown
- `expires_at`: string(date-time) _(required)_
- `fame_terms_snapshot`: unknown
- `id`: string(uuid) _(required, uuid)_
- `initiator_org_id`: string(uuid) _(required, uuid)_
- `initiator_org_name`: unknown
- `initiator_side`: string _(required)_
- `initiator_user_id`: unknown
- `last_actor_org_id`: string(uuid) _(required, uuid)_
- `lng_terms_snapshot`: unknown
- `product_id`: string(uuid) _(required, uuid)_
- `product_name`: unknown
- `quantity_mt`: string _(required)_
- `rounds`: NegotiationRoundResponse[]
- `status`: string _(required)_
- `trade_id`: unknown
- `updated_at`: string(date-time) _(required)_

### NegotiationRoundResponse

- `created_at`: string(date-time) _(required)_
- `id`: string(uuid) _(required, uuid)_
- `notes`: unknown
- `proposed_price`: string _(required)_
- `proposer_org_id`: string(uuid) _(required, uuid)_
- `proposer_org_name`: unknown
- `proposer_user_id`: unknown
- `round_number`: integer _(required)_

### NotificationResponse

- `created_at`: string(date-time) _(required)_
- `data`: unknown _(required)_
- `id`: string(uuid4) _(required)_
- `is_read`: boolean _(required)_
- `message`: string _(required)_
- `title`: string _(required)_
- `type`: string _(required)_

### OrderBookSnapshotResponse

- `asks`: OrderResponse[]
- `availability_window`: string _(required)_
- `bids`: OrderResponse[]
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `demo_status`: MarketDemoStatus _(required)_
- `generated_at`: string(date-time) _(required)_
- `market_product`: string _(required)_
- `scope`: MarketScope
- `source_kind`: MarketSourceKind _(required)_

### OrderCancelRequest

- `reason`: string _(required)_

### OrderCreate-Input

- `availability_window`: string
- `carbon_intensity_gco2_mj`: unknown
- `carbon_intensity_method`: unknown
- `certification_declared`: boolean
- `certification_scheme`: unknown
- `certifications`: string[]
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `expires_at`: unknown
- `fame_terms`: unknown
- `feedstock`: unknown
- `is_anonymous`: boolean
- `lng_terms`: unknown
- `msds_available`: boolean
- `off_spec`: boolean
- `off_spec_notes`: unknown
- `origin`: unknown
- `port_id`: unknown
- `price_per_mt_usd`: unknown _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `quantity_mt`: unknown _(required)_
- `side`: OrderSide _(required)_
- `specification_standard`: unknown
- `support_confirmation`: unknown
- `vessel_id`: unknown

### OrderCreate-Output

- `availability_window`: string
- `carbon_intensity_gco2_mj`: unknown
- `carbon_intensity_method`: unknown
- `certification_declared`: boolean
- `certification_scheme`: unknown
- `certifications`: string[]
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `expires_at`: unknown
- `fame_terms`: unknown
- `feedstock`: unknown
- `is_anonymous`: boolean
- `lng_terms`: unknown
- `msds_available`: boolean
- `off_spec`: boolean
- `off_spec_notes`: unknown
- `origin`: unknown
- `port_id`: unknown
- `price_per_mt_usd`: string _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `quantity_mt`: string _(required)_
- `side`: OrderSide _(required)_
- `specification_standard`: unknown
- `support_confirmation`: unknown
- `vessel_id`: unknown

### OrderMyResponse

- `availability_window`: string _(required)_
- `benchmark_price_per_mt_usd`: unknown
- `benchmark_source`: unknown
- `carbon_intensity_gco2_mj`: unknown
- `carbon_intensity_method`: unknown
- `certification_declared`: boolean
- `certification_scheme`: unknown
- `certifications`: string[]
- `created_at`: string(date-time) _(required)_
- `creation_method`: string
- `delivery_point_id`: unknown
- `delivery_point_name`: unknown
- `demo_status`: MarketDemoStatus
- `etag`: unknown
- `expires_at`: unknown
- `fame_terms`: unknown
- `feedstock`: unknown
- `fuel_grade`: string
- `fuel_type`: string
- `id`: string(uuid) _(required, uuid)_
- `is_crossed`: boolean
- `is_demo_listing`: boolean
- `is_verdaxis_verified`: boolean _(required)_
- `lng_terms`: unknown
- `market_product`: unknown
- `msds_available`: boolean
- `off_spec`: boolean
- `off_spec_notes`: unknown
- `organization_id`: string(uuid) _(required, uuid)_
- `origin`: unknown
- `port_id`: unknown
- `premium_discount_per_mt_usd`: unknown
- `price_per_mt_usd`: string _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `product_name`: string
- `quantity_mt`: string _(required)_
- `region`: string
- `remaining_quantity_mt`: string _(required)_
- `scope`: MarketScope
- `side`: OrderSide _(required)_
- `source_kind`: MarketSourceKind
- `specification_standard`: unknown
- `status`: OrderBookStatus _(required)_
- `terms_digest`: string _(required)_
- `tier_label`: TierLabel
- `trade_count`: integer
- `unknown_count`: integer
- `updated_at`: string(date-time) _(required)_
- `version`: integer
- `vessel_id`: unknown

### OrderResponse

- `availability_window`: string _(required)_
- `benchmark_price_per_mt_usd`: unknown
- `benchmark_source`: unknown
- `carbon_intensity_gco2_mj`: unknown
- `carbon_intensity_method`: unknown
- `certification_declared`: boolean
- `certification_scheme`: unknown
- `certifications`: string[]
- `created_at`: string(date-time) _(required)_
- `delivery_point_id`: unknown
- `delivery_point_name`: unknown
- `demo_status`: MarketDemoStatus
- `expires_at`: unknown
- `fame_terms`: unknown
- `feedstock`: unknown
- `fuel_grade`: string
- `fuel_type`: string
- `id`: string(uuid) _(required, uuid)_
- `is_crossed`: boolean
- `is_demo_listing`: boolean
- `is_verdaxis_verified`: boolean _(required)_
- `lng_terms`: unknown
- `market_product`: unknown
- `msds_available`: boolean
- `off_spec`: boolean
- `off_spec_notes`: unknown
- `origin`: unknown
- `port_id`: unknown
- `premium_discount_per_mt_usd`: unknown
- `price_per_mt_usd`: string _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `product_name`: string
- `quantity_mt`: string _(required)_
- `region`: string
- `remaining_quantity_mt`: string _(required)_
- `scope`: MarketScope
- `side`: OrderSide _(required)_
- `source_kind`: MarketSourceKind
- `specification_standard`: unknown
- `status`: OrderBookStatus _(required)_
- `terms_digest`: string _(required)_
- `tier_label`: TierLabel
- `unknown_count`: integer
- `version`: integer

### OrderResponseWithCI

- `availability_window`: string _(required)_
- `benchmark_price_per_mt_usd`: unknown
- `benchmark_source`: unknown
- `carbon_intensity_gco2_mj`: unknown
- `carbon_intensity_method`: unknown
- `certification_declared`: boolean
- `certification_scheme`: unknown
- `certifications`: string[]
- `ci_adjusted_price`: unknown
- `created_at`: string(date-time) _(required)_
- `delivery_point_id`: unknown
- `delivery_point_name`: unknown
- `demo_status`: MarketDemoStatus
- `expires_at`: unknown
- `fame_terms`: unknown
- `feedstock`: unknown
- `fuel_grade`: string
- `fuel_type`: string
- `id`: string(uuid) _(required, uuid)_
- `is_crossed`: boolean
- `is_demo_listing`: boolean
- `is_verdaxis_verified`: boolean _(required)_
- `lng_terms`: unknown
- `market_product`: unknown
- `msds_available`: boolean
- `off_spec`: boolean
- `off_spec_notes`: unknown
- `origin`: unknown
- `port_id`: unknown
- `premium_discount_per_mt_usd`: unknown
- `price_per_mt_usd`: string _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `product_name`: string
- `quantity_mt`: string _(required)_
- `region`: string
- `remaining_quantity_mt`: string _(required)_
- `scope`: MarketScope
- `side`: OrderSide _(required)_
- `source_kind`: MarketSourceKind
- `specification_standard`: unknown
- `status`: OrderBookStatus _(required)_
- `terms_digest`: string _(required)_
- `tier_label`: TierLabel
- `unknown_count`: integer
- `version`: integer

### OrderUpdate

- `availability_window`: unknown
- `carbon_intensity_gco2_mj`: unknown
- `carbon_intensity_method`: unknown
- `certification_declared`: unknown
- `certification_scheme`: unknown
- `certifications`: unknown
- `expires_at`: unknown
- `fame_terms`: unknown
- `feedstock`: unknown
- `lng_terms`: unknown
- `msds_available`: unknown
- `off_spec`: unknown
- `off_spec_notes`: unknown
- `origin`: unknown
- `price_per_mt_usd`: unknown
- `quantity_mt`: unknown
- `specification_standard`: unknown

### OrganizationConcentration

- `hhi_band`: unknown
- `suppressed`: boolean

### OrganizationCreate

- `country_code`: unknown
- `name`: string _(required)_
- `tax_id`: unknown
- `type`: OrgType _(required)_

### OverlayAssumptions

- `eur_usd_rate`: string _(required)_
- `excluded_factors`: string[] _(required)_
- `fleet_intensity_basis`: string _(required)_
- `fleet_vessel_count`: integer _(required)_
- `ghgie_actual_gco2_mj`: string _(required)_
- `penalty_eur_per_tonne`: string _(required)_
- `vlsfo_baseline_gco2_mj`: string _(required)_
- `year`: integer _(required)_
- `year_target`: string _(required)_

### OverviewKpis

- `active_members`: MetricValue _(required)_
- `confirmed_trades`: MetricValue _(required)_
- `live_orders`: MetricValue _(required)_
- `participating_organizations`: MetricValue _(required)_
- `qualified_organizations`: MetricValue _(required)_

### PaginatedResponse_AssistedListingResponse_

- `items`: AssistedListingResponse[] _(required)_
- `limit`: integer _(required)_
- `skip`: integer _(required)_
- `total`: integer _(required)_

### PaginatedResponse_AuthorizationResponse_

- `items`: AuthorizationResponse[] _(required)_
- `limit`: integer _(required)_
- `skip`: integer _(required)_
- `total`: integer _(required)_

### PaginatedResponse_MarketSupportOrganization_

- `items`: MarketSupportOrganization[] _(required)_
- `limit`: integer _(required)_
- `skip`: integer _(required)_
- `total`: integer _(required)_

### PaginatedResponse_OrderResponse_

- `items`: OrderResponse[] _(required)_
- `limit`: integer _(required)_
- `skip`: integer _(required)_
- `total`: integer _(required)_

### PaginatedResponse_TradeResponse_

- `items`: TradeResponse[] _(required)_
- `limit`: integer _(required)_
- `skip`: integer _(required)_
- `total`: integer _(required)_

### PasswordChangeRequest

- `current_password`: string _(required)_
- `new_password`: string _(required)_

### PinTargetCreate

- `order_id`: string(uuid) _(required, uuid)_
- `target_type`: string _(required)_

### PortFuelAvailability

- `availability_level`: AvailabilityLevel _(required)_
- `avg_price_per_mt`: unknown
- `demo_status`: MarketDemoStatus
- `fuel_type`: string _(required)_
- `lat`: number _(required)_
- `lng`: number _(required)_
- `market_product_code`: string _(required)_
- `port_id`: string _(required)_
- `port_name`: string _(required)_
- `scope`: MarketScope
- `source_kind`: MarketSourceKind
- `supplier_count`: integer _(required)_
- `total_stock_mt`: unknown _(required)_
- `unknown_count`: integer

### PortIntelligenceBase

- `biofuel_price_avg`: unknown
- `captured_at`: string(date-time) _(required)_
- `congestion_level`: unknown
- `methanol_price_avg`: unknown

### PortResponse

- `country`: string _(required)_
- `id`: string _(required)_
- `intelligence`: unknown
- `is_active`: boolean
- `lat`: unknown
- `lng`: unknown
- `location`: unknown
- `name`: string _(required)_
- `timezone`: unknown

### PriceDiscoveryResponse

- `generated_at`: string(date-time) _(required)_
- `summaries`: PriceSummary[] _(required)_

### PriceSummary

- `availability_window`: string
- `avg_price_24h`: unknown
- `delivery_point_id`: unknown
- `delivery_point_name`: unknown
- `demo_status`: MarketDemoStatus
- `demo_trade_count_24h`: integer
- `fuel_type`: string
- `high_24h`: unknown
- `is_reference`: boolean
- `last_price`: unknown
- `last_trade_at`: unknown
- `low_24h`: unknown
- `market_product`: unknown
- `observed_at`: unknown
- `price_change_pct`: unknown
- `product_id`: unknown
- `product_name`: string
- `real_trade_count_24h`: integer
- `region`: string
- `scope`: MarketScope
- `source_kind`: MarketSourceKind
- `trade_count_24h`: integer
- `unknown_trade_count_24h`: integer
- `volume_24h`: string

### PricingOverlayRequest

- `order_ids`: string[] _(required)_
- `year`: integer

### PricingOverlayResponse

- `assumptions`: OverlayAssumptions _(required)_
- `overlays`: object _(required)_

### ProducerProjectResponse

- `capacity_kt_per_year`: unknown
- `carbon_intensity_gco2_mj`: unknown
- `cod_date`: unknown
- `cod_year`: unknown
- `country`: string _(required)_
- `created_at`: string(date-time) _(required)_
- `data_source`: unknown
- `feedstock`: unknown
- `fuel_type`: string _(required)_
- `gena_project_id`: unknown
- `id`: string(uuid) _(required, uuid)_
- `lat`: unknown
- `lng`: unknown
- `name`: string _(required)_
- `notes`: unknown
- `organization_id`: unknown
- `region`: unknown
- `status`: string _(required)_
- `technology`: unknown

### ProductCountResponse

- `counts`: object _(required)_
- `total`: integer _(required)_

### ProductPortCell

- `delivery_point_key`: string _(required)_
- `delivery_point_label`: string _(required)_
- `orders`: AggregateCell _(required)_
- `organizations`: AggregateCell _(required)_
- `product_key`: string _(required)_
- `product_label`: string _(required)_

### ProductResponse

- `available_delivery_point_ids`: string[] _(required)_
- `execution_mode`: string _(required)_
- `fuel_grade`: string _(required)_
- `fuel_type`: string _(required)_
- `id`: string(uuid) _(required, uuid)_
- `is_active`: boolean _(required)_
- `market_product`: unknown
- `min_lot_size`: string _(required)_
- `name`: string _(required)_
- `spec_description`: unknown
- `unit`: string _(required)_

### ProductUsageResponse

- `authoritative`: AuthoritativeUsage _(required)_
- `behavioral`: BehavioralUsage _(required)_
- `behavioral_status`: string _(required)_
- `days`: integer _(required)_
- `diagnostic`: unknown _(required)_
- `funnel`: FunnelStage[] _(required)_
- `observed_at`: string(date-time) _(required)_
- `period_end`: string(date-time) _(required)_
- `period_start`: string(date-time) _(required)_

### RFQCreateRequest

- `availability_window`: string
- `contract_terms`: unknown
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `expected_source_offer_revision`: unknown
- `expires_in_hours`: integer
- `is_anonymous`: boolean
- `notes`: unknown
- `product_id`: string(uuid) _(required, uuid)_
- `quantity_mt`: unknown _(required)_
- `source_offer_id`: unknown
- `target_price_per_mt`: unknown

### RFQListResponse

- `items`: RFQResponse[] _(required)_
- `total`: integer _(required)_

### RFQQuoteRequest

- `expires_at`: unknown
- `notes`: unknown
- `offer_terms`: unknown
- `price_per_mt_usd`: unknown _(required)_

### RFQQuoteResponse

- `created_at`: string(date-time) _(required)_
- `execution_enabled`: boolean
- `expires_at`: unknown
- `id`: string(uuid) _(required, uuid)_
- `is_expired`: boolean
- `notes`: unknown
- `offer_terms`: unknown
- `price_per_mt_usd`: string _(required)_
- `revision`: integer
- `seller_org_id`: string(uuid) _(required, uuid)_
- `seller_org_name`: unknown
- `status`: string _(required)_

### RFQQuoteRevisionRequest

- `expected_revision`: integer _(required)_
- `expires_at`: unknown
- `notes`: unknown
- `offer_terms`: unknown
- `price_per_mt_usd`: unknown _(required)_

### RFQResponse

- `availability_window`: string _(required)_
- `buyer_org_id`: unknown
- `buyer_org_name`: unknown
- `can_cancel`: boolean
- `contract_terms`: unknown
- `created_at`: string(date-time) _(required)_
- `delivery_point_id`: unknown
- `delivery_point_name`: unknown
- `execution_enabled`: boolean
- `expires_at`: string(date-time) _(required)_
- `id`: string(uuid) _(required, uuid)_
- `is_anonymous`: boolean _(required)_
- `notes`: unknown
- `product_id`: string(uuid) _(required, uuid)_
- `product_name`: unknown
- `quantity_mt`: string _(required)_
- `quote_count`: integer
- `quotes`: RFQQuoteResponse[]
- `source_offer_id`: unknown
- `source_offer_snapshot`: unknown
- `status`: string _(required)_
- `target_price_per_mt`: unknown
- `target_supplier_org_id`: unknown

### RankedRow

- `count`: unknown
- `key`: string _(required)_
- `label`: string _(required)_
- `share_pct`: unknown
- `suppressed`: boolean

### RatioValue

- `cohort_complete`: boolean
- `denominator`: unknown
- `numerator`: unknown
- `rate_pct`: unknown
- `suppressed`: boolean

### ReferenceCoverageRow

- `availability_window`: string _(required)_
- `availability_window_label`: string _(required)_
- `benchmark_price_usd_per_mt`: unknown
- `coverage_status`: string _(required)_
- `delivery_point_key`: string _(required)_
- `delivery_point_label`: string _(required)_
- `generated_at`: string(date-time) _(required)_
- `observed_at`: unknown
- `product_key`: string _(required)_
- `product_label`: string _(required)_
- `scope`: ReferenceScope _(required)_
- `source_kind`: ReferenceSourceKind _(required)_
- `source_label`: ReferenceSourceLabel _(required)_

### ReferenceCoverageSection

- `rows`: ReferenceCoverageRow[]

### ReferencePriceItem

- `availability_window`: string
- `date`: string(date) _(required)_
- `delivery_point_id`: unknown
- `delivery_point_name`: unknown
- `demo_status`: MarketDemoStatus
- `fuel_type`: string
- `is_reference`: boolean
- `market_product`: unknown
- `product_id`: unknown
- `product_name`: string
- `region`: string
- `scope`: MarketScope
- `source_kind`: MarketSourceKind
- `total_volume_mt`: string _(required)_
- `trade_count`: integer _(required)_
- `visibility`: string
- `vwap_usd`: string _(required)_

### ReferencePriceResponse

- `generated_at`: string(date-time) _(required)_
- `prices`: ReferencePriceItem[] _(required)_

### ReferralCodeResponse

- `referral_code`: string _(required)_
- `referral_link`: string _(required)_

### ReferralInviteRequest

- `email`: string(email) _(required)_

### ReferralListItem

- `organization_name`: unknown
- `role`: unknown
- `signed_up_at`: string(date-time) _(required)_
- `status`: string _(required)_

### ReferralStatsResponse

- `active`: integer _(required)_
- `referrals`: ReferralListItem[] _(required)_
- `total`: integer _(required)_
- `verified`: integer _(required)_

### RefreshRequest

- `refresh_token`: unknown

### RegisterWithOrgRequest

- `organization`: OrganizationCreate _(required)_
- `registration_token`: string _(required)_

### RegistrationResponse

- `registration_token`: unknown
- `status`: string _(required)_
- `user`: unknown

### ReliabilityResponse

- `audit_activity`: AuditActivityRow[]
- `backend_unavailable`: BackendUnavailablePanel _(required)_
- `collector`: CollectorState _(required)_
- `frontend_errors`: FrontendErrorPanel _(required)_
- `login_failures`: LoginFailurePanel _(required)_
- `meta`: AnalyticsMeta _(required)_
- `navigation_latency`: NavigationLatencyRow[]

### ResendVerificationRequest

- `email`: string _(required)_

### ResetPasswordRequest

- `new_password`: string _(required)_
- `token`: string _(required)_

### ResolveCodeResponse

- `organization_name`: unknown
- `organization_type`: unknown
- `referrer_name`: unknown
- `valid`: boolean _(required)_

### RetentionKpis

- `dormant_approved_members`: MetricValue _(required)_
- `reactivated_organizations`: MetricValue _(required)_
- `retained_organizations`: MetricValue _(required)_
- `returning_members`: MetricValue _(required)_

### RetentionResponse

- `kpis`: RetentionKpis _(required)_
- `member_cohorts`: CohortRow[]
- `meta`: AnalyticsMeta _(required)_
- `organization_cohorts`: CohortRow[]
- `repeat_participation`: AggregateCell[]

### ScenarioInput

- `fuel_mix`: object _(required)_
- `vessel_id`: string _(required)_
- `year`: integer

### SeriesPoint

- `date`: string(date) _(required)_
- `value`: unknown

### SliceLiquidityRow

- `availability_window`: string _(required)_
- `availability_window_label`: string _(required)_
- `best_ask_depth_mt`: unknown
- `best_ask_usd_per_mt`: unknown
- `best_bid_depth_mt`: unknown
- `best_bid_usd_per_mt`: unknown
- `contributing_organizations`: unknown
- `crossed`: unknown
- `delivery_point_key`: string _(required)_
- `delivery_point_label`: string _(required)_
- `one_percent_ask_depth_mt`: unknown
- `one_percent_bid_depth_mt`: unknown
- `product_key`: string _(required)_
- `product_label`: string _(required)_
- `spread_bps`: unknown
- `spread_usd_per_mt`: unknown
- `suppressed`: boolean

### SliceTargetCreate

- `availability_window_code`: string _(required)_
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `market_product_code`: string _(required)_
- `target_type`: string _(required)_

### SubscriptionResponse

- `expires_at`: unknown
- `id`: string(uuid) _(required, uuid)_
- `is_active`: boolean _(required)_
- `org_id`: string(uuid) _(required, uuid)_
- `seller_fee_per_mt_usd`: unknown
- `started_at`: unknown
- `tier`: SubscriptionTier _(required)_

### SubscriptionUpdate

- `seller_fee_per_mt_usd`: unknown
- `tier`: SubscriptionTier _(required)_

### SupplierListingTemplateResponse

- `availability_window`: string _(required)_
- `carbon_intensity_gco2_mj`: unknown
- `carbon_intensity_method`: unknown
- `certification_declared`: boolean
- `certification_scheme`: unknown
- `certifications`: string[]
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `fame_terms`: unknown
- `feedstock`: unknown
- `lng_terms`: unknown
- `msds_available`: boolean
- `off_spec`: boolean
- `off_spec_notes`: unknown
- `origin`: unknown
- `price_per_mt_usd`: string _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `quantity_mt`: string _(required)_
- `specification_standard`: unknown

### SupplierOfferCreate

- `availability_window`: string
- `claims_terms`: unknown
- `delivery_basis`: string _(required)_
- `delivery_end`: string(date) _(required)_
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `delivery_start`: string(date) _(required)_
- `evidence_due`: string _(required)_
- `expires_at`: string(date-time) _(required)_
- `fuel_terms`: FameListingFuelTerms-Input _(required)_
- `inspection_terms`: unknown
- `min_fill_mt`: unknown _(required)_
- `named_location`: string _(required)_
- `notes`: unknown
- `payment_terms`: unknown
- `price_per_mt_usd`: unknown _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `quantity_mt`: unknown _(required)_
- `quantity_tolerance_pct`: unknown _(required)_
- `title_risk_terms`: unknown

### SupplierOfferList

- `items`: SupplierOfferResponse[] _(required)_
- `limit`: integer _(required)_
- `skip`: integer _(required)_
- `total`: integer _(required)_

### SupplierOfferPublicSnapshot

- `availability_window`: string _(required)_
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `expires_at`: string(date-time) _(required)_
- `listing_terms`: SupplierOfferPublicTerms _(required)_
- `min_fill_mt`: string _(required)_
- `offer_id`: string(uuid) _(required, uuid)_
- `price_per_mt_usd`: string _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `quantity_mt`: string _(required)_
- `revision`: integer _(required)_

### SupplierOfferPublicTerms

- `claims_terms`: unknown
- `delivery_basis`: string _(required)_
- `delivery_end`: string(date) _(required)_
- `delivery_start`: string(date) _(required)_
- `evidence_due`: string _(required)_
- `fuel_terms`: FameFuelSummary _(required)_
- `inspection_terms`: unknown
- `named_location`: string _(required)_
- `notes`: unknown
- `payment_terms`: unknown
- `quantity_tolerance_pct`: string _(required)_
- `title_risk_terms`: unknown

### SupplierOfferResponse

- `availability_window`: string _(required)_
- `can_edit`: boolean
- `can_request_quote`: boolean
- `can_withdraw`: boolean
- `claims_terms`: unknown
- `created_at`: string(date-time) _(required)_
- `delivery_basis`: string _(required)_
- `delivery_end`: string(date) _(required)_
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `delivery_point_name`: string
- `delivery_start`: string(date) _(required)_
- `evidence_due`: string _(required)_
- `execution_enabled`: boolean
- `expires_at`: string(date-time) _(required)_
- `fuel_terms`: unknown _(required)_
- `id`: string(uuid) _(required, uuid)_
- `inspection_terms`: unknown
- `listing_kind`: string
- `market_product`: string
- `min_fill_mt`: string _(required)_
- `named_location`: string _(required)_
- `notes`: unknown
- `payment_terms`: unknown
- `price_per_mt_usd`: string _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `product_name`: string
- `quantity_mt`: string _(required)_
- `quantity_tolerance_pct`: string _(required)_
- `revision`: integer _(required)_
- `status`: string _(required)_
- `title_risk_terms`: unknown
- `updated_at`: string(date-time) _(required)_

### SupplierOfferSnapshot

- `availability_window`: string _(required)_
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `expires_at`: string(date-time) _(required)_
- `listing_terms`: SupplierOfferTerms _(required)_
- `min_fill_mt`: string _(required)_
- `offer_id`: string(uuid) _(required, uuid)_
- `price_per_mt_usd`: string _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `quantity_mt`: string _(required)_
- `revision`: integer _(required)_
- `supplier_org_id`: string(uuid) _(required, uuid)_

### SupplierOfferTerms

- `claims_terms`: unknown
- `delivery_basis`: string _(required)_
- `delivery_end`: string(date) _(required)_
- `delivery_start`: string(date) _(required)_
- `evidence_due`: string _(required)_
- `fuel_terms`: FameListingFuelTerms-Output _(required)_
- `inspection_terms`: unknown
- `named_location`: string _(required)_
- `notes`: unknown
- `payment_terms`: unknown
- `quantity_tolerance_pct`: string _(required)_
- `title_risk_terms`: unknown

### SupplierOfferUpdate

- `availability_window`: string
- `claims_terms`: unknown
- `delivery_basis`: string _(required)_
- `delivery_end`: string(date) _(required)_
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `delivery_start`: string(date) _(required)_
- `evidence_due`: string _(required)_
- `expected_revision`: integer _(required)_
- `expires_at`: string(date-time) _(required)_
- `fuel_terms`: FameListingFuelTerms-Input _(required)_
- `inspection_terms`: unknown
- `min_fill_mt`: unknown _(required)_
- `named_location`: string _(required)_
- `notes`: unknown
- `payment_terms`: unknown
- `price_per_mt_usd`: unknown _(required)_
- `product_id`: string(uuid) _(required, uuid)_
- `quantity_mt`: unknown _(required)_
- `quantity_tolerance_pct`: unknown _(required)_
- `title_risk_terms`: unknown

### SupplierOfferWithdraw

- `expected_revision`: integer _(required)_

### SurveySubmission

- `referral_source`: unknown
- `use_case`: string _(required)_

### TradeCreate

- `certification_declared`: boolean
- `expected_order_version`: unknown
- `expected_terms_digest`: unknown
- `fame_terms`: unknown
- `lng_terms`: unknown
- `msds_available`: boolean
- `order_id`: string(uuid) _(required, uuid)_
- `quantity_mt`: unknown _(required)_

### TradeDeliverPayload

- `final_price_per_mt`: unknown _(required)_
- `final_quantity_mt`: unknown _(required)_

### TradeResponse

- `ask_order_id`: unknown
- `availability_window`: unknown
- `bid_order_id`: unknown
- `buyer_id`: unknown
- `buyer_name`: string
- `commission_amount_usd`: unknown
- `commission_fee_per_mt_usd`: unknown
- `commission_payer`: unknown
- `commission_plan`: unknown
- `commission_rate_pct`: string
- `confirmed_at`: unknown
- `created_at`: string(date-time) _(required)_
- `delivered_at`: unknown
- `delivery_point_id`: unknown
- `delivery_point_name`: unknown
- `demo_status`: MarketDemoStatus
- `fame_terms_snapshot`: unknown
- `final_price_per_mt`: unknown
- `final_quantity_mt`: unknown
- `final_total_usd`: unknown
- `fuel_grade`: string
- `fuel_type`: string
- `id`: string(uuid) _(required, uuid)_
- `initiated_by`: Initiator _(required)_
- `is_anonymous`: boolean
- `lng_terms_snapshot`: unknown
- `market_product`: unknown
- `paid_at`: unknown
- `price_per_mt_usd`: string _(required)_
- `product_id`: unknown
- `product_name`: string
- `quantity_mt`: string _(required)_
- `region`: string
- `scope`: MarketScope
- `seller_id`: unknown
- `seller_name`: string
- `source_kind`: MarketSourceKind
- `status`: TradeStatus _(required)_
- `unknown_count`: integer

### TradeSummaryResponse

- `action_required_count`: integer _(required)_
- `awaiting_counterparty_count`: integer _(required)_
- `confirmed_count`: integer _(required)_
- `total_count`: integer _(required)_

### TradeTapeEntry

- `availability_window`: string _(required)_
- `confirmed_at`: string(date-time) _(required)_
- `delivery_point_id`: unknown
- `delivery_point_name`: unknown
- `demo_status`: MarketDemoStatus
- `fuel_grade`: string _(required)_
- `fuel_type`: string _(required)_
- `id`: string _(required)_
- `is_demo_trade`: boolean
- `market_product`: unknown
- `price_per_mt_usd`: string _(required)_
- `product_id`: unknown
- `provenance_kind`: string
- `quantity_mt`: string _(required)_
- `region`: string _(required)_
- `scope`: string
- `source_kind`: MarketSourceKind
- `total_usd`: string _(required)_

### TradeTapeResponse

- `items`: TradeTapeEntry[] _(required)_
- `market_hours`: boolean _(required)_
- `total`: integer _(required)_

### TutorialStepRow

- `completed`: AggregateCell _(required)_
- `skipped`: AggregateCell _(required)_
- `step`: string _(required)_

### UserCreate

- `email`: string(email) _(required)_
- `first_name`: unknown
- `last_name`: unknown
- `organization_id`: unknown
- `password`: string _(required)_
- `referral_code`: unknown
- `role`: UserRole _(required)_

### UserResponse

- `email`: string _(required)_
- `execution_eligibility`: ExecutionEligibilityResponse
- `first_name`: unknown
- `id`: string(uuid) _(required, uuid)_
- `kyc_status`: string
- `last_name`: unknown
- `must_change_password`: boolean
- `organization_id`: unknown
- `referral_code`: unknown
- `role`: unknown
- `status`: UserStatus _(required)_

### UserUpdate

- `first_name`: unknown
- `last_name`: unknown
- `role`: unknown

### ValidationError

- `ctx`: object
- `input`: unknown
- `loc`: any[] _(required)_
- `msg`: string _(required)_
- `type`: string _(required)_

### VesselResponse

- `cii_rating`: unknown
- `current_location`: unknown
- `dwt`: unknown
- `eu_ets_status`: unknown
- `flag_state`: unknown
- `fueleu_status`: unknown
- `id`: string _(required)_
- `imo_number`: string _(required)_
- `lat`: unknown
- `lng`: unknown
- `name`: string _(required)_
- `organization_id`: unknown
- `prev_lat`: unknown
- `prev_lng`: unknown
- `previous_location`: unknown
- `updated_at`: string(date-time) _(required)_
- `vessel_type`: unknown

### WatchlistCreateRequest

- `name`: string _(required)_

### WatchlistDetailResponse

- `created_at`: string(date-time) _(required)_
- `has_more_slices`: boolean
- `id`: string(uuid) _(required, uuid)_
- `kind`: string _(required)_
- `latest_event_at`: unknown
- `name`: string _(required)_
- `slices`: WatchlistSliceResponse[]
- `total_slice_count`: integer
- `unread_event_count`: integer

### WatchlistEntryAddRequest

- `delivery_point_id`: unknown
- `product_id`: string(uuid) _(required, uuid)_

### WatchlistEntryResponse

- `best_ask`: unknown
- `best_bid`: unknown
- `created_at`: string(date-time) _(required)_
- `delivery_point_id`: unknown
- `delivery_point_name`: unknown
- `id`: string(uuid) _(required, uuid)_
- `product_id`: string(uuid) _(required, uuid)_
- `product_name`: unknown

### WatchlistEventResponse

- `availability_window_code`: unknown
- `created_at`: string(date-time) _(required)_
- `delivery_point_id`: unknown
- `delivery_point_name`: unknown
- `demo_status`: MarketDemoStatus
- `event_payload`: object
- `event_type`: string _(required)_
- `id`: string(uuid) _(required, uuid)_
- `is_read`: boolean _(required)_
- `market_product_code`: unknown
- `observed_at`: unknown
- `order_id`: unknown
- `scope`: MarketScope
- `source_kind`: MarketSourceKind
- `target_type`: string _(required)_
- `watchlist_id`: string(uuid) _(required, uuid)_
- `watchlist_target_id`: string(uuid) _(required, uuid)_

### WatchlistEventsPageResponse

- `items`: WatchlistEventResponse[]
- `next_cursor`: unknown

### WatchlistResponse

- `created_at`: string(date-time) _(required)_
- `entries`: WatchlistEntryResponse[]
- `entry_count`: integer
- `id`: string(uuid) _(required, uuid)_
- `name`: string _(required)_

### WatchlistSliceResponse

- `active_order_count`: integer
- `availability_window_code`: string _(required)_
- `created_at`: string(date-time) _(required)_
- `delivery_point_id`: string(uuid) _(required, uuid)_
- `delivery_point_name`: unknown
- `id`: string(uuid) _(required, uuid)_
- `latest_event_at`: unknown
- `market_product_code`: string _(required)_
- `pins`: WatchlistTargetResponse[]
- `target_type`: string
- `unread_event_count`: integer

### WatchlistSummaryResponse

- `created_at`: string(date-time) _(required)_
- `has_more_slices`: boolean
- `id`: string(uuid) _(required, uuid)_
- `kind`: string _(required)_
- `latest_event_at`: unknown
- `name`: string _(required)_
- `slices`: WatchlistSliceResponse[]
- `total_slice_count`: integer
- `unread_event_count`: integer

### WatchlistTargetResponse

- `active_order_count`: integer
- `availability_window_code`: unknown
- `created_at`: string(date-time) _(required)_
- `delivery_point_id`: unknown
- `delivery_point_name`: unknown
- `id`: string(uuid) _(required, uuid)_
- `latest_event_at`: unknown
- `market_product_code`: unknown
- `order_id`: unknown
- `snapshot_availability_window`: unknown
- `snapshot_counterparty_label`: unknown
- `snapshot_delivery_point_name`: unknown
- `snapshot_market_product`: unknown
- `snapshot_price_per_mt_usd`: unknown
- `snapshot_quantity_mt`: unknown
- `snapshot_remaining_quantity_mt`: unknown
- `snapshot_side`: unknown
- `snapshot_status`: unknown
- `target_type`: string _(required)_
- `unread_event_count`: integer

### app__routers__admin_analytics__OverviewResponse

- `active_users_7d`: integer _(required)_
- `confirmed_trades`: integer _(required)_
- `open_orders`: integer _(required)_
- `total_gmv_usd`: number _(required)_
- `total_orders`: integer _(required)_
- `total_organizations`: integer _(required)_
- `total_revenue_usd`: number _(required)_
- `total_trades`: integer _(required)_
- `total_users`: integer _(required)_
- `total_volume_mt`: number _(required)_

### app__schemas__product_analytics__OverviewResponse

- `activity_trend`: ActivityTrend _(required)_
- `kpis`: OverviewKpis _(required)_
- `lifecycle`: LifecycleStage[]
- `marketplace_balance`: MarketplaceBalance _(required)_
- `meta`: AnalyticsMeta _(required)_
- `needs_attention`: NeedsAttentionItem[]

## Schema Source Files

Read and edit these files when adding columns, creating migrations, or changing relations:

- `app/models/user.py` — imported by **170** files
- `app/models/orderbook.py` — imported by **112** files
- `app/models/catalog.py` — imported by **68** files
- `app/schemas/market_activity.py` — imported by **33** files
- `app/model_base.py` — imported by **32** files
- `app/schemas/orderbook.py` — imported by **28** files

---
_Back to [overview.md](./overview.md)_