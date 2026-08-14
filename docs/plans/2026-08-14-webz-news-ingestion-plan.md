# Webz.io News Ingestion Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make licensed Webz.io results the primary Verdaxis news source with safe RSS fallback and no market-data side effects.

**Architecture:** Extend the existing singleton news refresh service with a bounded Webz.io fetcher and a small provider selector. Normalize external records into the current `NewsItem` input shape so persistence, categorization, deduplication, routing, and scheduling remain unchanged.

**Tech Stack:** Python 3.10+, FastAPI settings via Pydantic, httpx, SQLAlchemy async, pytest.

---

### Task 1: Define Provider Configuration And Contract Tests

**Files:**
- Modify: `app/config.py`
- Modify: `.env.example`
- Modify: `tests/unit/test_news_feed.py`

1. Add failing tests for provider selection, a valid normalized response,
   malformed-record rejection, sanitized failure fallback, and summary
   preservation.
2. Run `pytest tests/unit/test_news_feed.py -q` and confirm the new tests fail.
3. Add optional `WEBZ_API_TOKEN` and bounded `WEBZ_API_URL` settings plus the
   documented environment placeholders.

### Task 2: Implement The Bounded Webz.io Fetcher

**Files:**
- Modify: `app/services/news_feed.py`
- Test: `tests/unit/test_news_feed.py`

1. Add the source-controlled query, request bounds, response parser, public URL
   validator, and timestamp parser.
2. Add `fetch_news_items()` to call Webz.io when configured and fall back to
   `fetch_all_feeds()` on provider failure or zero usable records.
3. Update `refresh_news()` to use the selector and retain a validated provider
   summary unless classification replaces it.
4. Run the focused tests until green.

### Task 3: Synchronize Documentation

**Files:**
- Modify: `docs/news-refresh-timer.md`
- Modify: `ARCHITECTURE.md`
- Modify: `CLAUDE.md`
- Modify: `deploy/systemd/verdaxis-news-refresh.timer`
- Modify: `deploy/systemd/verdaxis-news-refresh-staging.timer`
- Modify: `tests/unit/test_news_refresh_cli.py`

1. Change both source-controlled timers from every 15 minutes to one run every
   six hours and update the timer ownership test.
2. Document Webz.io as the primary optional source, RSS as fallback, secret
   handling, quota budget, and the strict separation from trusted market-signal
   ingestion.
3. Confirm source ownership and runtime timer documentation remain accurate.

### Task 4: Verify And Review

1. Run the focused news suites.
2. Run the complete unit suite with the repository's documented test
   environment.
3. Review the diff for token leakage, URL-validation bypasses, accidental
   schema/API changes, duplicated code, and unnecessary abstractions.
4. Commit the complete doc, test, and implementation change together.

### Task 5: Stage And Promote

1. Supply the licensed token transiently for one bounded staging integration
   check; do not retain it in staging or print it.
2. deploy the reviewed staging commit with `scripts/deploy.sh`.
3. Run the staging one-shot news service and verify inserted/provider counts,
   public API records, timer health, and absence of secrets.
4. Promote the exact feature commits to `prod`, configure the protected
   production environment, deploy, run one refresh, and verify production. The
   steady-state Webz budget is at most 124 scheduled production requests per
   month.
5. Push both environment branches and remove the temporary worktree after all
   live checks pass.
