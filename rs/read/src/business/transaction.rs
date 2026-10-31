use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use serde_json::Value;
use sqlx::{Connection, PgConnection, PgPool, Row};
use tokio::sync::Semaphore;
use uuid::Uuid;

use super::error::{authentication, database_failure, refusal, retry_exhausted};
use crate::{
    ApiError, AppState,
    auth::{
        AccountState, CurrentActor, VerifiedAccess,
        lock::{
            OrgWriteError, OrganizationWriteScope, WriteConflict, lock_organization_write,
            retry_org_write,
        },
        validate_account,
    },
};

pub type CommandFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

/// A preview contains only values. It cannot carry a transaction or row lock.
#[derive(Debug, Clone, PartialEq)]
pub struct OrganizationWritePlan {
    scope: OrganizationWriteScope,
    fingerprint: Value,
}

impl OrganizationWritePlan {
    pub fn new(
        organizations: impl IntoIterator<Item = Uuid>,
        users: impl IntoIterator<Item = Uuid>,
        joins: impl IntoIterator<Item = Uuid>,
        fingerprint: Value,
    ) -> Self {
        Self {
            scope: OrganizationWriteScope::new(organizations, users, joins),
            fingerprint,
        }
    }

    fn with_organization(self, organization: Option<Uuid>) -> Self {
        Self {
            scope: OrganizationWriteScope::new(
                self.scope
                    .organizations()
                    .iter()
                    .copied()
                    .chain(organization),
                self.scope.users().iter().copied(),
                self.scope.join_requests().iter().copied(),
            ),
            fingerprint: self.fingerprint,
        }
    }

    fn with_actor(self, actor: Uuid) -> Self {
        Self {
            scope: OrganizationWriteScope::new(
                self.scope.organizations().iter().copied(),
                self.scope.users().iter().copied().chain([actor]),
                self.scope.join_requests().iter().copied(),
            ),
            fingerprint: self.fingerprint,
        }
    }
}

/// Implementations must include all membership/join authority in the preview
/// fingerprint. Execute receives a connection only after that preview is stable.
pub trait BusinessCommand: Send + Sync + 'static {
    type Output: Send + 'static;
    fn preview<'a>(
        &'a self,
        conn: &'a mut PgConnection,
    ) -> CommandFuture<'a, OrganizationWritePlan, AttemptError>;
    fn execute<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        actor: &'a CurrentActor,
    ) -> CommandFuture<'a, Self::Output, AttemptError>;
}

#[derive(Debug)]
pub enum AttemptError {
    Database(sqlx::Error),
    Application(ApiError),
    PreviewChanged,
}

impl From<sqlx::Error> for AttemptError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
impl From<ApiError> for AttemptError {
    fn from(error: ApiError) -> Self {
        Self::Application(error)
    }
}
impl WriteConflict for AttemptError {
    fn is_write_conflict(&self) -> bool {
        match self {
            Self::Database(error) => error.is_write_conflict(),
            Self::PreviewChanged => true,
            Self::Application(_) => false,
        }
    }
}

fn database(error: sqlx::Error) -> ApiError {
    database_failure(
        error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
    )
}

pub async fn run_organization_write<C: BusinessCommand>(
    state: &AppState,
    access: VerifiedAccess,
    command: C,
) -> Result<C::Output, ApiError> {
    run_ordered_write(
        state.pools.read.clone(),
        state.reads.clone(),
        access,
        command,
    )
    .await
}

/// The detached owner retains admission and the connection through cancellation.
/// It never retries an uncertain commit or a failed connection cleanup.
pub async fn run_ordered_write<C: BusinessCommand>(
    pool: PgPool,
    permits: Arc<Semaphore>,
    access: VerifiedAccess,
    command: C,
) -> Result<C::Output, ApiError> {
    run_owned(pool, permits, access, command, Completion::default()).await
}

async fn run_owned<C: BusinessCommand>(
    pool: PgPool,
    permits: Arc<Semaphore>,
    access: VerifiedAccess,
    command: C,
    completion: Completion,
) -> Result<C::Output, ApiError> {
    tokio::spawn(async move {
        let _permit = tokio::time::timeout(Duration::from_secs(30), permits.acquire_owned())
            .await
            .map_err(|_| database_failure(None))?
            .map_err(|_| database_failure(None))?;
        let command = Arc::new(command);
        let access = Arc::new(access);
        retry_org_write(|_| {
            attempt(
                pool.clone(),
                access.clone(),
                command.clone(),
                completion.clone(),
            )
        })
        .await
        .map_err(|error| match error {
            OrgWriteError::Busy => retry_exhausted(),
            OrgWriteError::Failed(AttemptError::Database(error)) => database(error),
            OrgWriteError::Failed(AttemptError::Application(error)) => error,
            OrgWriteError::Failed(AttemptError::PreviewChanged) => retry_exhausted(),
        })
    })
    .await
    .map_err(|_| database_failure(None))?
}

async fn attempt<C: BusinessCommand>(
    pool: PgPool,
    access: Arc<VerifiedAccess>,
    command: Arc<C>,
    completion: Completion,
) -> Result<C::Output, AttemptError> {
    let mut connection = pool.acquire().await?;
    // An unwind must not return a connection with an uncertain outcome.
    // Explicit successful paths still complete and ping before release.
    connection.close_on_drop();
    let mut preview = connection
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED READ ONLY")
        .await?;
    let plan = async {
        let actor_organization: Option<Uuid> =
            sqlx::query_scalar("SELECT organization_id FROM users WHERE id=$1")
                .bind(access.subject)
                .fetch_optional(&mut *preview)
                .await?
                .ok_or_else(|| {
                    AttemptError::Application(authentication(crate::auth::AuthError::Credentials))
                })?;
        Ok::<_, AttemptError>((command.preview(&mut preview).await?, actor_organization))
    }
    .await;
    if preview.rollback().await.is_err() || connection.ping().await.is_err() {
        let _ = connection.close().await;
        return Err(database_failure(None).into());
    }
    let (plan, actor_organization) = plan?;
    let plan = plan
        .with_actor(access.subject)
        .with_organization(actor_organization);
    let mut transaction = connection
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED READ WRITE")
        .await?;
    let result = async {
        lock_organization_write(&mut transaction, &plan.scope).await?;
        let row = sqlx::query("SELECT id,organization_id,role::text AS role,status::text AS status,email_verified,must_change_password,password_changed_at,authentication_revoked_at FROM users WHERE id=$1")
            .bind(access.subject).fetch_optional(&mut *transaction).await?
            .ok_or_else(|| AttemptError::Application(authentication(crate::auth::AuthError::Credentials)))?;
        let password: Option<chrono::DateTime<chrono::Utc>> = row.get("password_changed_at");
        let revoked: Option<chrono::DateTime<chrono::Utc>> = row.get("authentication_revoked_at");
        validate_account(&access, &AccountState {
            exists: true,
            status: row.get("status"),
            must_change_password: row.get("must_change_password"),
            password_changed_at: password.map(|time| time.timestamp_micros()),
            authentication_revoked_at: revoked.map(|time| time.timestamp_micros()),
        }).map_err(|error| AttemptError::Application(authentication(error)))?;
        let current_organization: Option<Uuid> = row.get("organization_id");
        if current_organization != actor_organization { return Err(AttemptError::PreviewChanged); }
        let current = command.preview(&mut transaction).await?.with_actor(access.subject).with_organization(current_organization);
        if current != plan { return Err(AttemptError::PreviewChanged); }
        let actor = CurrentActor { id: row.get("id"), organization_id: row.get("organization_id"), role: row.get("role"), email_verified: row.get("email_verified") };
        command.execute(&mut transaction, &actor).await
    }.await;
    match result {
        Err(error) => {
            if transaction.rollback().await.is_err() || connection.ping().await.is_err() {
                let _ = connection.close().await;
                return Err(database_failure(None).into());
            }
            connection.return_to_pool().await;
            Err(error)
        }
        Ok(value) => {
            completion.wait_at(CompletionPoint::BeforeCommit).await;
            let committed = transaction.commit().await;
            if committed.is_ok() {
                completion.wait_at(CompletionPoint::AfterCommit).await;
            }
            // The narrow test seam can remove a successful acknowledgement, but
            // it feeds the same classifier as a real SQLx completion failure.
            let committed = completion.acknowledgement(committed);
            if let Err(error) = committed {
                let _ = connection.close().await;
                return Err(commit_failure(error));
            }
            if connection.ping().await.is_err() {
                let _ = connection.close().await;
                return Err(database_failure(None).into());
            }
            // This owner conservatively closes even a clean connection. A later
            // pool-reuse optimization needs its own cleanup qualification.
            let _ = connection.close().await;
            Ok(value)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionPoint {
    BeforeCommit,
    AfterCommit,
}

#[derive(Clone, Default)]
struct Completion {
    #[cfg(feature = "contract-tests")]
    control: Option<CompletionControl>,
}

impl Completion {
    async fn wait_at(&self, point: CompletionPoint) {
        #[cfg(feature = "contract-tests")]
        if let Some(control) = &self.control {
            control.wait_at(point).await;
        }
        let _ = point;
    }
    fn acknowledgement(&self, result: Result<(), sqlx::Error>) -> Result<(), sqlx::Error> {
        #[cfg(feature = "contract-tests")]
        if result.is_ok()
            && self
                .control
                .as_ref()
                .is_some_and(|control| control.lose_ack())
        {
            return Err(sqlx::Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "completion acknowledgement unavailable",
            )));
        }
        result
    }
}

#[cfg(feature = "contract-tests")]
#[derive(Clone)]
pub struct CompletionControl {
    point: CompletionPoint,
    lost_ack: bool,
    reached: Arc<tokio::sync::Notify>,
    released: Arc<tokio::sync::Notify>,
}

#[cfg(feature = "contract-tests")]
impl CompletionControl {
    pub fn new(point: CompletionPoint, lost_ack: bool) -> Self {
        Self {
            point,
            lost_ack,
            reached: Arc::new(tokio::sync::Notify::new()),
            released: Arc::new(tokio::sync::Notify::new()),
        }
    }
    pub async fn wait_at(&self, point: CompletionPoint) {
        if point == self.point {
            self.reached.notify_one();
            self.released.notified().await;
        }
    }
    pub async fn reached(&self) {
        self.reached.notified().await;
    }
    pub fn release(&self) {
        self.released.notify_one();
    }
    pub fn lose_ack(&self) -> bool {
        self.lost_ack
    }
}

#[cfg(feature = "contract-tests")]
pub async fn run_controlled_write<C: BusinessCommand>(
    pool: PgPool,
    permits: Arc<Semaphore>,
    access: VerifiedAccess,
    command: C,
    control: CompletionControl,
) -> Result<C::Output, ApiError> {
    run_owned(
        pool,
        permits,
        access,
        command,
        Completion {
            control: Some(control),
        },
    )
    .await
}

fn commit_failure(error: sqlx::Error) -> AttemptError {
    // An ErrorResponse is a known server rejection. Preserve its SQLSTATE for
    // the existing whole-transaction retry classifier and database adapter.
    if error.as_database_error().is_some() {
        return AttemptError::Database(error);
    }
    refusal(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "COMMIT_OUTCOME_UNKNOWN",
        "The transaction outcome must be checked before another command.",
    )
    .into()
}
