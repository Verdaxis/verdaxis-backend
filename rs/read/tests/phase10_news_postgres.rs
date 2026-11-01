//! Phase 10 Task 11: the news refresh job against a disposable PostgreSQL.
//!
//! Each case creates its own schema holding `news_items`, so cases run side by
//! side and leave nothing behind. The job connects as a deployed application
//! role; the schema is built and read by the migrator role.
use chrono::{DateTime, TimeZone, Utc};
use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection, PgPool};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::Notify;
use uuid::Uuid;
use verdaxis_read::cli::jobs::news::*;
use verdaxis_read::cli::{Environment, Exit};

fn url(variable: &str) -> String {
    std::env::var(variable).unwrap_or_else(|_| panic!("{variable} must name a disposable database"))
}

fn database_of(url: &str) -> String {
    PgConnectOptions::from_str(url)
        .expect("parse a disposable URL")
        .get_database()
        .expect("the URL names a database")
        .to_owned()
}

fn fixed_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap()
}

/// A schema with a `news_items` table, granted to the deployed roles.
struct Fixture {
    schema: String,
    admin: PgPool,
}

impl Fixture {
    async fn new() -> Self {
        let schema = format!("news_t_{}", Uuid::new_v4().simple());
        let options = PgConnectOptions::from_str(&url("RUST_TEST_MIGRATOR_DATABASE_URL")).unwrap();
        let mut setup = PgConnection::connect_with(&options)
            .await
            .expect("connect the migrator");
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&mut setup)
            .await
            .expect("create the schema");
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "SET search_path = {schema}; {}; \
             GRANT USAGE ON SCHEMA {schema} TO verdaxis_app, verdaxis_app_staging, rust_app; \
             GRANT SELECT, INSERT ON news_items TO verdaxis_app, verdaxis_app_staging, rust_app;",
            include_str!("fixtures/news_items_schema.sql")
        )))
        .execute(&mut setup)
        .await
        .expect("create news_items");
        let admin = pool_for(&url("RUST_TEST_MIGRATOR_DATABASE_URL"), &schema).await;
        Self { schema, admin }
    }

    async fn pool(&self, variable: &str) -> PgPool {
        pool_for(&url(variable), &self.schema).await
    }

    async fn rows(
        &self,
    ) -> Vec<(
        String,
        String,
        Option<String>,
        String,
        String,
        String,
        i32,
        DateTime<Utc>,
        DateTime<Utc>,
    )> {
        sqlx::query_as(
            "SELECT url, title, summary, source, source_url, category, relevance, published_at, fetched_at \
             FROM news_items ORDER BY url",
        )
        .fetch_all(&self.admin)
        .await
        .expect("read news_items")
    }

    async fn seed(&self, url: &str, title: &str) {
        sqlx::query(
            "INSERT INTO news_items (id, title, source, source_url, url, category, published_at, fetched_at) \
             VALUES ($1, $2, 'Seed', 'https://seed.example.org', $3, 'carbon', $4, $4)",
        )
        .bind(Uuid::new_v4())
        .bind(title)
        .bind(url)
        .bind(fixed_now())
        .execute(&self.admin)
        .await
        .expect("seed a row");
    }

    async fn finish(self) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&self.admin)
        .await
        .expect("drop the schema");
    }
}

async fn pool_for(url: &str, schema: &str) -> PgPool {
    let options = PgConnectOptions::from_str(url)
        .unwrap()
        .options([("search_path", schema)]);
    PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .expect("connect a pool")
}

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

/// Serves one Webz page. When `hold` is set, a request announces itself on
/// `started` and then waits until `release` is signalled.
struct Inner {
    body: Vec<u8>,
    calls: AtomicU32,
    started: Notify,
    release: Notify,
    hold: bool,
}

#[derive(Clone)]
struct SharedFetch(Arc<Inner>);

impl SharedFetch {
    fn new(posts: serde_json::Value, hold: bool) -> Self {
        Self(Arc::new(Inner {
            body: serde_json::to_vec(&json!({ "posts": posts })).unwrap(),
            calls: AtomicU32::new(0),
            started: Notify::new(),
            release: Notify::new(),
            hold,
        }))
    }

    fn calls(&self) -> u32 {
        self.0.calls.load(Ordering::SeqCst)
    }
}

impl Fetch for SharedFetch {
    async fn get(&self, _request: FetchRequest<'_>) -> Result<Vec<u8>, FetchError> {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        if self.0.hold {
            self.0.started.notify_one();
            self.0.release.notified().await;
        }
        Ok(self.0.body.clone())
    }
}

fn post(title: &str, url: &str) -> serde_json::Value {
    json!({
        "title": title,
        "url": url,
        "thread": {"site": "Example News"},
        "published": "2026-10-31T10:00:00Z"
    })
}

fn sources(fetch: SharedFetch) -> NewsSources<SharedFetch> {
    NewsSources {
        fetch,
        webz: Some(WebzConfig {
            token: Secret::new("fixture-token"),
            url: WEBZ_DEFAULT_URL.into(),
            timeout: Duration::from_secs(15),
        }),
        feeds: Vec::new(),
    }
}

struct NoModel;

impl TextModel for NoModel {
    async fn generate(&self, _prompt: &str, _timeout: Duration) -> Result<String, ModelError> {
        panic!("no model is configured");
    }
}

fn no_model() -> Categorizer<NoModel> {
    Categorizer::new(None)
}

fn production(database: &str) -> RefreshTarget<'_> {
    RefreshTarget {
        environment: Environment::Production,
        database,
    }
}

async fn run_once(pool: &PgPool, database: &str, fetch: &SharedFetch) -> Result<Refresh, Exit> {
    refresh_news(
        pool,
        production(database),
        &sources(fetch.clone()),
        &mut no_model(),
        fixed_now,
    )
    .await
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn news_inserts_only_new_urls() {
    let fixture = Fixture::new().await;
    let production_url = url("RUST_TEST_PRODUCTION_DATABASE_URL");
    let database = database_of(&production_url);
    let pool = fixture.pool("RUST_TEST_PRODUCTION_DATABASE_URL").await;
    fixture
        .seed("https://news.example.org/stored", "Stored title")
        .await;

    let fetch = SharedFetch::new(
        json!([
            post("Changed title for a stored url", "https://news.example.org/stored"),
            {
                "title": "IMO adopts new regulation",
                "url": "https://news.example.org/second",
                "thread": {"site": "Example News"},
                "published": "2026-10-31T10:00:00Z",
                "summary": "Provider summary"
            },
            post("Crude oil price rises", "https://news.example.org/third"),
        ]),
        false,
    );
    let outcome = run_once(&pool, &database, &fetch).await;
    assert_eq!(
        outcome,
        Ok(Refresh::Completed {
            fetched: 3,
            new: 2,
            inserted: 2
        })
    );
    assert_eq!(fetch.calls(), 1);

    let rows = fixture.rows().await;
    assert_eq!(rows.len(), 3);
    let by_url = |url: &str| rows.iter().find(|row| row.0 == url).unwrap().clone();

    // The stored row is untouched.
    let stored = by_url("https://news.example.org/stored");
    assert_eq!(stored.1, "Stored title");
    assert_eq!(stored.5, "carbon");

    // New rows carry the Python columns: local labels, provider summary,
    // source, publisher origin, the published time and the run time.
    let second = by_url("https://news.example.org/second");
    assert_eq!(second.1, "IMO adopts new regulation");
    assert_eq!(second.2.as_deref(), Some("Provider summary"));
    assert_eq!(second.3, "Example News");
    assert_eq!(second.4, "https://news.example.org");
    assert_eq!(second.5, "regulation");
    assert_eq!(second.6, 3);
    assert_eq!(
        second.7,
        Utc.with_ymd_and_hms(2026, 10, 31, 10, 0, 0).unwrap()
    );
    assert_eq!(second.8, fixed_now());
    let third = by_url("https://news.example.org/third");
    assert_eq!(third.2, None);
    assert_eq!(third.5, "commodities");

    // A second run finds every URL stored and adds nothing.
    let outcome = run_once(&pool, &database, &fetch).await;
    assert_eq!(
        outcome,
        Ok(Refresh::Completed {
            fetched: 3,
            new: 0,
            inserted: 0
        })
    );
    assert_eq!(fixture.rows().await.len(), 3);

    // The transaction lock is gone after the run: a session lock on the same
    // key is available at once.
    let mut session = fixture.admin.acquire().await.unwrap();
    let free: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(NEWS_REFRESH_ADVISORY_LOCK_ID)
        .fetch_one(&mut *session)
        .await
        .unwrap();
    assert!(free, "the refresh left no lock behind");
    sqlx::query("SELECT pg_advisory_unlock_all()")
        .execute(&mut *session)
        .await
        .unwrap();
    drop(session);
    fixture.finish().await;
}

#[tokio::test]
async fn an_empty_fetch_commits_nothing_and_succeeds() {
    let fixture = Fixture::new().await;
    let database = database_of(&url("RUST_TEST_PRODUCTION_DATABASE_URL"));
    let pool = fixture.pool("RUST_TEST_PRODUCTION_DATABASE_URL").await;
    let fetch = SharedFetch::new(json!([]), false);
    assert_eq!(
        run_once(&pool, &database, &fetch).await,
        Ok(Refresh::Completed {
            fetched: 0,
            new: 0,
            inserted: 0
        })
    );
    assert!(fixture.rows().await.is_empty());
    fixture.finish().await;
}

#[tokio::test]
async fn news_lock_busy_is_a_successful_noop() {
    let fixture = Fixture::new().await;
    let database = database_of(&url("RUST_TEST_PRODUCTION_DATABASE_URL"));
    let pool = fixture.pool("RUST_TEST_PRODUCTION_DATABASE_URL").await;
    let fetch = SharedFetch::new(
        json!([post("Port news", "https://news.example.org/a")]),
        false,
    );

    // Another process (the Python command, in production) holds the same
    // transaction-scoped advisory key.
    let mut holder = PgConnection::connect(&url("RUST_TEST_MIGRATOR_DATABASE_URL"))
        .await
        .unwrap();
    sqlx::query("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(NEWS_REFRESH_ADVISORY_LOCK_ID)
        .execute(&mut holder)
        .await
        .unwrap();

    assert_eq!(
        run_once(&pool, &database, &fetch).await,
        Ok(Refresh::LockBusy)
    );
    assert_eq!(fetch.calls(), 0, "a busy lock means no provider request");
    assert!(fixture.rows().await.is_empty());

    // Once the holder commits, the next run works: nothing stays locked.
    sqlx::query("COMMIT").execute(&mut holder).await.unwrap();
    assert_eq!(
        run_once(&pool, &database, &fetch).await,
        Ok(Refresh::Completed {
            fetched: 1,
            new: 1,
            inserted: 1
        })
    );
    fixture.finish().await;
}

#[tokio::test]
async fn news_overlap_inserts_once() {
    let fixture = Fixture::new().await;
    let database = database_of(&url("RUST_TEST_PRODUCTION_DATABASE_URL"));
    let first_pool = fixture.pool("RUST_TEST_PRODUCTION_DATABASE_URL").await;
    let second_pool = fixture.pool("RUST_TEST_PRODUCTION_DATABASE_URL").await;
    let fetch = SharedFetch::new(
        json!([
            post("Port news one", "https://news.example.org/one"),
            post("Port news two", "https://news.example.org/two"),
        ]),
        true,
    );

    // The first run takes the lock and waits inside its provider request while
    // the second part of the case acts as the overlapping process.
    let first = run_once(&first_pool, &database, &fetch);
    let overlap = async {
        fetch.0.started.notified().await;

        // The key is the Python job's key: the held lock is an advisory lock in
        // that exact key space, so the Python command also sees it as busy.
        let held: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_locks \
             WHERE locktype = 'advisory' AND granted AND objsubid = 1 \
               AND classid::bigint = ($1::bigint >> 32) & 4294967295 \
               AND objid::bigint = $1::bigint & 4294967295",
        )
        .bind(NEWS_REFRESH_ADVISORY_LOCK_ID)
        .fetch_one(&fixture.admin)
        .await
        .unwrap();
        assert_eq!(held, 1, "the running refresh holds the advisory key");
        let python_view: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
            .bind(NEWS_REFRESH_ADVISORY_LOCK_ID)
            .fetch_one(&fixture.admin)
            .await
            .unwrap();
        assert!(!python_view, "a second claimant of the key is refused");

        // The overlapping run is a no-op and asks the provider for nothing.
        assert_eq!(
            run_once(&second_pool, &database, &fetch).await,
            Ok(Refresh::LockBusy)
        );
        assert_eq!(fetch.calls(), 1);
        fetch.0.release.notify_one();
    };
    let (first_outcome, ()) = tokio::join!(first, overlap);
    assert_eq!(
        first_outcome,
        Ok(Refresh::Completed {
            fetched: 2,
            new: 2,
            inserted: 2
        })
    );
    assert_eq!(fetch.calls(), 1, "one provider request in all");
    assert_eq!(fixture.rows().await.len(), 2, "no duplicate URL");
    fixture.finish().await;
}

/// A model that stores a competing row for the same URL after the job has
/// looked, then answers with a fixed label.
struct RacingModel {
    admin: PgPool,
    url: String,
}

impl TextModel for RacingModel {
    async fn generate(&self, _prompt: &str, _timeout: Duration) -> Result<String, ModelError> {
        sqlx::query(
            "INSERT INTO news_items (id, title, source, source_url, url, published_at, fetched_at) \
             VALUES ($1, 'Other writer', 'Other', 'https://other.example.org', $2, now(), now()) \
             ON CONFLICT (url) DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(&self.url)
        .execute(&self.admin)
        .await
        .expect("competing insert");
        Ok(json!({"category": "carbon", "relevance": 5, "summary": "Model summary"}).to_string())
    }
}

#[tokio::test]
async fn a_url_stored_after_the_check_is_skipped_and_model_labels_are_kept() {
    let fixture = Fixture::new().await;
    let database = database_of(&url("RUST_TEST_PRODUCTION_DATABASE_URL"));
    let pool = fixture.pool("RUST_TEST_PRODUCTION_DATABASE_URL").await;
    let raced = "https://news.example.org/raced";
    let fetch = SharedFetch::new(
        json!([
            post("Raced headline", raced),
            post("Plain headline", "https://news.example.org/plain")
        ]),
        false,
    );
    let model = RacingModel {
        admin: fixture.admin.clone(),
        url: raced.to_owned(),
    };
    let outcome = refresh_news(
        &pool,
        production(&database),
        &sources(fetch),
        &mut Categorizer::new(Some(model)),
        fixed_now,
    )
    .await;
    assert_eq!(
        outcome,
        Ok(Refresh::Completed {
            fetched: 2,
            new: 2,
            inserted: 1
        }),
        "the conflicting URL is skipped, not an error"
    );
    let rows = fixture.rows().await;
    assert_eq!(rows.len(), 2);
    let raced_row = rows.iter().find(|row| row.0 == raced).unwrap();
    assert_eq!(
        raced_row.1, "Other writer",
        "the earlier writer keeps its row"
    );
    let plain = rows.iter().find(|row| row.0.ends_with("/plain")).unwrap();
    assert_eq!(plain.2.as_deref(), Some("Model summary"));
    assert_eq!(plain.5, "carbon");
    assert_eq!(plain.6, 5);
    fixture.finish().await;
}

#[tokio::test]
async fn target_attestation_refuses_before_any_fetch_or_write() {
    let fixture = Fixture::new().await;
    let production_url = url("RUST_TEST_PRODUCTION_DATABASE_URL");
    let database = database_of(&production_url);
    let posts = json!([post("Port news", "https://news.example.org/a")]);

    // The staging role asked to act as production.
    let fetch = SharedFetch::new(posts.clone(), false);
    let staging_pool = fixture.pool("RUST_TEST_STAGING_DATABASE_URL").await;
    assert_eq!(
        run_once(&staging_pool, &database, &fetch).await,
        Err(Exit::Precondition)
    );

    // A role that is not a deployed one.
    let other_pool = fixture.pool("RUST_TEST_DATABASE_URL").await;
    assert_eq!(
        run_once(&other_pool, &database, &fetch).await,
        Err(Exit::Precondition)
    );

    // The right role on the wrong database.
    let wrong_pool = pool_for(&url("RUST_TEST_WRONG_DATABASE_URL"), &fixture.schema).await;
    assert_eq!(
        run_once(&wrong_pool, &database, &fetch).await,
        Err(Exit::Precondition)
    );

    assert_eq!(
        fetch.calls(),
        0,
        "no provider request after a refused target"
    );
    assert!(fixture.rows().await.is_empty());

    // The staging role passes as staging.
    let outcome = refresh_news(
        &staging_pool,
        RefreshTarget {
            environment: Environment::Staging,
            database: &database,
        },
        &sources(fetch.clone()),
        &mut no_model(),
        fixed_now,
    )
    .await;
    assert_eq!(
        outcome,
        Ok(Refresh::Completed {
            fetched: 1,
            new: 1,
            inserted: 1
        })
    );
    fixture.finish().await;
}

#[tokio::test]
async fn a_database_failure_is_a_dependency_exit_and_writes_nothing() {
    let fixture = Fixture::new().await;
    let database = database_of(&url("RUST_TEST_PRODUCTION_DATABASE_URL"));
    // The table is not on this pool's search path.
    let pool = pool_for(&url("RUST_TEST_PRODUCTION_DATABASE_URL"), "pg_catalog").await;
    let fetch = SharedFetch::new(
        json!([post("Port news", "https://news.example.org/a")]),
        false,
    );
    assert_eq!(
        run_once(&pool, &database, &fetch).await,
        Err(Exit::Dependency)
    );
    assert!(fixture.rows().await.is_empty());
    fixture.finish().await;
}
