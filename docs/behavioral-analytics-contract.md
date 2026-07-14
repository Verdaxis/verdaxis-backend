# Behavioral Analytics Contract

## Purpose

The Verdaxis backend exposes aggregated behavioral analytics to administrators
and keeps authoritative conversion outcomes grounded in Verdaxis data. Umami is
an optional external dependency; its failure must never affect authentication,
orders, trades, or existing admin analytics.

## Configuration

- `ANALYTICS_ENABLED`
- `UMAMI_BASE_URL`
- `UMAMI_WEBSITE_ID`
- `UMAMI_API_USERNAME`
- `UMAMI_API_PASSWORD`
- `ANALYTICS_REQUEST_TIMEOUT_SECONDS`

All credentials are server-only secrets. Empty or disabled configuration yields
an explicit unavailable behavioral section rather than application startup
failure.

`ANALYTICS_REQUEST_TIMEOUT_SECONDS` must be greater than zero and no more than
10 seconds; the recommended default is 2 seconds. `UMAMI_BASE_URL` must be an
absolute HTTP(S) URL without embedded credentials. The API username and password
are required only for administrative aggregate reads. Collector event delivery
uses Umami's unauthenticated `/api/send` endpoint and still requires analytics to
be enabled with a base URL and website ID.

## Authoritative Conversion Events

After the related database transaction succeeds, the backend sends bounded,
best-effort events for `registration_completed`, `organization_created`,
`order_created`, and `trade_created`. These events may include role, side,
canonical product, delivery point, and availability window. They may not include
internal user IDs, email, names, organization names, order/trade IDs,
counterparties, price, quantity, or total value. Delivery is timeout-bounded and
may never fail the user request.

Events are scheduled in-process immediately after the owning transaction commits.
This deliberately favors request isolation over guaranteed delivery: a process
termination can drop an event, and analytics is not a financial system of record.
Verdaxis remains authoritative for registrations, logins, organizations, orders,
and trades.

Requests authenticated with the configured internal monitor token are excluded
from event delivery so signup canaries test the real flow without inflating
acquisition metrics.

The server producer allowlist is limited to `registration_completed`,
`organization_created`, `order_created`, and `trade_created`. It is separate from
the browser reporting taxonomy below. Server events copy the originating request's
User-Agent when available, bounded to 256 printable ASCII characters, and use the
request hostname or the configured frontend hostname. User-Agent and hostname are
collector metadata, not event properties, and are never logged. Umami's
`{"beep":"boop"}` bot-drop marker is treated as failed delivery.

## Browser Reporting Taxonomy

The Admin aggregate accepts only the documented frontend events:

- Landing: `landing_cta_clicked`, `energy_calculator_started`,
  `energy_calculator_completed`, `public_language_changed`.
- Signup/auth: `signup_started`, `signup_role_selected`, `signup_submitted`,
  `signup_organization_required`, `signup_organization_submitted`,
  `login_submitted`, `login_succeeded`, `login_failed`.
- Platform: `platform_navigation`, `market_slice_selected`, `listing_opened`,
  `order_form_opened`, `order_form_submitted`, `trade_confirmation_opened`,
  `tutorial_started`, `tutorial_step_completed`, `tutorial_step_skipped`,
  `tutorial_completed`, `estimator_opened`, `estimator_completed`.

The bounded Admin Feature Usage subset is the platform list above. Unknown Umami
event names are discarded from both totals and series rather than reflected into
the Admin API.

## Admin Aggregate

`GET /api/admin/analytics/product-usage?days=7|30|90`

The route requires the `ADMIN` role and returns:

- Behavioral availability and observation timestamp.
- Visitors, visits, pageviews, total time, and calculated average session
  duration (`totaltime / visits`). This is passive session duration, not
  focus-aware active or engaged time.
- Allowlisted event totals, daily event series, and daily visitor series from Umami.
- Top entry paths and referrers from Umami.
- Registrations, active logins, and non-demo order-creating organizations from
  the Verdaxis database for the same UTC period.
- Funnel stages and conversion rates calculated from the above values.

The database definitions are:

- `registrations`: BUYER/SUPPLIER users created inside the UTC period.
- `users_logging_in`: BUYER/SUPPLIER users whose latest successful login is
  inside the UTC period.
- `order_placing_organizations`: distinct organizations with at least one market
  user and a created order inside the UTC period, excluding canonical demo market
  organization IDs.

The endpoint must not expose raw sessions, distinct IDs, emails, names,
organization names, transaction values, counterparties, prices, or quantities.

## Failure Contract

Umami authentication, timeout, malformed response, or upstream failure returns
HTTP 200 with `behavioral_status=unavailable`, a short diagnostic category, and
the authoritative Verdaxis counts. Secrets and upstream response bodies are not
returned or logged.

The only diagnostic categories are `disabled`, `configuration`,
`authentication`, `timeout`, `upstream`, and `malformed_response`. No upstream
body, URL credentials, username, password, bearer token, or exception text is
included in responses or logs.

## Product Analytics Property Aggregates

Verified against the installed Umami 3.2.0 container on 2026-07-15 by
`scripts/smoke_umami_product_analytics.py` (read-only, view-only credentials,
loopback-only unless `--allow-remote-readonly`). The Product Analytics
workspace may call exactly these authenticated HTTP routes — never the Umami
database:

- `GET /api/websites/{websiteId}/event-data/properties?startAt&endAt` —
  event×property inventory. Rows:
  `{eventName: str, propertyName: str, dataType: int, total: int}`.
- `GET /api/websites/{websiteId}/event-data/events?startAt&endAt&event=<name>`
  — per-value breakdown for one registered event. Rows:
  `{eventName: str, propertyName: str, dataType: int, propertyValue: str,
  total: int}`.
- `GET /api/websites/{websiteId}/event-data/values?startAt&endAt&event=<name>&propertyName=<prop>`
  — value distribution for one event property. Rows:
  `{value: str, total: int}`.

Unsupported or excluded forms (do not call):

- `event-data/events` **without** the `event` filter returns HTTP 500 on this
  build.
- `event-data/values` silently ignores an `eventName` parameter and returns an
  empty result; the filter parameter is `event`.
- `event-data-pivot` is supported (paginated
  `{count, data, isCapped, page, pageSize}` envelope, requires `eventName`)
  but is excluded from the implementation contract; the smoke probes it only
  to detect upstream drift.

Event names passed to these routes must come from the registered taxonomies
above; property names must come from the analytics event registry. Responses
are bounded (1MB body, 5,000 rows) and any shape drift is a contract failure,
not data.

## Caching

Successful behavioral aggregates may be cached in-process for at most five
minutes per period. Failures may be cached for at most 30 seconds to avoid
hammering an unhealthy collector while allowing quick recovery.

The cache is process-local and keyed by the requested 7, 30, or 90-day period.
It is an optimization only; restarting a backend process clears it.
