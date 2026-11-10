//! Signup-canary cleanup: remove the synthetic user and organization that the
//! external monitor's signup canary creates.
//!
//! One function serves both callers: `POST /api/monitor/signup-canary-cleanup`
//! (`routes::monitor_canary`) and `canary cleanup --email <address>
//! --environment <env> --release-sha <sha> --apply`. It follows
//! `app/routers/monitor.py` in one transaction:
//!
//! 1. lock the user with exactly this email (`FOR UPDATE`);
//! 2. refuse if a retained KYC document names the user or its organization,
//!    then delete the user;
//! 3. lock the user's organization, or else the organization whose domain is
//!    the canary domain;
//! 4. when that organization's domain is the canary domain and it has no
//!    member left, refuse if a retained KYC document names it, then delete it;
//! 5. commit.
//!
//! A refusal rolls everything back, so a retained document blocks both
//! deletions. Two concurrent cleanups serialize on the user row and then on
//! the organization row; the second one finds nothing and reports zeros.
//!
//! The application deletes rows through plain statements. Python deleted
//! through its ORM, which would also have cleared referral links to the user;
//! a canary account never has one, so here such a link makes the delete fail
//! instead of being cleared silently.
use serde::Serialize;
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

use super::context::{self, Purpose};
use super::gate::{require_apply, require_target, require_unit_deploy_state_absent};
use super::{CanaryAction, CanaryArgs, Exit};
use crate::business::attention::is_monitor_canary_email;

/// What one cleanup deleted. The field order is the response body's.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub deleted_users: u64,
    pub deleted_orgs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupError {
    /// A retained KYC document names the user or the organization (HTTP 409).
    Retained,
    /// The database failed or refused a statement; nothing was committed.
    Database,
}

pub const RETAINED_CODE: &str = "KYC_DOCUMENTS_RETAINED";
pub const RETAINED_MESSAGE: &str =
    "Retained KYC documents prevent deletion of this account or organization";

/// The canary domain of an already normalized email: a `canary+` local part
/// and a domain under `.canary.verdaxis.exchange`. Anything else is `None`.
pub fn canary_domain(email: &str) -> Option<&str> {
    is_monitor_canary_email(email).then(|| email.split_once('@').map_or("", |(_, domain)| domain))
}

/// Run the cleanup on one pooled connection.
pub async fn cleanup(pool: &PgPool, email: &str, domain: &str) -> Result<Counts, CleanupError> {
    let mut connection = pool.acquire().await.map_err(|_| CleanupError::Database)?;
    cleanup_on(&mut connection, email, domain).await
}

/// Run the cleanup in one transaction on this connection.
pub async fn cleanup_on(
    connection: &mut PgConnection,
    email: &str,
    domain: &str,
) -> Result<Counts, CleanupError> {
    // The route runs on the read pool, whose sessions default to read-only
    // transactions; this cleanup writes, so it opens a read-write one.
    let mut transaction = connection
        .begin_with("BEGIN READ WRITE")
        .await
        .map_err(|_| CleanupError::Database)?;
    let mut counts = Counts::default();

    let user: Option<(Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT id, organization_id FROM users WHERE email = $1 FOR UPDATE")
            .bind(email)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| CleanupError::Database)?;
    let organization_id = user.and_then(|(_, organization)| organization);
    if let Some((user_id, _)) = user {
        // Both identities are checked before either is deleted.
        require_no_retained_documents(&mut transaction, Some(user_id), organization_id).await?;
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| CleanupError::Database)?;
        counts.deleted_users = 1;
    }

    let mut organization: Option<(Uuid, Option<String>)> = None;
    if let Some(id) = organization_id {
        organization =
            sqlx::query_as("SELECT id, domain FROM organizations WHERE id = $1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(|_| CleanupError::Database)?;
    }
    if organization.is_none() {
        // `organizations.domain` is unique, so at most one row matches.
        organization =
            sqlx::query_as("SELECT id, domain FROM organizations WHERE domain = $1 FOR UPDATE")
                .bind(domain)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(|_| CleanupError::Database)?;
    }
    if let Some((id, Some(organization_domain))) = organization
        && organization_domain == domain
    {
        let members: i64 =
            sqlx::query_scalar("SELECT count(*) FROM users WHERE organization_id = $1")
                .bind(id)
                .fetch_one(&mut *transaction)
                .await
                .map_err(|_| CleanupError::Database)?;
        if members == 0 {
            require_no_retained_documents(&mut transaction, None, Some(id)).await?;
            sqlx::query("DELETE FROM organizations WHERE id = $1")
                .bind(id)
                .execute(&mut *transaction)
                .await
                .map_err(|_| CleanupError::Database)?;
            counts.deleted_orgs = 1;
        }
    }

    transaction
        .commit()
        .await
        .map_err(|_| CleanupError::Database)?;
    Ok(counts)
}

/// `require_no_retained_documents`: any KYC document of the user or the
/// organization refuses. A refusal drops the transaction, which rolls back.
async fn require_no_retained_documents(
    connection: &mut PgConnection,
    user_id: Option<Uuid>,
    organization_id: Option<Uuid>,
) -> Result<(), CleanupError> {
    let retained: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM kyc_documents WHERE user_id = $1 OR organization_id = $2 LIMIT 1",
    )
    .bind(user_id)
    .bind(organization_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| CleanupError::Database)?;
    match retained {
        Some(_) => Err(CleanupError::Retained),
        None => Ok(()),
    }
}

/// Entry point for `canary cleanup`. The operator runs it with the serve
/// environment (`ENVIRONMENT`, `RELEASE_SHA`, `DATABASE_URL`) and
/// `DEPLOY_STATE_DIR`. Refusals come before any statement, in this order:
/// `--apply`, the deploy state, the identity and app URL, the email; the
/// connection that deletes is then attested. Prints one JSON line on stdout.
pub async fn command(args: &CanaryArgs) -> Exit {
    let CanaryAction::Cleanup = args.action;
    let result = run_command(args).await;
    let line = match &result {
        Ok(counts) => serde_json::json!({
            "command": "canary cleanup",
            "outcome": "cleaned",
            "deleted_users": counts.deleted_users,
            "deleted_orgs": counts.deleted_orgs,
        }),
        Err((_, outcome)) => serde_json::json!({"command": "canary cleanup", "outcome": outcome}),
    };
    println!("{line}");
    result.map_or_else(|(exit, _)| exit, |_| Exit::Ok)
}

async fn run_command(args: &CanaryArgs) -> Result<Counts, (Exit, &'static str)> {
    require_apply(args.apply).map_err(|exit| (exit, "usage_refused"))?;
    let environment = args.identity.environment;
    require_unit_deploy_state_absent(environment).map_err(|exit| (exit, "deploy_state_refused"))?;
    let context = context::open(Purpose::Operator, environment, &args.identity.release_sha)
        .await
        .map_err(|exit| (exit, "context_refused"))?;
    let email =
        crate::crud::referrals::invite::normalize_email(&serde_json::json!({"email": args.email}))
            .map_err(|_| (Exit::Usage, "invalid_email"))?;
    let domain = canary_domain(&email).ok_or((Exit::Usage, "not_a_canary_email"))?;
    let database = context
        .pool
        .connect_options()
        .get_database()
        .ok_or((Exit::Precondition, "context_refused"))?
        .to_owned();
    let mut connection = context
        .pool
        .acquire()
        .await
        .map_err(|_| (Exit::Dependency, "dependency_unavailable"))?;
    require_target(&mut connection, environment, &database)
        .await
        .map_err(|exit| (exit, "target_refused"))?;
    cleanup_on(&mut connection, &email, domain)
        .await
        .map_err(|error| match error {
            CleanupError::Retained => (Exit::Precondition, "kyc_documents_retained"),
            CleanupError::Database => (Exit::Dependency, "dependency_unavailable"),
        })
}
