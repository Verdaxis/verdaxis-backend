//! `migrate`: the sqlx migration history that replaces Alembic (Phase 10 plan
//! section 8). This part holds the read-only pieces: the embedded migrator, the
//! catalog fingerprint and `migrate status`.
//!
//! Versions, descriptions and checksums always come from sqlx's own embedded
//! `Migrator`; nothing here computes a checksum by hand.
use sha2::{Digest, Sha256};
use sqlx::migrate::Migrator;
use sqlx::{Connection, PgConnection, Row};

/// The migrations compiled into this binary (`rs/read/migrations`).
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// The ordered catalog projection shared with the baseline generator and tests.
const FINGERPRINT_SQL: &str = include_str!("../../../../deploy/rust/catalog-fingerprint.sql");

/// The table sqlx keeps its history in.
pub const HISTORY_TABLE: &str = "_sqlx_migrations";

/// The revision literal a release publishes once sqlx owns the schema.
pub fn sqlx_revision(version: i64) -> String {
    format!("sqlx_{version}")
}

/// SHA-256 of the catalog projection, read in one REPEATABLE READ READ ONLY
/// transaction so every part of the catalog comes from one snapshot.
pub async fn fingerprint(connection: &mut PgConnection) -> Result<String, sqlx::Error> {
    let mut transaction = connection.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *transaction)
        .await?;
    let lines: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(FINGERPRINT_SQL))
        .fetch_all(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(Sha256::digest(lines.join("\n").as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// One row of the sqlx history as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRow {
    pub version: i64,
    pub description: String,
    pub success: bool,
    pub checksum: Vec<u8>,
}

/// What `migrate status` reads. It never writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The rows of `alembic_version`, in order; empty when the table is absent.
    pub alembic: Vec<String>,
    /// The rows of `_sqlx_migrations`, by version; empty when the table is absent.
    pub history: Vec<HistoryRow>,
    /// Embedded versions that the history does not record as successful.
    pub pending: Vec<i64>,
    /// Recorded versions that are failed, unknown to this binary, or carry
    /// another checksum or description than the embedded migration.
    pub foreign: Vec<i64>,
}

impl Status {
    /// The revision literal the database is at: `sqlx_<highest successful
    /// version>` once sqlx history exists, else the single Alembic literal.
    pub fn revision(&self) -> Option<String> {
        if let Some(last) = self
            .history
            .iter()
            .filter(|row| row.success)
            .map(|row| row.version)
            .max()
        {
            return Some(sqlx_revision(last));
        }
        match self.alembic.as_slice() {
            [single] => Some(single.clone()),
            _ => None,
        }
    }

    /// The one-line JSON report; no credential or database URL appears in it.
    pub fn report(&self) -> serde_json::Value {
        serde_json::json!({
            "command": "migrate status",
            "revision": self.revision(),
            "alembic": self.alembic,
            "applied": self.history.iter().filter(|row| row.success).map(|row| row.version).collect::<Vec<_>>(),
            "pending": self.pending,
            "foreign": self.foreign,
        })
    }
}

async fn table_exists(connection: &mut PgConnection, table: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
        .bind(format!("public.{table}"))
        .fetch_one(connection)
        .await
}

/// Read the Alembic and sqlx histories and compare the sqlx history with the
/// embedded migrations. Read-only.
pub async fn status(connection: &mut PgConnection) -> Result<Status, sqlx::Error> {
    let alembic = if table_exists(connection, "alembic_version").await? {
        sqlx::query_scalar("SELECT version_num FROM public.alembic_version ORDER BY version_num")
            .fetch_all(&mut *connection)
            .await?
    } else {
        Vec::new()
    };
    let history: Vec<HistoryRow> = if table_exists(connection, HISTORY_TABLE).await? {
        sqlx::query(
            "SELECT version, description, success, checksum FROM public._sqlx_migrations ORDER BY version",
        )
        .fetch_all(&mut *connection)
        .await?
        .iter()
        .map(|row| HistoryRow {
            version: row.get("version"),
            description: row.get("description"),
            success: row.get("success"),
            checksum: row.get("checksum"),
        })
        .collect()
    } else {
        Vec::new()
    };
    let embedded = |version: i64| MIGRATOR.iter().find(|migration| migration.version == version);
    let foreign = history
        .iter()
        .filter(|row| match embedded(row.version) {
            Some(migration) => {
                !row.success
                    || row.checksum.as_slice() != &*migration.checksum
                    || row.description != migration.description
            }
            None => true,
        })
        .map(|row| row.version)
        .collect();
    let pending = MIGRATOR
        .iter()
        .filter(|migration| {
            !history
                .iter()
                .any(|row| row.version == migration.version && row.success)
        })
        .map(|migration| migration.version)
        .collect();
    Ok(Status {
        alembic,
        history,
        pending,
        foreign,
    })
}
