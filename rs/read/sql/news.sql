-- R05 GET /api/news (Task 8).
--
-- Python filters on a minimum relevance, applies the category filter only when
-- the parameter is truthy, orders by published time descending, and limits the
-- result. The empty-string branch below is that falsy check: FastAPI passes an
-- empty category through unchanged.
--
-- The route has no response model, so the ten keys are the wire contract.
-- Dates are formatted with Python's `datetime.isoformat()` semantics in Rust,
-- not by a Pydantic formatter.
-- Binding: $1 minimum relevance (integer), $2 category (text, null = any),
--          $3 row limit (bigint).
SELECT
    id::text AS id,
    title,
    summary,
    source,
    source_url,
    url,
    category,
    relevance,
    published_at,
    fetched_at
FROM news_items
WHERE relevance >= $1
    AND ($2::text IS NULL OR $2 = '' OR category = $2)
ORDER BY published_at DESC
LIMIT $3
