//! Test-only stand-in for the deploy/recovery owner. It writes the same record
//! format and the same database row as `scripts/rust_cache_authority.py`; the
//! Python tests prove that script produces exactly these shapes.
#![allow(dead_code)]

use sqlx::PgConnection;
use std::path::{Path, PathBuf};
use verdaxis_read::runtime::cache_authority::Authority;

/// Owner identities are random lowercase hex, 64 characters here.
pub fn new_identity() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub fn authority(revision: u64) -> Authority {
    Authority {
        database_epoch: new_identity(),
        primary_identity: new_identity(),
        revision,
    }
}

/// Bind a database incarnation row exactly as the owner's migrator step does.
pub async fn bind(connection: &mut PgConnection, authority: &Authority, active: bool) {
    sqlx::query(
        "UPDATE public.public_read_cache_incarnation \
         SET database_epoch = $1, primary_identity = $2, authority_revision = $3, active = $4 \
         WHERE singleton",
    )
    .bind(&authority.database_epoch)
    .bind(&authority.primary_identity)
    .bind(i64::try_from(authority.revision).unwrap())
    .bind(active)
    .execute(connection)
    .await
    .unwrap();
}

/// Leave the shared disposable database inactive for the next test.
pub async fn reset(connection: &mut PgConnection) {
    sqlx::query(
        "UPDATE public.public_read_cache_incarnation SET active = false, authority_revision = 0",
    )
    .execute(connection)
    .await
    .unwrap();
}

pub async fn generation(connection: &mut PgConnection) -> i64 {
    sqlx::query_scalar(
        "SELECT generation FROM public.public_read_cache_generations WHERE domain = 'trading'",
    )
    .fetch_one(connection)
    .await
    .unwrap()
}

/// Only ever moves forward past rows already in the event table, so the other
/// suites' writers never meet a duplicate event key.
pub async fn set_generation(connection: &mut PgConnection, generation: i64) {
    sqlx::query(
        "UPDATE public.public_read_cache_generations SET generation = $1 WHERE domain = 'trading'",
    )
    .bind(generation)
    .execute(connection)
    .await
    .unwrap();
}

/// One temporary owner-record location, removed on drop.
pub struct Record {
    directory: PathBuf,
    path: PathBuf,
}

impl Record {
    pub fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("verdaxis-cache-{}", &new_identity()[..16]));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("cache-authority.json");
        Self { directory, path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Atomic publication: write a sibling, then rename it over the record.
    fn publish(&self, body: &str) {
        let temporary = self.directory.join(".record.tmp");
        std::fs::write(&temporary, body).unwrap();
        std::fs::rename(&temporary, &self.path).unwrap();
    }

    pub fn activate(&self, authority: &Authority) {
        self.publish(&format!(
            "{{\"version\":1,\"state\":\"active\",\"database_epoch\":\"{}\",\"primary_identity\":\"{}\",\"authority_revision\":{}}}",
            authority.database_epoch, authority.primary_identity, authority.revision
        ));
    }

    pub fn revoke(&self, revision: u64) {
        self.publish(&format!(
            "{{\"version\":1,\"state\":\"revoked\",\"authority_revision\":{revision}}}"
        ));
    }

    pub fn write_raw(&self, body: &str) {
        self.publish(body);
    }

    pub fn remove(&self) {
        std::fs::remove_file(&self.path).unwrap();
    }

    /// A record reachable only through a symlink must never be trusted.
    pub fn replace_with_symlink(&self, authority: &Authority) {
        let target = self.directory.join("real-record.json");
        std::fs::write(
            &target,
            format!(
                "{{\"version\":1,\"state\":\"active\",\"database_epoch\":\"{}\",\"primary_identity\":\"{}\",\"authority_revision\":{}}}",
                authority.database_epoch, authority.primary_identity, authority.revision
            ),
        )
        .unwrap();
        let _ = std::fs::remove_file(&self.path);
        std::os::unix::fs::symlink(&target, &self.path).unwrap();
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
