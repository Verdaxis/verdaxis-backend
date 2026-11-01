-- The news_items table as the Python schema defines it (revision news_2026_03).
-- The news job tests create it inside a throwaway schema.
CREATE TABLE news_items (
    id uuid PRIMARY KEY,
    title varchar(500) NOT NULL,
    summary text,
    source varchar(100) NOT NULL,
    source_url varchar(500) NOT NULL,
    url varchar(1000) NOT NULL UNIQUE,
    category varchar(50) NOT NULL DEFAULT 'markets',
    relevance integer NOT NULL DEFAULT 3,
    published_at timestamptz NOT NULL,
    fetched_at timestamptz NOT NULL
);
CREATE INDEX ix_news_items_published_at ON news_items (published_at);
CREATE INDEX ix_news_items_category ON news_items (category);
