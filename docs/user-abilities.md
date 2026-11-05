# User abilities contract (Rust owner)

`GET /api/auth/me/abilities` returns user-level UI hints. Rust owns this path;
Python has no route here. The frontend parser accepts schema version 1.
Every action remains subject to the enforcing endpoint's ownership, state,
counterparty, input, context and replay checks. `allowed: true` is not authority.

## Transport and authentication

Bearer authentication and current-account validation use the same Rust
`run_authenticated` transaction as `/api/auth/me`. The user-row lock is currently
`FOR UPDATE`, as it is for Rust `/auth/me`; this change does not alter that route.
Missing/invalid/stale tokens return 401; pending/rejected accounts and required
password changes return 403. `/auth/me` itself permits password-change-required
profile reads; abilities deliberately uses the normal restricted path.
Successful responses carry `Cache-Control: no-store`. Refetch after account,
organization, KYC, capability or feature-switch changes. No database writes,
new migrations, external requests or background jobs are added.

```json
{"schema_version":1,"abilities":[
  {"action":"place_bid_order","allowed":true,"reason_code":null,"reason":null},
  {"action":"create_rfq","allowed":false,"reason_code":"ACTION_RETIRED","reason":"This feature has been retired."}
]}
```

Every action below is present in this fixed order. Allowed rows have null reason
fields. Denied rows have a stable uppercase code and short English display text.
Never match the text. Unknown actions may be ignored; unknown codes are generic
denials. IDs/codes are not renamed or removed.

## Fixed actions and first-denial order

Execution account admission checks trading role, organization membership/record,
account approval, verified email, password requirement and organization approval.
Execution KYC uses `KYC_EXECUTION_GATE_ENABLED`: strict mode allows current-org
APPROVED KYC or active current-org migration grace, with rejection overriding
both; advisory mode refuses rejected or differently bound reviews. Inventory
management always uses advisory KYC. Grace is not returned here; `/auth/me`
provides execution eligibility and its grace deadline.

| Action | Flat gate, in order |
| --- | --- |
| place_bid_order | KYC, execution account admission, BUYER |
| place_ask_order | KYC, execution account admission, SUPPLIER |
| amend_order | legacy administrator path, execution admission/KYC |
| cancel_order | legacy administrator path, ordinary ADMIN refusal; recorded ownership remains per order |
| hit_ask_order | execution admission/KYC, BUYER, REAL provenance |
| hit_bid_order | execution admission/KYC, SUPPLIER, REAL provenance |
| confirm_trade | ordinary ADMIN refusal, KYC, execution account admission |
| deliver_trade | no additional flat gate; recorded settlement authority is per trade |
| pay_trade | SUPPLIER; recorded settlement authority is per trade |
| manage_inventory | indicative account/advisory KYC, SUPPLIER |
| publish_inventory | KYC, execution account admission, SUPPLIER |
| create_rfq | always ACTION_RETIRED |
| quote_rfq | always ACTION_RETIRED |
| view_admin_analytics | ADMIN |
| view_audit_log | ADMIN |
| view_user_activity | ADMIN |
| view_feedback | ADMIN |
| manage_subscriptions | ADMIN |
| manage_accounts | ADMIN |
| review_kyc | ADMIN |
| manage_commissions | ADMIN |
| market_support_access | authenticated approved account, switch enabled, ADMIN, either active staff capability |
| market_support_assisted_orders | same staff gate, both MARKET_SUPPORT_LISTINGS and MARKET_SUPPORT_AUTHORIZATIONS |

Staff assignments must be unrevoked and unexpired. Staff access does not require
a trading organization or staff KYC. Customer/context execution is a separate
per-subject check. An assisted context does not change this signed-in-user payload.
The legacy administrator amend/cancel path is retired when Market Support is on.

## Reason codes

- ADMIN_CANNOT_EXECUTE: ordinary administrator trading is refused.
- ROLE_NOT_ASSIGNED: no trading role.
- ORGANIZATION_REQUIRED: missing membership or organization record.
- ACCOUNT_NOT_APPROVED: account is not approved.
- EMAIL_NOT_VERIFIED: email verification is required.
- PASSWORD_CHANGE_REQUIRED: password replacement is required.
- KYC_REQUIRED: no execution-eligible review/grace, or execution binding mismatch.
- KYC_REJECTED: rejected KYC.
- KYC_ORGANIZATION_MISMATCH: advisory review belongs to another organization.
- ORGANIZATION_NOT_APPROVED: organization is not approved.
- BUYER_ROLE_REQUIRED / SUPPLIER_ROLE_REQUIRED / ADMIN_ROLE_REQUIRED: wrong role.
- ORGANIZATION_PROVENANCE_UNVERIFIED: direct take requires REAL provenance.
- LEGACY_ADMIN_MUTATION_RETIRED: use an active Market Support context.
- MARKET_SUPPORT_DISABLED: staff workspace switch is off.
- MARKET_SUPPORT_CAPABILITY_REQUIRED: missing active required assignment.
- ACTION_RETIRED: **This feature has been retired.** Phase 7 RFQ actions always deny.

## Shared Rust enforcement and reference differences

`auth/abilities.rs` composes `order_write/party_policy.rs`. Enforcing order,
trade, inventory and staff adapters consume the same extracted predicates.
KYC's truth table is owned by `kyc/policy.rs`; capability loading and any/both
checks remain in `order_write/support.rs`.

Differences from closed Python PR #138 are intentional:

1. Rust order creation and inventory publication check KYC first; confirmation
   checks ADMIN, then KYC, then account/organization admission. The reference
   uses its account-first party gate. Rust execution also checks organization
   approval before its final KYC check, unlike that reference gate.
2. Rust cleanup uses recorded ownership/membership and does not require fresh
   email, organization approval or KYC admission. The abilities route still
   requires a current approved authenticated account to fetch any hints.
3. Direct take permits REAL only, not paired DEMO. Counterparty checks remain per order.
4. Rust `/auth/me` uses an exclusive lock, not the Python reference's shared lock.
5. Rust profile reads permit a required password change; this endpoint does not.
6. RFQ IDs remain but always return ACTION_RETIRED because Phase 7 is retired.

Fuel terms, lane compatibility, quotas, commissions, settlement freeze,
subscription state and per-order restrictions are not flat abilities.

## Routing and checks

Import `deploy/caddy/rust-abilities.caddy` inside the site's route block before
Python fallback: `import verdaxis_rust_abilities <rust-upstream> <python-upstream>`.
One exact matcher moves GET, HEAD and OPTIONS on `^/api/auth/me/abilities$`.
Equal arguments disable it. No Rust-to-Python failure fallback exists. Slash,
case variants and adjacent paths do not move. No host configuration is changed
by this source-only work.

`tests/rust_contract/abilities/routes.json` is this route's inventory; historical
phase inventories retain their frozen scope. `test_abilities_routing.py` checks
real local Caddy ownership. `abilities_http.rs`, `abilities_policy.rs` and
`abilities_postgres.rs` check Rust behavior. Run the migrated database case with
`bash rs/read/tests/run_abilities.sh` and the required cargo lock/environment.
