# Database

> **Navigation aid.** Schema shapes and field types extracted via AST. Read the actual schema source files before writing migrations or query logic.

**sqlalchemy** — 30 models

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
- `changes`: JSONB _(nullable)_
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

pk: `id` (UUID) · fk: direct_order_id

- `id`: UUID _(pk, default)_
- `direct_order_id`: unknown _(fk)_
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

### InventoryItem

pk: `id` (UUID) · fk: supplier_id, port_id

- `id`: UUID _(pk, default)_
- `supplier_id`: unknown _(fk)_
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

pk: `id` (UUID) · fk: bid_order_id, ask_order_id, initiator_org_id, counterparty_org_id, product_id, last_actor_org_id, trade_id

- `id`: UUID _(pk, default)_
- `bid_order_id`: UUID _(fk, nullable)_
- `ask_order_id`: UUID _(fk, nullable)_
- `initiator_org_id`: UUID _(fk)_
- `counterparty_org_id`: UUID _(fk)_
- `initiator_side`: String
- `product_id`: UUID _(fk)_
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

pk: `id` (UUID) · fk: negotiation_id, proposer_org_id

- `id`: UUID _(pk, default)_
- `negotiation_id`: UUID _(fk)_
- `round_number`: Integer
- `proposer_org_id`: UUID _(fk)_
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

pk: `id` (UUID) · fk: organization_id, product_id, delivery_point_id, vessel_id

- `id`: UUID _(pk, default)_
- `organization_id`: unknown _(fk)_
- `side`: Enum
- `product_id`: UUID _(fk)_
- `delivery_point_id`: UUID _(fk, nullable)_
- `port_id`: String _(nullable)_
- `vessel_id`: unknown _(fk, nullable)_
- `quantity_mt`: Numeric
- `remaining_quantity_mt`: Numeric
- `price_per_mt_usd`: Numeric
- `availability_window`: String _(default)_
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
- `created_at`: DateTime _(default)_
- `updated_at`: DateTime _(default)_
- _relations_: organization: Organization, product: Product, delivery_point: DeliveryPoint, vessel: Vessel, bid_trades: Trade, ask_trades: Trade

### Trade

pk: `id` (UUID) · fk: bid_order_id, ask_order_id, buyer_id, seller_id

- `id`: UUID _(pk, default)_
- `bid_order_id`: unknown _(fk, nullable)_
- `ask_order_id`: unknown _(fk, nullable)_
- `buyer_id`: unknown _(fk)_
- `seller_id`: unknown _(fk)_
- `initiated_by`: Enum
- `is_anonymous`: Boolean _(default)_
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
- _relations_: bid_order: OrderBookOrder, ask_order: OrderBookOrder, buyer: Organization, seller: Organization, commission: Commission

### Commission

pk: `id` (UUID) · fk: match_id, trade_id

- `id`: UUID _(pk, default)_
- `match_id`: unknown _(fk, unique)_
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

### RFQ

pk: `id` (UUID) · fk: buyer_org_id, product_id, delivery_point_id

- `id`: UUID _(pk, default)_
- `buyer_org_id`: UUID _(fk)_
- `product_id`: UUID _(fk)_
- `delivery_point_id`: UUID _(fk, nullable)_
- `quantity_mt`: Numeric
- `target_price_per_mt`: Numeric _(nullable)_
- `availability_window`: String _(default)_
- `notes`: Text _(nullable)_
- `is_anonymous`: Boolean _(default)_
- `status`: Enum _(default)_
- `expires_at`: DateTime
- `created_at`: DateTime _(default)_
- _relations_: quotes: 

### RFQQuote

pk: `id` (UUID) · fk: rfq_id, seller_org_id

- `id`: UUID _(pk, default)_
- `rfq_id`: UUID _(fk)_
- `seller_org_id`: UUID _(fk)_
- `price_per_mt_usd`: Numeric
- `notes`: Text _(nullable)_
- `status`: Enum _(default)_
- `created_at`: DateTime _(default)_
- _relations_: rfq: 

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
- `verification_status`: String _(default)_
- `created_at`: DateTime _(default)_
- _relations_: users: , vessels: , orderbook_orders: 

### User

pk: `id` (UUID) · fk: organization_id, referred_by_id

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
- `created_at`: DateTime _(default)_
- `email_verified`: Boolean _(default)_
- `email_verification_token`: String _(nullable)_
- `kyc_status`: String _(default)_
- `kyc_rejection_reason`: Text _(nullable)_
- `password_reset_token_hash`: String _(nullable)_
- `password_reset_expires`: DateTime _(nullable)_
- `referral_code`: String _(unique, nullable)_
- `referred_by_id`: UUID _(fk, nullable)_
- _relations_: organization: , referrals_made: , referral_received: 

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