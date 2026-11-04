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
    // A deploy interrupted after this approved migration committed reruns
    // with the same approval: the database is already at the target.
    if before.revision().as_deref() == Some(target) {
        return Ok(RunOutcome::AtTarget);
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

/// The revision literal adoption moves an Alembic-built database to.
pub const ADOPTED_REVISION: &str = "sqlx_1";

/// One approved deploy transition (plan section 8.5), from any current
/// literal the checkpoint policy names:
///
/// - `sqlx_<a>` -> `sqlx_<b>`: [`run_to_target`];
/// - an Alembic literal to itself: no schema change; the database must report
///   exactly that literal;
/// - the baseline's Alembic head -> `sqlx_1`: adoption ([`adopt`]). A rerun
///   after adoption committed finds the exact adoption row and writes nothing,
///   so an interrupted deploy resumes with the same approval.
///
/// Any other Alembic upgrade is refused: it needs the Python schema tool,
/// which a Python-free deploy does not run.
pub async fn run_transition(
    connection: &mut PgConnection,
    migrator: &Migrator,
    policy: &TransitionPolicy,
    baseline: &BaselineRecord,
    expected_current: &str,
    target: &str,
    apply: bool,
) -> Result<RunOutcome, RunRefusal> {
    if !policy.allows(expected_current, target) {
        return Err(RunRefusal::Transition);
    }
    match (sqlx_version(expected_current), sqlx_version(target)) {
        (Some(_), Some(_)) => {
            run_to_target(
                connection,
                migrator,
                policy,
                expected_current,
                target,
                apply,
            )
            .await
        }
        (None, None) if expected_current == target => {
            let current = status_with(connection, migrator).await?;
            if !current.foreign.is_empty() {
                return Err(RunRefusal::History);
            }
            if current.revision().as_deref() == Some(expected_current) {
                Ok(RunOutcome::AtTarget)
            } else {
                Err(RunRefusal::CurrentRevision)
            }
        }
        (None, Some(_)) if target == ADOPTED_REVISION => {
            if !apply {
                let current = status_with(connection, migrator).await?;
                return match current.revision().as_deref() {
                    _ if !current.foreign.is_empty() => Err(RunRefusal::History),
                    Some(revision) if revision == expected_current => {
                        Ok(RunOutcome::WouldApply(vec![1]))
                    }
                    Some(revision) if revision == target => Ok(RunOutcome::AtTarget),
                    _ => Err(RunRefusal::CurrentRevision),
                };
            }
            match adopt(connection, expected_current, baseline).await {
                Ok(Adoption::Adopted) => Ok(RunOutcome::Applied(vec![1])),
                Ok(Adoption::AlreadyAdopted) => Ok(RunOutcome::AtTarget),
                Err(AdoptionRefusal::Database) => Err(RunRefusal::Database),
                Err(AdoptionRefusal::AlembicRevision) => Err(RunRefusal::CurrentRevision),
                Err(AdoptionRefusal::BaselineHead) => Err(RunRefusal::Target),
                Err(_) => Err(RunRefusal::History),
            }
        }
        _ => Err(RunRefusal::Target),
    }
}

/// What a deploy-owned `migrate baseline` or `migrate run --apply` must find:
/// the deploy that runs it holds the deploy state and both deploy locks.
pub struct DeployOwnership<'a> {
    /// The private deploy directory (`.runtime-deploy`).
    pub deploy_directory: &'a std::path::Path,
    pub environment: super::Environment,
    pub release_sha: &'a str,
    /// The deploy run's nonce, passed to `migrate` and written in its state.
    pub nonce: &'a str,
}

/// Bound for the deploy state text: four short lines.
const MAX_DEPLOY_STATE_BYTES: u64 = 4 * 1024;

fn valid_nonce(nonce: &str) -> bool {
    (32..=128).contains(&nonce.len())
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Refuse unless the deploy state is exactly the migration phase of this
/// environment, release and nonce, and the deploy holds both its locks. The
/// check never creates a lock file and never takes a lock it keeps.
pub fn require_deploy_ownership(owner: &DeployOwnership<'_>) -> Result<(), super::Exit> {
    use super::Exit;
    use std::io::Read;
    if !valid_nonce(owner.nonce) {
        return Err(Exit::Usage);
    }
    let environment = owner.environment.as_str();
    let state_path = owner.deploy_directory.join(format!("{environment}.state"));
    let mut text = String::new();
    super::state::open_regular(&state_path)?
        .take(MAX_DEPLOY_STATE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|_| Exit::Precondition)?;
    let expected = format!(
        "DEPLOYMENT_STATE=blocked\nENVIRONMENT={environment}\nRELEASE_SHA={}\nDEPLOY_NONCE={}\n",
        owner.release_sha, owner.nonce
    );
    if text != expected {
        return Err(Exit::Precondition);
    }
    for lock in [
        format!("{environment}.lock"),
        format!("{environment}.restart.lock"),
    ] {
        let path = owner.deploy_directory.join(lock);
        let entry = std::fs::symlink_metadata(&path).map_err(|_| Exit::Precondition)?;
        if !entry.file_type().is_file() {
            return Err(Exit::Precondition);
        }
        // A lock this check can take is not held by the deploy: refuse (the
        // lock is released again when `free` drops).
        if let Some(free) = super::state::StateLock::try_acquire(&path)? {
            drop(free);
            return Err(Exit::Precondition);
        }
    }
    Ok(())
}

fn report(command: &str, outcome: &str) {
    println!(
        "{}",
        serde_json::json!({"command": format!("migrate {command}"), "outcome": outcome})
    );
}

/// Entry point for `migrate`. The live connection comes from the purpose-bound
/// context; until Task 9 projects the migrator credentials, it refuses.
pub async fn run(args: &super::MigrateArgs) -> super::Exit {
    let _ = tracing_subscriber::fmt()
        .json()
        .with_writer(std::io::stderr)
        .try_init();
    let mut vars: std::collections::HashMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    if let Some(path) = args.environment_file.as_deref() {
        match deployed_migrator_values(path) {
            Ok(values) => vars.extend(values),
            Err(exit) => {
                report("environment_file", "refused");
                return exit;
            }
        }
    }
    let context_vars = vars.clone();
    run_with(args, &vars, |environment, release_sha| async move {
        super::context::RunContext::open_with(
            super::context::Purpose::Migrate,
            environment,
            &release_sha,
            &context_vars,
        )
    })
    .await
}

/// The three keys `migrate` takes from the deployed `.env`: the identity and
/// the migrator URL. The URL stays inside this process (the deploy never
/// reads it); no other key of the file is used.
pub(crate) fn deployed_migrator_values(
    path: &std::path::Path,
) -> Result<Vec<(String, String)>, super::Exit> {
    let values = super::ops::runtime_env::read_environment_file(path)?;
    ["ENVIRONMENT", "RELEASE_SHA", "MIGRATOR_DATABASE_URL"]
        .into_iter()
        .map(|key| match values.get(key) {
            Some(Some(value)) => Ok((key.to_owned(), value.clone())),
            _ => Err(super::Exit::Precondition),
        })
        .collect()
}

/// [`run`] with an injected context opener. Every refusal that needs no
/// database happens before `open` is called.
pub async fn run_with<O, Fut>(
    args: &super::MigrateArgs,
    vars: &std::collections::HashMap<String, String>,
    open: O,
) -> super::Exit
where
    O: FnOnce(super::Environment, String) -> Fut,
    Fut: std::future::Future<Output = Result<super::context::RunContext, super::Exit>>,
{
    use super::{Exit, MigrationAction};
    let writes = match args.action {
        MigrationAction::Status => false,
        MigrationAction::Baseline => true,
        MigrationAction::Run => args.apply,
        MigrationAction::Verify => {
            report("verify", "not_available");
            return Exit::Precondition;
        }
    };
    // `status` never writes; `baseline` always does and needs --apply.
    if (args.action == MigrationAction::Status && args.apply)
        || (args.action == MigrationAction::Baseline && !args.apply)
    {
        return Exit::Usage;
    }
    // `--check` belongs to `status` and needs the literal it checks.
    if args.check && (args.action != MigrationAction::Status || args.expected_current.is_none()) {
        return Exit::Usage;
    }
    let needs_current = args.action != MigrationAction::Status;
    if needs_current && args.expected_current.is_none()
        || args.action == MigrationAction::Run && args.target.is_none()
    {
        return Exit::Usage;
    }
    if writes {
        let Some(directory) = vars.get("DEPLOY_STATE_DIR").map(std::path::PathBuf::from) else {
            return Exit::Precondition;
        };
        if !directory.is_absolute() {
            return Exit::Precondition;
        }
        let owner = DeployOwnership {
            deploy_directory: &directory,
            environment: args.identity.environment,
            release_sha: &args.identity.release_sha,
            nonce: args.deploy_nonce.as_deref().unwrap_or(""),
        };
        if let Err(exit) = require_deploy_ownership(&owner) {
            return exit;
        }
    }
    let context = match open(args.identity.environment, args.identity.release_sha.clone()).await {
        Ok(context) => context,
        Err(exit) => {
            report("connect", "not_available");
            return exit;
        }
    };
    // Migration control runs only as the migrator role, never on the app pool.
    let Some(pool) = context.migrator.as_ref() else {
        return Exit::Precondition;
    };
    let Ok(mut connection) = pool.acquire().await else {
        return Exit::Dependency;
    };
    match args.action {
        MigrationAction::Status => match status(&mut connection).await {
            Ok(current) => {
                println!("{}", current.report());
                // `--check`: the deploy's exact-current gate. The database must
                // report exactly the expected literal with a clean history.
                match args.expected_current.as_deref() {
                    Some(expected) if args.check => {
                        if current.foreign.is_empty()
                            && current.revision().as_deref() == Some(expected)
                        {
                            Exit::Ok
                        } else {
                            report("status", "unexpected_revision");
                            Exit::Precondition
                        }
                    }
                    _ => Exit::Ok,
                }
            }
            Err(_) => Exit::Dependency,
        },
        MigrationAction::Baseline => {
            let expected = args.expected_current.as_deref().unwrap_or_default();
            match adopt(&mut connection, expected, &compiled_baseline()).await {
                Ok(Adoption::Adopted) => {
                    report("baseline", "adopted");
                    Exit::Ok
                }
                Ok(Adoption::AlreadyAdopted) => {
                    report("baseline", "already_adopted");
                    Exit::Ok
                }
                Err(AdoptionRefusal::Database) => Exit::Dependency,
                Err(refusal) => {
                    report("baseline", &format!("refused_{refusal:?}").to_lowercase());
                    Exit::Precondition
                }
            }
        }
        MigrationAction::Run => {
            let outcome = run_transition(
                &mut connection,
                &MIGRATOR,
                &TransitionPolicy::compiled(),
                &compiled_baseline(),
                args.expected_current.as_deref().unwrap_or_default(),
                args.target.as_deref().unwrap_or_default(),
                args.apply,
            )
            .await;
            match outcome {
                Ok(RunOutcome::AtTarget) => {
                    report("run", "at_target");
                    Exit::Ok
                }
                Ok(RunOutcome::WouldApply(_)) => {
                    report("run", "would_apply");
                    Exit::Ok
                }
                Ok(RunOutcome::Applied(_)) => {
                    report("run", "applied");
                    Exit::Ok
                }
                Err(RunRefusal::Database) => Exit::Dependency,
                Err(refusal) => {
                    report("run", &format!("refused_{refusal:?}").to_lowercase());
                    Exit::Precondition
                }
            }
        }
        MigrationAction::Verify => Exit::Precondition,
    }
}
