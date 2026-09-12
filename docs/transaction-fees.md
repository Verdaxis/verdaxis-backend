# Transaction fees

Buyers do not pay a Verdaxis transaction fee. The seller pays a fixed USD fee
for each delivered metric tonne.

| Subscription tier | Seller fee |
| --- | ---: |
| `free` (Pilot) | USD 2.00/MT |
| `standard` (Professional) | USD 1.50/MT |
| `enterprise` | Negotiated |

`GET /api/subscriptions/fees` is the public source for this schedule. Numeric
values are JSON strings because the API uses exact decimals.

Operators can change the two standard rates for future trades with
`SELLER_FEE_PILOT_PER_MT_USD` and
`SELLER_FEE_PROFESSIONAL_PER_MT_USD`. Values must be from 0 through 100,000
with at most two decimal places. Enterprise rates are set per organization by
an administrator through `PUT /api/admin/subscriptions/{org_id}` using
`seller_fee_per_mt_usd`.

Environment rate changes take effect after an approved application restart.
A missing, inactive, or expired subscription uses Pilot. An active Enterprise
subscription must have a negotiated rate; otherwise new trade creation returns
409 instead of guessing a fee. Configure these rates before exposing the release.

Each new trade stores the seller's resolved `commission_plan` and
`commission_fee_per_mt_usd`. Delivery multiplies final quantity by that stored
rate. Later configuration or subscription changes do not reprice the trade.
Historical trades keep a null per-MT snapshot and continue to use their stored
`commission_rate_pct`.

Trade responses identify `commission_payer` as `SELLER` only for new per-MT
fee snapshots. The snapshot plan and rate are private to the seller and
administrators; buyer responses return null for both fields and the derived
commission amount.
Legacy responses keep the historical commission amount but leave the payer
null because the historical records did not identify one.
