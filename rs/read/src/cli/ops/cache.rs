//! `ops cache-authority status|revoke|activate`: publish and rotate the
//! public-read cache authority.
//!
//! The Rust read service serves a cached trading payload only while the
//! owner record in the runtime directory names an active authority and the
//! database connection answering the request reports the same identity. This
//! command is the only writer of that record and of the database row it must
//! match.
//!
//! - `revoke` publishes a revoked record, then waits out every Rust lease.
//! - `activate --apply` revokes and drains, binds the database incarnation row
//!   to a fresh epoch, primary identity and higher revision through the
//!   migrator role, then publishes the matching active record.
//! - `status` prints the record without a full identity.
//!
//! Order makes this safe: the record is revoked before the row changes, the
//! drain wait is twice the lease, and the active record is published only
//! after the row is committed. A failure between the steps leaves the record
//! revoked (caching stays off); the command prints that revision and exits 4.
use std::future::Future;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, Row};

use crate::cli::context::{Purpose, RunContext};
use crate::cli::{CacheAction, CacheArgs, Exit};
use crate::runtime::cache_authority::{
    AUTHORITY_LEASE, Authority, MAX_RECORD_BYTES, parse_owner_record,
};

pub const RECORD_NAME: &str = "cache-authority.json";
/// Twice the lease: every process has re-read the revoked record or let its
/// lease expire before the successor identity is bound.
pub const FENCE_DRAIN: Duration = Duration::from_secs(2 * AUTHORITY_LEASE.as_secs());
/// The revision is a BIGINT column, read as a signed value.
pub const MAX_REVISION: u64 = i64::MAX as u64;

/// A refusal. `revoked` is set once a revoked record was published: caching is
/// then off and the operator must activate again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityError {
    pub message: &'static str,
    pub revoked: Option<u64>,
}

fn refused(message: &'static str) -> AuthorityError {
    AuthorityError {
        message,
        revoked: None,
    }
}

/// One owner record: `authority` is `None` for a revocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub revision: u64,
    pub authority: Option<Authority>,
}

/// The migrator-owned incarnation row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncarnationRow {
    pub database_epoch: String,
    pub primary_identity: String,
    pub authority_revision: u64,
    pub active: bool,
    pub in_recovery: bool,
}

pub trait IncarnationStore {
    fn read(&mut self) -> impl Future<Output = Result<IncarnationRow, AuthorityError>>;
    fn bind(&mut self, authority: &Authority) -> impl Future<Output = Result<(), AuthorityError>>;
}

/// A fresh random identity (32 bytes from the operating system, lowercase
/// hex), never from a database. Without entropy the command cannot run safely.
pub fn new_identity() -> String {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system entropy source is available");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The record text: compact JSON, sorted keys, final newline.
pub fn render_record(record: &Record) -> String {
    let body = match &record.authority {
        None => json!({
            "authority_revision": record.revision,
            "state": "revoked",
            "version": 1,
        }),
        Some(authority) => json!({
            "authority_revision": record.revision,
            "database_epoch": authority.database_epoch,
            "primary_identity": authority.primary_identity,
            "state": "active",
            "version": 1,
        }),
    };
    format!("{body}\n")
}

fn record_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(RECORD_NAME)
}

/// The current record, `None` when absent. A link, a non-regular or an
/// oversized file is refused, as the Rust reader refuses it.
pub fn read_record(runtime_dir: &Path) -> Result<Option<Record>, AuthorityError> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(record_path(runtime_dir))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(refused(
                "cache authority record must be a readable regular file",
            ));
        }
    };
    let metadata = file
        .metadata()
        .map_err(|_| refused("cache authority record must be a readable regular file"))?;
    if !metadata.is_file() || metadata.len() > MAX_RECORD_BYTES {
        return Err(refused(
            "cache authority record must be a small regular file",
        ));
    }
    let mut text = Vec::new();
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut text)
        .map_err(|_| refused("cache authority record must be a readable regular file"))?;
    let (revision, authority) = parse_owner_record(&String::from_utf8_lossy(&text))
        .map_err(|_| refused("invalid cache authority record"))?;
    Ok(Some(Record {
        revision,
        authority,
    }))
}

/// Publish atomically: a private 0600 temp file, fsync, rename, directory fsync.
pub fn publish_record(runtime_dir: &Path, record: &Record) -> Result<(), AuthorityError> {
    let real_directory =
        std::fs::symlink_metadata(runtime_dir).is_ok_and(|metadata| metadata.is_dir());
    if !real_directory {
        return Err(refused(
            "the Rust runtime directory must be a real directory",
        ));
    }
    crate::cli::state::write_atomic_bytes(
        &record_path(runtime_dir),
        render_record(record).as_bytes(),
    )
    .map_err(|_| refused("the cache authority record could not be published"))
}

/// The published revision, or zero for an absent or unreadable record.
fn current_revision(runtime_dir: &Path) -> u64 {
    read_record(runtime_dir)
        .ok()
        .flatten()
        .map_or(0, |record| record.revision)
}

/// Publish a revoked record above `floor` and the current revision, then wait
/// out every Rust lease. Returns the revoked revision.
pub async fn revoke_and_drain<S, F>(
    runtime_dir: &Path,
    floor: u64,
    sleep: S,
) -> Result<u64, AuthorityError>
where
    S: Fn(Duration) -> F,
    F: Future<Output = ()>,
{
    let revision = floor.max(current_revision(runtime_dir)).saturating_add(1);
    if revision > MAX_REVISION - 1 {
        return Err(refused("cache authority revision space is exhausted"));
    }
    publish_record(
        runtime_dir,
        &Record {
            revision,
            authority: None,
        },
    )?;
    sleep(FENCE_DRAIN).await;
    Ok(revision)
}

/// Revoke, drain, bind the database to a fresh identity, publish it.
pub async fn activate<St, S, F>(
    runtime_dir: &Path,
    store: &mut St,
    sleep: S,
) -> Result<Authority, AuthorityError>
where
    St: IncarnationStore,
    S: Fn(Duration) -> F,
    F: Future<Output = ()>,
{
    let row = store.read().await?;
    if row.in_recovery {
        return Err(refused(
            "the database is a replica; promote it before activation",
        ));
    }
    let revoked = revoke_and_drain(runtime_dir, row.authority_revision, sleep).await?;
    let after_revocation = |message: &'static str| AuthorityError {
        message,
        revoked: Some(revoked),
    };
    let authority = Authority {
        database_epoch: new_identity(),
        primary_identity: new_identity(),
        revision: revoked + 1,
    };
    // A copied or reused identity would defeat the fence; the identities are
    // random, so a repeat means the entropy source is broken.
    let fresh = [&authority.database_epoch, &authority.primary_identity];
    let mut earlier = vec![row.database_epoch.clone(), row.primary_identity.clone()];
    if let Ok(Some(Record {
        authority: Some(previous),
        ..
    })) = read_record(runtime_dir)
    {
        earlier.extend([previous.database_epoch, previous.primary_identity]);
    }
    if fresh.iter().any(|identity| earlier.contains(identity)) {
        return Err(after_revocation(
            "allocated identity repeats an earlier identity",
        ));
    }
    // A failure from here leaves the record revoked, so caching stays off.
    store
        .bind(&authority)
        .await
        .map_err(|error| after_revocation(error.message))?;
    publish_record(
        runtime_dir,
        &Record {
            revision: authority.revision,
            authority: Some(authority.clone()),
        },
    )
    .map_err(|error| after_revocation(error.message))?;
    Ok(authority)
}

fn fingerprint(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The operator summary: never a full identity.
pub fn describe(record: Option<&Record>) -> Value {
    match record {
        None => json!({"state": "absent"}),
        Some(Record {
            revision,
            authority: None,
        }) => json!({"authority_revision": revision, "state": "revoked"}),
        Some(Record {
            revision,
            authority: Some(authority),
        }) => json!({
            "authority_revision": revision,
            "database_epoch_fingerprint": fingerprint(&authority.database_epoch),
            "primary_identity_fingerprint": fingerprint(&authority.primary_identity),
            "state": "active",
        }),
    }
}

/// The incarnation row through one migrator connection; each method runs its
/// own transaction.
pub struct SqlIncarnationStore<'a>(pub &'a mut PgConnection);

fn database_failed() -> AuthorityError {
    refused("the cache incarnation row could not be read or bound")
}

impl IncarnationStore for SqlIncarnationStore<'_> {
    async fn read(&mut self) -> Result<IncarnationRow, AuthorityError> {
        let mut transaction = self.0.begin().await.map_err(|_| database_failed())?;
        let row = sqlx::query(
            "SELECT database_epoch, primary_identity, authority_revision, active, \
             pg_catalog.pg_is_in_recovery() AS in_recovery \
             FROM public.public_read_cache_incarnation WHERE singleton",
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| database_failed())?
        .ok_or_else(|| refused("the database has no cache incarnation row"))?;
        transaction.commit().await.map_err(|_| database_failed())?;
        let revision: i64 = row
            .try_get("authority_revision")
            .map_err(|_| database_failed())?;
        Ok(IncarnationRow {
            database_epoch: row
                .try_get("database_epoch")
                .map_err(|_| database_failed())?,
            primary_identity: row
                .try_get("primary_identity")
                .map_err(|_| database_failed())?,
            authority_revision: u64::try_from(revision).map_err(|_| database_failed())?,
            active: row.try_get("active").map_err(|_| database_failed())?,
            in_recovery: row.try_get("in_recovery").map_err(|_| database_failed())?,
        })
    }

    async fn bind(&mut self, authority: &Authority) -> Result<(), AuthorityError> {
        let revision = i64::try_from(authority.revision)
            .map_err(|_| refused("cache authority revision space is exhausted"))?;
        let mut transaction = self.0.begin().await.map_err(|_| database_failed())?;
        let current = sqlx::query(
            "SELECT database_epoch, primary_identity, authority_revision \
             FROM public.public_read_cache_incarnation WHERE singleton FOR UPDATE",
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| database_failed())?
        .ok_or_else(|| refused("the database has no cache incarnation row"))?;
        let current_revision: i64 = current
            .try_get("authority_revision")
            .map_err(|_| database_failed())?;
        if revision <= current_revision {
            return Err(refused("authority revision must increase"));
        }
        let epoch: String = current
            .try_get("database_epoch")
            .map_err(|_| database_failed())?;
        let primary: String = current
            .try_get("primary_identity")
            .map_err(|_| database_failed())?;
        if [&epoch, &primary]
            .iter()
            .any(|old| **old == authority.database_epoch || **old == authority.primary_identity)
        {
            return Err(refused("authority identities must be fresh"));
        }
        let updated = sqlx::query(
            "UPDATE public.public_read_cache_incarnation \
             SET database_epoch = $1, primary_identity = $2, authority_revision = $3, active = true \
             WHERE singleton",
        )
        .bind(&authority.database_epoch)
        .bind(&authority.primary_identity)
        .bind(revision)
        .execute(&mut *transaction)
        .await
        .map_err(|_| database_failed())?;
        if updated.rows_affected() != 1 {
            return Err(refused("the cache incarnation row was not bound"));
        }
        transaction.commit().await.map_err(|_| database_failed())
    }
}

/// The migrator role of the environment, attested on the connection that binds.
async fn attest_migrator(
    connection: &mut PgConnection,
    environment: crate::cli::Environment,
) -> Result<(), AuthorityError> {
    let expected_role = match environment {
        crate::cli::Environment::Production => "verdaxis_migrator",
        crate::cli::Environment::Staging => "verdaxis_migrator_staging",
    };
    let expected_database = crate::cli::ops::runtime_env::identity(environment).database_name;
    let (database, role): (String, String) =
        sqlx::query_as("SELECT current_database()::text, current_user::text")
            .fetch_one(&mut *connection)
            .await
            .map_err(|_| refused("the migrator connection could not be attested"))?;
    if database == expected_database && role == expected_role {
        Ok(())
    } else {
        Err(refused(
            "the connection is not the environment's migrator role",
        ))
    }
}

fn failure(error: &AuthorityError) -> Exit {
    match error.revoked {
        Some(revision) => {
            println!(
                "{}",
                json!({"command": "ops cache-authority", "outcome": "left_revoked", "authority_revision": revision})
            );
            eprintln!("cache authority refused: {}", error.message);
            Exit::Dependency
        }
        None => {
            eprintln!("cache authority refused: {}", error.message);
            Exit::Precondition
        }
    }
}

async fn activate_through_migrator(args: &CacheArgs, environment_file: &Path) -> Exit {
    let Ok(values) = crate::cli::migrate::deployed_migrator_values(environment_file) else {
        eprintln!("cache authority refused: the environment file lacks the migrator identity");
        return Exit::Precondition;
    };
    let variables = values.into_iter().collect();
    let context = match RunContext::open_with(
        Purpose::Migrate,
        args.identity.environment,
        &args.identity.release_sha,
        &variables,
    ) {
        Ok(context) => context,
        Err(exit) => {
            eprintln!("cache authority refused: identity or migrator URL");
            return exit;
        }
    };
    let Some(pool) = context.migrator.as_ref() else {
        return Exit::Precondition;
    };
    let mut connection = match pool.acquire().await {
        Ok(connection) => connection,
        Err(_) => {
            eprintln!("cache authority refused: the database is unavailable");
            return Exit::Dependency;
        }
    };
    if let Err(error) = attest_migrator(&mut connection, args.identity.environment).await {
        return failure(&error);
    }
    let mut store = SqlIncarnationStore(&mut connection);
    match activate(&args.runtime_dir, &mut store, tokio::time::sleep).await {
        Ok(authority) => {
            println!(
                "{}",
                json!({"command": "ops cache-authority activate", "outcome": "activated", "authority_revision": authority.revision})
            );
            Exit::Ok
        }
        Err(error) => failure(&error),
    }
}

/// Entry point for `ops cache-authority`.
pub async fn command(args: &CacheArgs) -> Exit {
    match args.action {
        CacheAction::Status => {
            if args.apply || args.environment_file.is_some() {
                return Exit::Usage;
            }
            match read_record(&args.runtime_dir) {
                Ok(record) => {
                    println!("{}", describe(record.as_ref()));
                    Exit::Ok
                }
                Err(error) => failure(&error),
            }
        }
        CacheAction::Revoke => {
            if args.environment_file.is_some() {
                return Exit::Usage;
            }
            match revoke_and_drain(&args.runtime_dir, 0, tokio::time::sleep).await {
                Ok(revision) => {
                    println!(
                        "{}",
                        json!({"command": "ops cache-authority revoke", "outcome": "revoked", "authority_revision": revision})
                    );
                    Exit::Ok
                }
                Err(error) => failure(&error),
            }
        }
        CacheAction::Activate => {
            let Some(environment_file) = args.environment_file.as_deref() else {
                return Exit::Usage;
            };
            if !args.apply {
                return Exit::Usage;
            }
            activate_through_migrator(args, environment_file).await
        }
    }
}
