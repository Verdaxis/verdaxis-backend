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
    status_with(connection, &MIGRATOR).await
}

/// [`status`] against another migrator (tests with an added migration).
pub async fn status_with(
    connection: &mut PgConnection,
    migrator: &Migrator,
) -> Result<Status, sqlx::Error> {
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
    let embedded = |version: i64| {
        migrator
            .iter()
            .find(|migration| migration.version == version)
    };
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
    let pending = migrator
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

/// The compiled baseline manifest (`deploy/rust/sqlx-baseline.json`).
const BASELINE_MANIFEST: &str = include_str!("../../../../deploy/rust/sqlx-baseline.json");

/// The fields of the baseline manifest that adoption checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineRecord {
    pub alembic_head: String,
    /// The catalog of an Alembic-built database (production, staging).
    pub fingerprint: String,
    pub file_sha256: String,
}

/// The manifest compiled into this binary.
pub fn compiled_baseline() -> BaselineRecord {
    let document: serde_json::Value =
        serde_json::from_str(BASELINE_MANIFEST).expect("the compiled baseline manifest is JSON");
    let text = |key: &str| {
        document[key]
            .as_str()
            .unwrap_or_else(|| panic!("the compiled baseline manifest has no {key}"))
            .to_owned()
    };
    BaselineRecord {
        alembic_head: text("alembic_head"),
        fingerprint: text("fingerprint"),
        file_sha256: text("file_sha256"),
    }
}

/// Why adoption refused. Every refusal happens before the first write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdoptionRefusal {
    /// `alembic_version` is not exactly one row equal to the expected literal.
    AlembicRevision,
    /// The expected literal is not the head the baseline was cut from.
    BaselineHead,
    /// The sqlx history holds anything but the exact adoption row.
    History,
    /// The live catalog differs from the baseline's Alembic-built catalog.
    Fingerprint,
    /// The embedded baseline file or migration list differs from the manifest.
    EmbeddedBaseline,
    Database,
}

impl From<sqlx::Error> for AdoptionRefusal {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adoption {
    /// This run created the history with the version 1 row.
    Adopted,
    /// The exact adoption row was already there; nothing was written.
    AlreadyAdopted,
}

/// The table definition sqlx itself creates for its history.
const HISTORY_DDL: &str = "CREATE TABLE public._sqlx_migrations (\
    version BIGINT PRIMARY KEY, \
    description TEXT NOT NULL, \
    installed_on TIMESTAMPTZ NOT NULL DEFAULT now(), \
    success BOOLEAN NOT NULL, \
    checksum BYTEA NOT NULL, \
    execution_time BIGINT NOT NULL)";

fn baseline_migration() -> Result<&'static sqlx::migrate::Migration, AdoptionRefusal> {
    let mut migrations = MIGRATOR.iter();
    match (migrations.next(), migrations.next()) {
        (Some(first), None) if first.version == 1 => Ok(first),
        _ => Err(AdoptionRefusal::EmbeddedBaseline),
    }
}

/// Adopt an Alembic-built database into the sqlx history (plan section 8.4):
/// every check runs before the first write, then one transaction creates the
/// history table with sqlx's own definition and records version 1 with the
/// embedded checksum. An exact earlier adoption is reported without a write.
pub async fn adopt(
    connection: &mut PgConnection,
    expected_current: &str,
    baseline: &BaselineRecord,
) -> Result<Adoption, AdoptionRefusal> {
    let migration = baseline_migration()?;
    let embedded_digest: String = Sha256::digest(migration.sql.as_str().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if embedded_digest != baseline.file_sha256 {
        return Err(AdoptionRefusal::EmbeddedBaseline);
    }
    if expected_current != baseline.alembic_head {
        return Err(AdoptionRefusal::BaselineHead);
    }
    let current = status(connection).await?;
    if current.alembic.as_slice() != [expected_current.to_owned()] {
        return Err(AdoptionRefusal::AlembicRevision);
    }
    let exact_adoption = |row: &HistoryRow| {
        row.version == migration.version
            && row.success
            && row.checksum.as_slice() == &*migration.checksum
            && row.description == migration.description
    };
    match current.history.as_slice() {
        [] => {}
        [row] if exact_adoption(row) => {
            // A resume checks the catalog again and writes nothing.
            if fingerprint(connection).await? != baseline.fingerprint {
                return Err(AdoptionRefusal::Fingerprint);
            }
            return Ok(Adoption::AlreadyAdopted);
        }
        _ => return Err(AdoptionRefusal::History),
    }
    if fingerprint(connection).await? != baseline.fingerprint {
        return Err(AdoptionRefusal::Fingerprint);
    }
    let history_exists = table_exists(connection, HISTORY_TABLE).await?;
    let mut transaction = connection.begin().await?;
    if !history_exists {
        sqlx::query(HISTORY_DDL).execute(&mut *transaction).await?;
    }
    sqlx::query(
        "INSERT INTO public._sqlx_migrations \
           (version, description, installed_on, success, checksum, execution_time) \
         VALUES ($1, $2, now(), true, $3, 0)",
    )
    .bind(migration.version)
    .bind(&*migration.description)
    .bind(&*migration.checksum)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    // Read the row back: the commit must have recorded exactly what sqlx expects.
    match status(connection).await?.history.as_slice() {
        [row] if exact_adoption(row) => Ok(Adoption::Adopted),
        _ => Err(AdoptionRefusal::History),
    }
}

/// The approved exact current -> target transitions (`deploy/migration-checkpoints.tsv`).
const CHECKPOINTS: &str = include_str!("../../../../deploy/migration-checkpoints.tsv");

/// Exact current -> target pairs. There is no "head" alias and no range: a
/// transition runs only when its exact pair is listed, a no-change run included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionPolicy {
    rows: Vec<(String, String)>,
}

impl TransitionPolicy {
    /// Parse tab-separated `current<TAB>target` rows; `#` lines are comments.
    pub fn parse(text: &str) -> Option<Self> {
        let mut rows = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let (current, target) = line.split_once('\t')?;
            let literal = |value: &str| {
                !value.is_empty()
                    && value.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                    })
            };
            if !literal(current) || !literal(target) {
                return None;
            }
            rows.push((current.to_owned(), target.to_owned()));
        }
        Some(Self { rows })
    }

    /// The policy compiled into this binary.
    pub fn compiled() -> Self {
        Self::parse(CHECKPOINTS).expect("the compiled checkpoint policy parses")
    }

    pub fn allows(&self, current: &str, target: &str) -> bool {
        self.rows
            .iter()
            .any(|(from, to)| from == current && to == target)
    }
}

/// Why `run` refused. Every refusal happens before the first write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunRefusal {
    /// The exact current -> target pair is not an approved transition.
    Transition,
    /// The database is not at the expected current revision.
    CurrentRevision,
    /// The target is not `sqlx_<version>` of an embedded migration.
    Target,
    /// The history holds a failed, unknown or changed version.
    History,
    Database,
}

impl From<sqlx::Error> for RunRefusal {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}

impl From<sqlx::migrate::MigrateError> for RunRefusal {
    fn from(_: sqlx::migrate::MigrateError) -> Self {
        Self::Database
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// The database is already at the target; nothing was written.
    AtTarget,
    /// Without `--apply`: the versions a real run would apply, in order.
    WouldApply(Vec<i64>),
    /// The versions this run applied, in order.
    Applied(Vec<i64>),
}

fn sqlx_version(literal: &str) -> Option<i64> {
    let digits = literal.strip_prefix("sqlx_")?;
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || digits.starts_with('0')
    {
        return None;
    }
    digits.parse().ok()
}

/// Move an adopted database from `expected_current` to `target` (both
/// `sqlx_<version>` literals) through the exact approved transition. Adoption
/// itself is the separate `baseline` action. Without `apply` nothing is written.
pub async fn run_to_target(
    connection: &mut PgConnection,
    migrator: &Migrator,
    policy: &TransitionPolicy,
    expected_current: &str,
    target: &str,
    apply: bool,
) -> Result<RunOutcome, RunRefusal> {
    if !policy.allows(expected_current, target) {
        return Err(RunRefusal::Transition);
    }
    let current_version = sqlx_version(expected_current).ok_or(RunRefusal::Transition)?;
    let target_version = sqlx_version(target).ok_or(RunRefusal::Target)?;
    if target_version < current_version
        || !migrator
            .iter()
            .any(|migration| migration.version == target_version)
    {
        return Err(RunRefusal::Target);
    }
    let before = status_with(connection, migrator).await?;
    if !before.foreign.is_empty() {
        return Err(RunRefusal::History);
    }
    if before.revision().as_deref() != Some(expected_current) {
        return Err(RunRefusal::CurrentRevision);
    }
    let due: Vec<i64> = before
        .pending
        .iter()
        .copied()
        .filter(|version| *version <= target_version)
        .collect();
    if due.is_empty() {
        return Ok(RunOutcome::AtTarget);
    }
    if !apply {
        return Ok(RunOutcome::WouldApply(due));
    }
    migrator.run_to(target_version, &mut *connection).await?;
    let after = status_with(connection, migrator).await?;
    if !after.foreign.is_empty() || after.revision().as_deref() != Some(target) {
        return Err(RunRefusal::History);
    }
    Ok(RunOutcome::Applied(due))
}
