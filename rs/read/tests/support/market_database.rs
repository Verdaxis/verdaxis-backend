//! Fresh disposable databases cloned from a runner-owned migrated template.
use sqlx::{Connection, PgConnection, Row};
use std::time::Duration;
use uuid::Uuid;

pub struct MarketDatabase {
    pub admin_url: String,
    pub runtime_url: String,
    control_url: String,
    name: String,
}

impl MarketDatabase {
    pub async fn new(prefix: &str) -> Self {
        let admin_url = std::env::var(format!("{prefix}_ADMIN_DATABASE_URL"))
            .or_else(|_| std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL"))
            .expect("explicit disposable administrator URL");
        let runtime_url = std::env::var(format!("{prefix}_DATABASE_URL"))
            .expect("explicit disposable runtime URL");
        let template = std::env::var(format!("{prefix}_TEMPLATE_DATABASE"))
            .expect("explicit disposable template database");
        let control_url = std::env::var("RUST_TEMPLATE_ADMIN_DATABASE_URL")
            .expect("explicit disposable database creator URL");
        let mut admin = url::Url::parse(&admin_url).unwrap();
        let mut runtime = url::Url::parse(&runtime_url).unwrap();
        let control = url::Url::parse(&control_url).unwrap();
        for target in [&admin, &runtime, &control] {
            assert_eq!(target.host_str(), Some("127.0.0.1"));
            assert_eq!(target.port(), control.port());
        }
        assert_eq!(admin.path(), runtime.path());
        assert_eq!(admin.path().trim_start_matches('/'), template);
        assert!(template.ends_with("_test"));
        assert!(template.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
        let name = format!("market_{}_test", Uuid::new_v4().simple());
        let mut connection = PgConnection::connect(&control_url).await.unwrap();
        let owner: String = sqlx::query("SELECT pg_get_userbyid(datdba) AS owner FROM pg_database WHERE datname=$1")
            .bind(&template)
            .fetch_one(&mut connection)
            .await
            .unwrap()
            .get("owner");
        assert!(owner.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
        let create = format!("CREATE DATABASE \"{name}\" TEMPLATE \"{template}\" OWNER \"{owner}\"");
        tokio::time::timeout(Duration::from_secs(30), sqlx::raw_sql(sqlx::AssertSqlSafe(create)).execute(&mut connection))
            .await
            .expect("template clone deadline")
            .unwrap();
        admin.set_path(&format!("/{name}"));
        runtime.set_path(&format!("/{name}"));
        Self { admin_url: admin.into(), runtime_url: runtime.into(), control_url, name }
    }
}

impl Drop for MarketDatabase {
    fn drop(&mut self) {
        let control = self.control_url.clone();
        let name = self.name.clone();
        // A separate runtime permits bounded cleanup during test panic or runtime shutdown.
        let result = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async move {
                tokio::time::timeout(Duration::from_secs(15), async {
                    let mut connection = PgConnection::connect(&control).await?;
                    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)")))
                        .execute(&mut connection).await?;
                    Ok::<_, sqlx::Error>(())
                }).await
            })
        }).join();
        if !matches!(result, Ok(Ok(Ok(())))) {
            eprintln!("disposable market database cleanup did not complete");
        }
    }
}
