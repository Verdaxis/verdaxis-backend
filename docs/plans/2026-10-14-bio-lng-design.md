# Bio-LNG Market Slice: Design and Contract

Date: 2026-10-14
Status: approved by the founder (build directly on staging, then production; "go with all your recommendations", 2026-10-14).
Scope: backend `staging` and `prod`, frontend `staging` and `prod`. This document is the shared contract. Every builder uses these names exactly.

## 1. Decisions

| Topic | Decision |
|---|---|
| What is traded | Physical bio-LNG (liquefied biomethane) bunkers |
| Unit and price | Tonnes (MT), USD per MT, as for every other product. No MWh or m³ conversion. |
| Delivery points | Rotterdam only, at first. More points are a later catalog change. |
| Certification | Mass-balance or physical chain of custody under ISCC EU or REDcert-EU, with a Proof of Sustainability. FuelEU credit needs the PoS in the EU Union Database. |
| GHG value | The supplier declares the certified **well-to-tank** value (gCO2e/MJ) from its PoS. It excludes engine methane slip, which depends on the buyer's engine. The value may be negative (manure). It must **not** feed the existing lifecycle carbon-intensity pricing (`app/services/ci_pricing.py`). |
| Resellers | A seller that is not the physical supplier must name the physical supplier and its own scheme certificate. Buyers can refuse resellers. |
| Trader admission | A `FUEL_TRADER` organization may hold the `SUPPLIER` role (ask side). This is admin invitation only; public signup is unchanged. Full trader capabilities come later. |
| Demo data | None. Do not add BIO_LNG to `PRICING`, demo activity, forward seeds or benchmark fallbacks. |

## 2. Product identity (identical on both branches)

| Field | Value |
|---|---|
| `MarketProduct` code | `BIO_LNG` |
| UUID | `8f6f34a5-2927-54a6-8564-cca40ae7cb8a` (UUIDv5, namespace `a1b2c3d4-e5f6-7890-abcd-ef1234567890`, name `product:Bio-LNG`) |
| name / fuel_type / fuel_grade | `Bio-LNG` / `LNG` / `Bio` |
| unit | `MT` |
| min_lot_size | `1` (platform input minimum; stems are usually larger) |
| spec_description | "Liquefied biomethane (bio-LNG) for marine fuel use, delivered to ISO 23306:2020 with a certified Proof of Sustainability." |
| execution_mode | `ORDERBOOK` |
| delivery points | Rotterdam `1379d36c-1ca9-55b7-9c0d-5235a0ba1f36` only |
| public fuel-type allowlist | Add `LNG` to `APPROVED_MARKETPLACE_FUEL_TYPES` |

Labels: English "Bio-LNG". Chinese: "生物液化天然气". Frontend color: sky (the same family as other gas fuels).

## 3. `lng_terms` contract

Store it as a nullable JSON column `lng_terms` on orders (and wherever `fame_terms` is stored on staging). Freeze it on trades as an immutable snapshot. It is required on BIO_LNG orders and rejected on any other product.

Shared types:

```text
SustainabilityScheme = "ISCC_EU" | "REDCERT_EU"            (reuse app/schemas/fame.py on staging)
ChainOfCustody       = "MASS_BALANCE" | "PHYSICAL_SEGREGATION"
DeliveryMethod       = "SHIP_TO_SHIP" | "TRUCK_TO_SHIP" | "TERMINAL_TO_SHIP"
FeedstockCategory    = "MANURE" | "FOOD_WASTE" | "SEWAGE_SLUDGE" | "LANDFILL_GAS"
                     | "AGRICULTURAL_RESIDUES" | "OTHER_WASTES_AND_RESIDUES" | "ENERGY_CROPS"
PosTiming            = "WITH_DELIVERY" | "AFTER_DELIVERY"
SPEC_STANDARD        = "ISO 23306:2020"                     (the only accepted value)
```

### ASK (seller declarations): `LngAskTerms`

| Field | Type | Rule |
|---|---|---|
| `side` | `"ASK"` | discriminator |
| `sustainability_scheme` | SustainabilityScheme | required |
| `scheme_certificate_number` | string, 3 to 64 chars | required; the **seller's own** certificate |
| `chain_of_custody` | ChainOfCustody | required |
| `union_database_registered` | bool | required |
| `bio_share_pct` | Decimal, 1 to 100, 2 decimal places | required |
| `feedstock` | FeedstockCategory | required |
| `feedstock_detail` | string up to 200 | optional |
| `production_country` | ISO 3166-1 alpha-2, uppercase | required |
| `wtt_ghg_gco2e_mj` | Decimal, −150 to 100, 2 decimal places | required; PoS well-to-tank value, excluding methane slip |
| `specification_standard` | `"ISO 23306:2020"` | required |
| `min_methane_number` | Decimal, 0 to 100 | optional |
| `delivery_method` | DeliveryMethod | required |
| `seller_is_physical_supplier` | bool | required |
| `physical_supplier_name` | string, 2 to 120 chars | required; if `seller_is_physical_supplier` is false it names the licensed physical supplier or bunkering partner |
| `pos_timing` | PosTiming | required |

Validator: if `chain_of_custody` is `MASS_BALANCE` or `PHYSICAL_SEGREGATION`, a scheme is required (it always is). If `union_database_registered` is false, the API accepts the ask, but the public terms carry `fueleu_recognition: "NOT_REGISTERED"` (see below).

### Public ASK view: `LngPublicAskTerms`

The public orderbook must not reveal identity before confirmation. It is `LngAskTerms` **without** `scheme_certificate_number` and `physical_supplier_name`, plus derived fields:

- `reseller: bool` = `not seller_is_physical_supplier`
- `fueleu_recognition: "UNION_DATABASE" | "NOT_REGISTERED"`

The full ask terms appear in the trade snapshot only after the trade is CONFIRMED, as counterparties are revealed today.

### BID (buyer requirements): `LngBidTerms`

| Field | Type | Default and rule |
|---|---|---|
| `side` | `"BID"` | discriminator |
| `accepted_schemes` | non-empty list of SustainabilityScheme, unique | required |
| `accepted_custody` | non-empty list of ChainOfCustody, unique | both values |
| `require_union_database` | bool | `true` |
| `min_bio_share_pct` | Decimal 1 to 100 | optional |
| `max_wtt_ghg_gco2e_mj` | Decimal −150 to 100 | optional |
| `accepted_delivery_methods` | non-empty list of DeliveryMethod, unique | required |
| `min_methane_number` | Decimal 0 to 100 | optional |
| `accept_resellers` | bool | `true` ("physical suppliers only" sets false) |

### Matching compatibility

An ask is compatible with a bid only if every one of these holds. Incompatible orders must never match or be taken, even if product, port and window match:

1. `ask.sustainability_scheme in bid.accepted_schemes`
2. `ask.chain_of_custody in bid.accepted_custody`
3. `ask.union_database_registered or not bid.require_union_database`
4. `bid.min_bio_share_pct is None or ask.bio_share_pct >= bid.min_bio_share_pct`
5. `bid.max_wtt_ghg_gco2e_mj is None or ask.wtt_ghg_gco2e_mj <= bid.max_wtt_ghg_gco2e_mj`
6. `ask.delivery_method in bid.accepted_delivery_methods`
7. `bid.min_methane_number is None or (ask.min_methane_number is not None and ask.min_methane_number >= bid.min_methane_number)`
8. `bid.accept_resellers or ask.seller_is_physical_supplier`

Put this in one pure function, `lng_terms_compatible(bid, ask) -> bool`, used by matching and by direct taking, so the two cannot drift. When a direct take is rejected, return a stable error code `LNG_TERMS_INCOMPATIBLE`, with no counterparty detail.

### Trade snapshot

`LngTradeSnapshot { bid: LngBidTerms, ask: LngAskTerms }` is frozen at trade creation and immutable, following the existing FAME snapshot trigger pattern. Before confirmation the public snapshot uses `LngPublicAskTerms`. After confirmation it shows the full ask.

## 4. Generic ASK predicates for BIO_LNG

The existing ASK requirements (certification declared, certification scheme, specification standard, MSDS, carbon intensity, feedstock, origin) must hold for BIO_LNG through `lng_terms`:

- `certification_declared = true`. `certification_scheme` is the scheme label (`ISCC EU` or `REDcert-EU`).
- `specification_standard = "ISO 23306:2020"`. `msds_available` stays required.
- `feedstock` is the feedstock category label, and `origin` is `production_country`.
- **Carbon intensity:** BIO_LNG is exempt from the scalar `carbon_intensity_gco2_mj` requirement, as UCOME_B100 is. Leave the scalar NULL. The WTT value lives only in `lng_terms`. `ci_pricing` and `/orderbook/with-ci` must skip BIO_LNG and not treat a NULL as zero.

Derive these scalars on the server from `lng_terms`. Do not trust the client to set them separately.

## 5. Migrations

| Branch | Current head | New revision | Checkpoint rows to add |
|---|---|---|---|
| staging | `rcp_20261003_command_results` | `blng_20261014_staging` | `rcp_20261003_command_results → blng_20261014_staging`, `blng_20261014_staging → blng_20261014_staging` |
| prod | `uadl_20261003_delivery_reports` | `blng_20261014_prod` | `uadl_20261003_delivery_reports → blng_20261014_prod`, `blng_20261014_prod → blng_20261014_prod` |

Each migration:
- freezes the product identity locally (no import from `app/market_catalog.py`) and inserts the product with conflict checking
- adds `lng_terms` columns and constraints wherever the branch stores product terms
- extends the trade product checks, the canonical trade-snapshot trigger (identity and immutability) and the product checks on `market_indications`, `fair_price_bands` and `physical_stems`
- leaves the ACL policy unchanged unless a new table is added
- has a tested downgrade

The ORM constraints must match (`app/market_constraints.py` on staging).

## 6. Prod differences

Prod has neither `available_delivery_point_ids` nor a FAME terms layer.

- **Delivery points:** prod adds lane enforcement for BIO_LNG (Rotterdam only) in the catalog, the API and execution, including public reads. It is not enough to restrict the UI.
- **Terms layer:** prod adds the `lng_terms` layer from this contract, without porting FAME.
- **Existing products:** B30 and B100 stay unchanged.

## 7. Frontend (both branches)

- Add the product code and labels (`src/utils/marketProduct.ts`), the market product lists (`src/types.ts`), the marketplace filter options (`src/utils/marketProducts.ts`), the TradeTape and fuel colors, and the forward curve taxonomy (catalog-driven).
- **Order modal:** a BIO_LNG branch. Sellers fill `LngAskTerms`; buyers fill `LngBidTerms`, including a "Physical suppliers only" switch. Client validation mirrors the server, which stays authoritative.
- **Display:** public ask terms in the order book and marketplace (scheme, custody, UDB status, bio share, feedstock, WTT value with the "excludes engine methane slip" note, delivery method, reseller flag). The full terms appear on confirmed trades.
- **Error:** handle `LNG_TERMS_INCOMPATIBLE`.
- **i18n:** en and zh.
- **Tests:** update the pinned product lists (`src/tests/features.test.ts`, `scripts/smoke-live.mjs`) and add terms tests.
- **Prod:** add a spec helper the way `src/utils/biofuelSpecification.ts` works, not by porting the staging FAME layer.

## 8. Out of scope

Voyage planner support for BIO_LNG (it needs engine-specific methane slip), any MWh, m³ or EUR pricing, more delivery points, demo liquidity, KYC changes (a separate workstream), and full trader capabilities.

## 9. Acceptance

- On staging, Diesol's organization (FUEL_TRADER, SUPPLIER role, via admin invitation) can post a BIO_LNG ask at Rotterdam with full `lng_terms`. It appears publicly without the certificate number or physical supplier name.
- A buyer bid with compatible requirements matches it. An incompatible bid never matches, and a direct take returns `LNG_TERMS_INCOMPATIBLE`.
- No BIO_LNG order is possible at any other delivery point.
- `/orderbook/with-ci` and the voyage MCP tools are unaffected by BIO_LNG.
- CI passes (`test`, `postgres-analytics`). The frontend CI and smoke tests pass with the new product list.
