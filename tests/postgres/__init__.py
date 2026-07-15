"""PostgreSQL 15/PostGIS 3.3 correctness suite for Product Analytics.

SQLite is the fast unit harness; this suite is the correctness authority.
Requires PRODUCT_ANALYTICS_TEST_DATABASE_URL (see
scripts/run_product_analytics_postgres_tests.sh); tests skip when unset.
"""
