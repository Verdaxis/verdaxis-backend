# Webz.io News Ingestion Design

**Status:** Approved 2026-08-14

## Objective

Use the licensed Webz.io News API as Verdaxis's primary open-web discovery
source while preserving the existing RSS collector as a resilience fallback.
The integration enriches the customer news feed only. It must never create or
modify benchmarks, market indications, fair-price bands, physical stems,
orders, or trades.

## Architecture

The existing environment-specific systemd timer remains the sole refresh
owner, with its calendar reduced from every 15 minutes to every six hours.
Production calls Webz.io on that bounded cadence; staging does not retain the
shared token after integration dogfood. `refresh_news()` calls one provider
selector:

1. When `WEBZ_API_TOKEN` is configured, request one bounded, deduplicated
   Webz.io result page covering Verdaxis's four canonical fuels, maritime
   bunkering, and applicable regulation.
2. Validate every provider record into the existing internal news-item shape.
3. If Webz.io is unconfigured, fails, or yields no usable records, run the
   existing five-feed RSS collector.
4. Apply the existing batch deduplication, database uniqueness, categorization,
   and insertion path without changing the public news API or database schema.

This deliberately avoids a provider abstraction hierarchy: there are only two
small fetch functions and one selector.

## Provider Contract

- Endpoint: configurable `WEBZ_API_URL`, defaulting to the documented premium
  `https://api.webz.io/filterWebContent` endpoint.
- Authentication: backend-only `WEBZ_API_TOKEN`, sent only to Webz.io.
- Query: a source-controlled Boolean query for canonical fuels, marine-fuel
  trading, bunkering, and maritime decarbonisation regulation.
- Bounds: one request per scheduled refresh, at most 100 records, an 8 MiB
  streamed response cap, 15-second provider timeout, bounded title/URL/summary
  lengths, and no response-body logging.
- Deduplication: request `includeSyndicated=false`; retain URL-based in-process
  and database deduplication.
- Ordering: request newest published records first.

## Validation And Security

Webz.io is an external untrusted-data boundary even though it is a licensed
provider. Accept only records with:

- a bounded non-control-character title;
- an absolute public HTTP(S) article URL without credentials;
- a public HTTP(S) publisher/source URL;
- a bounded source label;
- a timezone-aware provider timestamp, with a safe current-time fallback;
- an optional bounded plain-text summary.

The token must not be committed, emitted in logs, returned in errors, stored in
the database, or included in public source URLs. Provider errors are logged as
sanitized reason codes only.

## Failure Behaviour

- Missing token: use RSS and log the provider as unconfigured at informational
  level once per run.
- Timeout, HTTP error, invalid JSON, or malformed response: log a sanitized
  provider failure and use RSS.
- Valid response with no usable records: use RSS.
- Partial malformed response: retain valid records; do not invoke RSS merely
  because some records were rejected.

RSS fallback prevents a Webz.io outage or quota issue from emptying the feed.
The database advisory lock and singleton timer continue preventing overlapping
refreshes.

## Quota Budget

The shared account leaves approximately 500 monthly requests available to
Verdaxis and other services. The source-controlled production timer runs once
every six hours: 4 per day, normally 120 to 124 Webz requests per month. Manual
operator verification should keep the practical ceiling below 130. Staging
uses the token only for a bounded manual integration check and otherwise stays
on RSS, preserving at least 370 requests per month for other services. A manual
service start is an operator-visible extra request; there is no hidden retry or
pagination loop.

## Classification

Webz.io summaries may be persisted after validation. Existing Gemini/local
classification remains responsible for Verdaxis's six UI categories and
relevance score, because the provider's IPTC taxonomy does not match the
product taxonomy. When a validated Webz.io summary exists, it is retained
unless the classifier supplies a more specific summary.

## Observability

Structured logs identify the selected provider and report fetched, rejected,
and inserted counts. They never contain the token, request URL, raw provider
body, or provider exception text.

## Explicit Non-Goals

- Scraping or deriving Platts, Argus, QCI, or other benchmark prices.
- Automatically converting article text into trusted market signals.
- Adding a public provider selector or exposing raw Webz.io fields.
- Adding another scheduler, database table, background queue, or frontend API.
