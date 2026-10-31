//! Offboarded accounts on the Rust credential, admission and issuance routes.
//!
//! These tests are `#[ignore]`d so a plain `cargo test` reports them as not
//! run. `rs/read/tests/run_offboarding_admission.sh` migrates a disposable
//! database to the Alembic head, applies the runtime ACL bundle and runs them
//! with `--include-ignored`:
//!
//! * `RUST_TEST_DATABASE_URL` names that database as the runtime role, which
//!   every route under test uses;
//! * `RUST_TEST_MIGRATOR_DATABASE_URL` names it as the owner, which seeds rows.
//!
//! The database constraints make every offboarded account `REJECTED` and
//! detached. Each case therefore compares an offboarded account with an
//! ordinarily rejected, detached control account: where both are refused, the
//! two refusals differ, so the marker, not the status, decides the result.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use sqlx::{
    Connection, PgConnection, PgPool, Row,
    postgres::{PgPoolOptions, PgRow},
};
use tower::ServiceExt;
use uuid::Uuid;
use verdaxis_read::{
    AppState,
    auth::{
        self, AuthError, AuthenticatedError, AuthenticatedRequest, TokenIssuer,
        VerificationSettings,
        email::MailDispatcher,
        email_verification::{self, EmailSettings},
        limits::SharedLimits,
        password, password_reset,
        token_bytes::{generate_token, token_digest},
    },
    business::{
        admission::{
            AdmissionEvents, ApprovalMail, ApprovalMailMarker, ApproveAccount, AuditContext,
            FrozenApprovalPayload, review_queue,
        },
        transaction::{AttemptError, CommandFuture, run_organization_write},
    },
    crud::referrals::invite::MailClient,
    streams::auth::{StreamError, revalidate, verify_stream},
};

const REVISION: &str = "ob_20261031_offboarding";
const APP_ROLE: &str = "verdaxis_app_staging";
const MIGRATOR_ROLE: &str = "verdaxis_migrator_staging";
const SECRET: &str = "offboarding-admission-postgres-secret-32-chars";
const ISSUER: &str = "verdaxis";
const AUDIENCE: &str = "verdaxis-api";
const ORIGIN: &str = "https://app.verdaxis.exchange";
const ENVIRONMENT: &str = "production";
/// A password the strength rules of every route accept.
const PASSWORD: &str = "OffboardPassword123";

fn disposable_url(variable: &str, role: &str) -> String {
    let raw = std::env::var(variable).expect("explicit disposable database URL required");
    let parsed = url::Url::parse(&raw).expect("valid disposable URL");
    assert!(matches!(parsed.scheme(), "postgres" | "postgresql"));
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert!(parsed.port().is_some_and(|port| port != 5432));
    assert!(parsed.path().ends_with("_test"));
    assert_eq!(parsed.username(), role);
    assert!(parsed.query().is_none() && parsed.fragment().is_none());
    raw
}

struct World {
    /// The runtime role, as the routes use it.
    app: PgPool,
    /// The owner role, for seeding and for reading results back.
    owner: PgPool,
    state: AppState,
}

/// One seeded account.
#[derive(Clone)]
struct Account {
    id: Uuid,
    email: String,
}

impl World {
    async fn open() -> Self {
        let app_url = disposable_url("RUST_TEST_DATABASE_URL", APP_ROLE);
        let app = PgPoolOptions::new()
            .max_connections(6)
            .connect(&app_url)
            .await
            .unwrap();
        let owner = PgPoolOptions::new()
            .max_connections(3)
            .connect(&disposable_url(
                "RUST_TEST_MIGRATOR_DATABASE_URL",
                MIGRATOR_ROLE,
            ))
            .await
            .unwrap();
        let revision: String = sqlx::query_scalar("SELECT version_num FROM alembic_version")
            .fetch_one(&owner)
            .await
            .unwrap();
        assert_eq!(revision, REVISION);
        let health = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&app_url)
            .unwrap();
        let state = AppState::new(
            verdaxis_read::Config {
                kyc_execution_gate_enabled: true,
                environment: "test".into(),
                release_sha: verdaxis_read::compiled_sha().into(),
                migration_revision: REVISION.into(),
                database_url: app_url,
                db_session_time_zone: "UTC".into(),
                prepared_budget: None,
                phase2: Default::default(),
                bind: "127.0.0.1:0".parse().unwrap(),
                read_admission_slots: 4,
                pool_max: 5,
                python_budget: 20,
                configured_max_connections: 100,
                compliance_eur_usd_rate: None,
                fleet_artifact_path: "data/fleet_demand.json".into(),
            },
            verdaxis_read::runtime::db::Pools {
                read: app.clone(),
                health,
            },
            verdaxis_read::MarketPolicy {
                source_sha: verdaxis_read::compiled_sha().into(),
                products: vec![],
                points: vec![],
            },
        )
        .unwrap();
        Self { app, owner, state }
    }

    async fn organization(&self) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO organizations(id,name,type,verification_status) \
             VALUES($1,'Offboarding admission','FUEL_BUYER','APPROVED')",
        )
        .bind(id)
        .execute(&self.owner)
        .await
        .unwrap();
        id
    }

    /// An account with a usable password. `offboarded` writes the marker, which
    /// the constraints allow only on a rejected, detached account.
    async fn account(
        &self,
        role: &str,
        status: &str,
        organization: Option<Uuid>,
        email_verified: bool,
        offboarded: bool,
    ) -> Account {
        let id = Uuid::new_v4();
        let email = format!("{id}@offboarding.example");
        // An old creation time sorts these accounts first in the review queue.
        sqlx::query(
            "INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,\
             created_at,offboarded_at) \
             VALUES($1,$2,$3,$4,$5,$6,$7,'2000-01-01T00:00:00Z',CASE WHEN $8 THEN now() END)",
        )
        .bind(id)
        .bind(&email)
        .bind(password::hash(PASSWORD).await.unwrap())
        .bind(role)
        .bind(status)
        .bind(organization)
        .bind(email_verified)
        .bind(offboarded)
        .execute(&self.owner)
        .await
        .unwrap();
        Account { id, email }
    }

    /// An offboarded buyer: rejected, detached and marked.
    async fn offboarded(&self) -> Account {
        self.account("BUYER", "REJECTED", None, true, true).await
    }

    /// The control: an ordinarily rejected, detached buyer without the marker.
    async fn rejected(&self) -> Account {
        self.account("BUYER", "REJECTED", None, true, false).await
    }

    async fn user_row(&self, user: Uuid) -> PgRow {
        sqlx::query(
            "SELECT password_hash,last_login,email_verified,email_verification_token_hash,\
             password_reset_token_hash,status::text AS status,offboarded_at FROM users WHERE id=$1",
        )
        .bind(user)
        .fetch_one(&self.owner)
        .await
        .unwrap()
    }

    async fn count(&self, statement: &'static str, user: Uuid) -> i64 {
        sqlx::query_scalar(statement)
            .bind(user)
            .fetch_one(&self.owner)
            .await
            .unwrap()
    }
}

fn verification() -> VerificationSettings {
    VerificationSettings::new(SECRET.into(), None, ISSUER.into(), AUDIENCE.into())
}

fn issuer() -> TokenIssuer {
    TokenIssuer::new(SECRET, ISSUER, AUDIENCE)
}

fn access(user: Uuid) -> auth::VerifiedAccess {
    let token = issuer().access(user, Utc::now(), None);
    auth::verify_access(&token, &verification(), Utc::now()).unwrap()
}

fn code(body: &Value) -> &str {
    body["detail"]["code"].as_str().unwrap_or_default()
}

async fn call(router: axum::Router, request: Request<Body>) -> (u16, Value) {
    let response = router.oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 65_536).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn login(world: &World, email: &str) -> (u16, Value) {
    let state = auth::login::LoginState::new(
        world.app.clone(),
        ENVIRONMENT,
        SECRET,
        None,
        ISSUER,
        AUDIENCE,
        SharedLimits::default(),
    )
    .await;
    call(
        auth::login::router(state),
        Request::post("/api/auth/login")
            .header("origin", ORIGIN)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", format!("device_session={}", generate_token()))
            .body(Body::from(format!(
                "username={}&password={PASSWORD}",
                email.replace('@', "%40")
            )))
            .unwrap(),
    )
    .await
}

/// One live refresh session on a fresh device, and the token that presents it.
struct Session {
    family: Uuid,
    device: String,
    token: String,
}

async fn session(world: &World, user: Uuid) -> Session {
    let device = generate_token();
    let jti = generate_token();
    let family = Uuid::new_v4();
    let now = Utc::now();
    auth::store::insert_refresh(
        &mut world.owner.acquire().await.unwrap(),
        &auth::store::NewRefreshRow {
            user_id: user,
            family_id: family,
            jti_hash: token_digest(&jti),
            device_id_hash: token_digest(&device),
            expires_at: now + Duration::days(7),
        },
    )
    .await
    .unwrap();
    Session {
        family,
        device,
        token: issuer().refresh(user, family, &jti, now - Duration::seconds(1), None),
    }
}

async fn refresh(world: &World, session: &Session) -> (u16, Value) {
    let state = auth::refresh::RefreshState::new(
        world.app.clone(),
        ENVIRONMENT,
        SECRET,
        None,
        ISSUER,
        AUDIENCE,
        SharedLimits::default(),
    );
    call(
        auth::refresh::router(state),
        Request::post("/api/auth/refresh")
            .header("origin", ORIGIN)
            .header(
                "cookie",
                format!(
                    "refresh_token={}; device_session={}",
                    session.token, session.device
                ),
            )
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn live_sessions(world: &World, family: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM refresh_sessions WHERE family_id=$1 AND NOT revoked")
        .bind(family)
        .fetch_one(&world.owner)
        .await
        .unwrap()
}

/// Run the stream-token issuance step through the shared bearer admission, as
/// the composed route does. Returns the admission result and whether the
/// issuance work ran.
async fn stream_token(
    world: &World,
    access: auth::VerifiedAccess,
) -> (Result<String, AuthenticatedError>, bool) {
    let ran = Arc::new(AtomicBool::new(false));
    let flag = ran.clone();
    let tokens = auth::stream::StreamTokens::new(issuer(), ENVIRONMENT, SharedLimits::default());
    let result = auth::run_authenticated(
        &world.state,
        access,
        AuthenticatedRequest::new("GET", "/api/auth/stream-token", false),
        move |transaction| {
            Box::pin(async move {
                flag.store(true, Ordering::SeqCst);
                tokens
                    .issue(transaction.actor(), "127.0.0.1", Utc::now())
                    .map(|response| response.stream_token)
            })
        },
    )
    .await;
    (result, ran.load(Ordering::SeqCst))
}

fn authentication(result: &Result<String, AuthenticatedError>) -> Option<AuthError> {
    match result {
        Err(AuthenticatedError::Authentication(error)) => Some(*error),
        _ => None,
    }
}

fn email_settings(world: &World) -> EmailSettings {
    EmailSettings::new(world.app.clone(), MailDispatcher::new(MailClient::disabled()))
}

async fn accept_invitation(world: &World, token: &str) -> (u16, Value) {
    let state = auth::invitation_accept::InvitationAcceptState::new(
        world.app.clone(),
        ENVIRONMENT,
        SECRET,
        ISSUER,
        AUDIENCE,
        "https://app.verdaxis.exchange",
        SharedLimits::default(),
    );
    call(
        auth::invitation_accept::router(state),
        Request::post("/api/auth/invitations/accept")
            .header("origin", ORIGIN)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"token": token, "new_password": PASSWORD, "accept_terms": true})
                    .to_string(),
            ))
            .unwrap(),
    )
    .await
}

async fn reset_token(world: &World, user: Uuid) -> String {
    let token = generate_token();
    sqlx::query(
        "INSERT INTO password_reset_tokens (user_id,token_hash,expires_at,created_at) \
         VALUES ($1,$2,now() + interval '1 hour',now())",
    )
    .bind(user)
    .bind(token_digest(&token))
    .execute(&world.owner)
    .await
    .unwrap();
    token
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn marked_login_refresh_and_issuance_denied() {
    let world = World::open().await;
    let marked = world.offboarded().await;
    let control = world.rejected().await;

    // Login: the offboarded account answers exactly as an unknown address and
    // writes nothing; the control is refused as inactive.
    let unknown = login(&world, &format!("{}@offboarding.example", Uuid::new_v4())).await;
    assert_eq!(unknown.0, 401);
    assert_eq!(code(&unknown.1), "AUTH_CREDENTIALS_INVALID");
    assert_eq!(login(&world, &marked.email).await, unknown);
    let (status, body) = login(&world, &control.email).await;
    assert_eq!((status, code(&body)), (403, "AUTH_ACCOUNT_NOT_ACTIVE"));
    assert!(
        world
            .user_row(marked.id)
            .await
            .get::<Option<DateTime<Utc>>, _>("last_login")
            .is_none()
    );
    for statement in [
        "SELECT count(*) FROM refresh_sessions WHERE user_id=$1",
        "SELECT count(*) FROM user_login_days WHERE user_id=$1",
    ] {
        assert_eq!(world.count(statement, marked.id).await, 0, "{statement}");
    }

    // Refresh: refused as an unknown session, and the session is not touched.
    let marked_session = session(&world, marked.id).await;
    let (status, body) = refresh(&world, &marked_session).await;
    assert_eq!((status, code(&body)), (401, "REFRESH_SESSION_REVOKED"));
    assert_eq!(live_sessions(&world, marked_session.family).await, 1);
    let control_session = session(&world, control.id).await;
    let (status, body) = refresh(&world, &control_session).await;
    assert_eq!((status, code(&body)), (401, "REFRESH_ACCOUNT_INACTIVE"));

    // Stream-token issuance: the shared admission refuses before any work.
    let (result, ran) = stream_token(&world, access(marked.id)).await;
    assert_eq!(authentication(&result), Some(AuthError::SessionRevoked));
    assert!(!ran);
    let (result, ran) = stream_token(&world, access(control.id)).await;
    assert_eq!(authentication(&result), Some(AuthError::Rejected));
    assert!(!ran);

    // Recovery: no reset link is issued, and an outstanding one is refused
    // without being used. The control gets a link and can use it.
    let settings = email_settings(&world);
    let issued = "SELECT count(*) FROM password_reset_tokens WHERE user_id=$1";
    let marked_reply = password_reset::forgot(&settings, &marked.email)
        .await
        .unwrap();
    assert_eq!(world.count(issued, marked.id).await, 0);
    let control_reply = password_reset::forgot(&settings, &control.email)
        .await
        .unwrap();
    assert_eq!(marked_reply, control_reply);
    assert_eq!(world.count(issued, control.id).await, 1);
    let before = world.user_row(marked.id).await.get::<String, _>("password_hash");
    let token = reset_token(&world, marked.id).await;
    let error = password_reset::reset(&settings, &token, "ReplacedPassword456")
        .await
        .unwrap_err();
    assert_eq!(error.status.as_u16(), 400);
    assert_eq!(code(&json!({"detail": error.detail})), "RESET_TOKEN_INVALID");
    assert_eq!(
        world.user_row(marked.id).await.get::<String, _>("password_hash"),
        before
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM password_reset_tokens WHERE user_id=$1 AND used_at IS NULL",
                marked.id,
            )
            .await,
        1
    );
    let control_token = reset_token_replacing(&world, control.id).await;
    password_reset::reset(&settings, &control_token, "ReplacedPassword456")
        .await
        .unwrap();

    // Email verification: an unverified offboarded account neither verifies
    // nor receives a new link; the unverified control verifies.
    let unverified = world
        .account("BUYER", "REJECTED", None, false, true)
        .await;
    let unverified_control = world
        .account("BUYER", "REJECTED", None, false, false)
        .await;
    for account in [&unverified, &unverified_control] {
        sqlx::query(
            "UPDATE users SET email_verification_token_hash=$2,\
             email_verification_token_expires_at=now() + interval '1 hour' WHERE id=$1",
        )
        .bind(account.id)
        .bind(token_digest(&account.id.to_string()))
        .execute(&world.owner)
        .await
        .unwrap();
    }
    let error = email_verification::verify(&settings, &unverified.id.to_string())
        .await
        .unwrap_err();
    assert_eq!(code(&json!({"detail": error.detail})), "EMAIL_TOKEN_INVALID");
    let row = world.user_row(unverified.id).await;
    assert!(!row.get::<bool, _>("email_verified"));
    email_verification::verify(&settings, &unverified_control.id.to_string())
        .await
        .unwrap();
    email_verification::resend(&settings, &unverified.email, false)
        .await
        .unwrap();
    assert_eq!(
        world
            .user_row(unverified.id)
            .await
            .get::<Option<String>, _>("email_verification_token_hash"),
        Some(token_digest(&unverified.id.to_string()))
    );

    // Invitation acceptance: an outstanding invitation secret never issues a
    // session or sets a password.
    let invitation = generate_token();
    sqlx::query(
        "UPDATE users SET password_reset_token_hash=$2,\
         password_reset_expires=now() + interval '1 hour',must_change_password=true WHERE id=$1",
    )
    .bind(marked.id)
    .bind(token_digest(&invitation))
    .execute(&world.owner)
    .await
    .unwrap();
    let before = world.user_row(marked.id).await.get::<String, _>("password_hash");
    let (status, body) = accept_invitation(&world, &invitation).await;
    assert_eq!((status, code(&body)), (400, "INVITATION_INVALID"));
    let row = world.user_row(marked.id).await;
    assert_eq!(row.get::<String, _>("password_hash"), before);
    assert_eq!(
        row.get::<Option<String>, _>("password_reset_token_hash"),
        Some(token_digest(&invitation))
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM refresh_sessions WHERE user_id=$1 AND NOT revoked",
                marked.id
            )
            .await,
        1
    );
}

/// A second reset link for the control replaces the one `forgot` issued.
async fn reset_token_replacing(world: &World, user: Uuid) -> String {
    sqlx::query("DELETE FROM password_reset_tokens WHERE user_id=$1")
        .bind(user)
        .execute(&world.owner)
        .await
        .unwrap();
    reset_token(world, user).await
}

/// A rotated refresh token presented again is a replay: for an admitted
/// account the route revokes the family and commits. For an offboarded
/// account it refuses before the replay rule, so nothing is written and no
/// replay result is returned. A stream principal is refused before any
/// retained event is read.
#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn marked_account_denied_before_receipt_replay() {
    let world = World::open().await;
    let organization = world.organization().await;
    let admitted = world
        .account("BUYER", "APPROVED", Some(organization), true, false)
        .await;
    let marked = world.offboarded().await;
    for (account, expected, live_after) in [
        (&admitted, "REFRESH_TOKEN_REPLAYED", 0),
        (&marked, "REFRESH_SESSION_REVOKED", 1),
    ] {
        let rotated = session(&world, account.id).await;
        // The successor row is live; the presented row is already rotated and
        // its grace period has ended.
        let successor = generate_token();
        sqlx::query(
            "INSERT INTO refresh_sessions (id,user_id,family_id,jti_hash,device_id_hash,expires_at) \
             VALUES ($1,$2,$3,$4,$5,now() + interval '7 days')",
        )
        .bind(Uuid::new_v4())
        .bind(account.id)
        .bind(rotated.family)
        .bind(token_digest(&successor))
        .bind(token_digest(&rotated.device))
        .execute(&world.owner)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE refresh_sessions SET revoked=true,replaced_by_jti_hash=$2,\
             rotation_grace_until=now() - interval '1 minute' \
             WHERE family_id=$1 AND jti_hash<>$2",
        )
        .bind(rotated.family)
        .bind(token_digest(&successor))
        .execute(&world.owner)
        .await
        .unwrap();
        let (status, body) = refresh(&world, &rotated).await;
        assert_eq!((status, code(&body)), (401, expected));
        assert_eq!(live_sessions(&world, rotated.family).await, live_after);
    }

    // A stream token issued while the account was a member: the membership
    // and the marker refuse it before any replay.
    let token = issuer().stream(marked.id, organization, "test", Utc::now());
    let principal = verify_stream(&token, &verification(), "test", Utc::now()).unwrap();
    let mut connection = world.app.acquire().await.unwrap();
    assert_eq!(
        revalidate(&mut connection, &principal).await,
        Err(StreamError::TokenInvalid)
    );
}

#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn marked_user_not_in_review_queue() {
    let world = World::open().await;
    let marked = world.offboarded().await;
    let control = world.rejected().await;
    let mut connection = world.app.acquire().await.unwrap();
    let listed = review_queue::list(&mut connection, review_queue::ReviewQuery::new(100).unwrap())
        .await
        .unwrap();
    let ids: Vec<&str> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["user_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&control.id.to_string().as_str()));
    assert!(!ids.contains(&marked.id.to_string().as_str()));
    review_queue::detail(&mut connection, control.id)
        .await
        .unwrap();
    let error = review_queue::detail(&mut connection, marked.id)
        .await
        .unwrap_err();
    assert_eq!(error.status.as_u16(), 404);
    assert_eq!(error.detail["code"], "REVIEW_CASE_NOT_FOUND");
}

struct NoEvents;
impl AdmissionEvents for NoEvents {
    fn visible<'a>(&'a self, _: &'a mut PgConnection, _: Uuid) -> CommandFuture<'a, bool, AttemptError> {
        Box::pin(async { Ok(false) })
    }
    fn append<'a>(
        &'a self,
        _: &'a mut PgConnection,
        _: Uuid,
        _: Uuid,
        _: bool,
    ) -> CommandFuture<'a, (), AttemptError> {
        Box::pin(async { Ok(()) })
    }
}

struct NoMail;
impl ApprovalMail for NoMail {
    fn freeze_approval_mail<'a>(
        &'a self,
        _: &'a mut PgConnection,
        _: &'a ApprovalMailMarker,
    ) -> CommandFuture<'a, (), AttemptError> {
        Box::pin(async { Ok(()) })
    }
}

fn approve(user: Uuid) -> ApproveAccount<NoMail, NoEvents> {
    ApproveAccount {
        user_id: user,
        now: Utc::now(),
        payload: FrozenApprovalPayload {
            from: "team@offboarding.example".into(),
            to: ["recipient@offboarding.example".into()],
            subject: "Approved".into(),
            html: "<p>Approved</p>".into(),
            reply_to: None,
        },
        audit: AuditContext::default(),
        mail: NoMail,
        events: NoEvents,
    }
}

/// Approval refuses an offboarded account before any write, and an offboarded
/// actor cannot run an organization write.
#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn marked_account_approval_refused() {
    let world = World::open().await;
    let administrator = world.account("ADMIN", "APPROVED", None, true, false).await;
    let marked = world.offboarded().await;
    let transitions = "SELECT count(*) FROM user_status_transitions WHERE user_id=$1";
    let error = run_organization_write(&world.state, access(administrator.id), approve(marked.id))
        .await
        .unwrap_err();
    assert_eq!(error.status.as_u16(), 409);
    assert_eq!(error.detail["code"], "ACCOUNT_OFFBOARDED");
    let row = world.user_row(marked.id).await;
    assert_eq!(row.get::<String, _>("status"), "REJECTED");
    assert!(row.get::<Option<DateTime<Utc>>, _>("offboarded_at").is_some());
    assert_eq!(world.count(transitions, marked.id).await, 0);

    // The actor projection of the organization write reads the marker too.
    let target = world.rejected().await;
    let offboarded_administrator = world.account("ADMIN", "REJECTED", None, true, true).await;
    let rejected_administrator = world.account("ADMIN", "REJECTED", None, true, false).await;
    for (actor, status, expected) in [
        (&offboarded_administrator, 401, "AUTH_SESSION_REVOKED"),
        (&rejected_administrator, 403, "ACCOUNT_REJECTED"),
    ] {
        let error = run_organization_write(&world.state, access(actor.id), approve(target.id))
            .await
            .unwrap_err();
        assert_eq!(error.status.as_u16(), status);
        assert_eq!(error.detail["code"], expected);
    }
    assert_eq!(world.count(transitions, target.id).await, 0);
}

/// An access token with exactly these issuance claims.
fn shaped_token(user: Uuid, iat: i64, iat_us: Value) -> auth::VerifiedAccess {
    let now = Utc::now();
    let token = encode(
        &Header::new(Algorithm::HS256),
        &json!({
            "sub": user.to_string(),
            "type": "access",
            "iat": iat,
            "iat_us": iat_us,
            "exp": (now + Duration::minutes(15)).timestamp(),
            "iss": ISSUER,
            "aud": AUDIENCE,
        }),
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap();
    auth::verify_access(&token, &verification(), now).unwrap()
}

/// Tokens before, equal to and after a stored cutoff, and one with an
/// issuance time outside the representable range, against the shared bearer
/// admission. The control's token after the cutoff reaches the status rule;
/// every token of the offboarded account is the revoked-session refusal.
#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn marked_token_shapes_fail_closed() {
    let world = World::open().await;
    let marked = world.offboarded().await;
    let control = world.rejected().await;
    let cutoff: DateTime<Utc> = sqlx::query_scalar("SELECT date_trunc('second', now()) - interval '1 minute'")
        .fetch_one(&world.owner)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET authentication_revoked_at=$2 WHERE id=ANY($1)")
        .bind(vec![marked.id, control.id])
        .bind(cutoff)
        .execute(&world.owner)
        .await
        .unwrap();
    let shapes = |user: Uuid| {
        let at = |seconds: i64| cutoff + Duration::seconds(seconds);
        [
            ("before", shaped_token(user, at(-5).timestamp(), json!(at(-5).timestamp_micros()))),
            ("equal", shaped_token(user, at(0).timestamp(), json!(at(0).timestamp_micros()))),
            ("after", shaped_token(user, at(30).timestamp(), json!(at(30).timestamp_micros()))),
            ("out of range", shaped_token(user, at(30).timestamp(), json!(u64::MAX))),
        ]
    };
    for (shape, token) in shapes(marked.id) {
        let (result, ran) = stream_token(&world, token).await;
        assert_eq!(authentication(&result), Some(AuthError::SessionRevoked), "{shape}");
        assert!(!ran, "{shape}");
    }
    for (shape, token) in shapes(control.id) {
        let expected = if shape == "after" {
            AuthError::Rejected
        } else {
            AuthError::SessionRevoked
        };
        let (result, _) = stream_token(&world, token).await;
        assert_eq!(authentication(&result), Some(expected), "{shape}");
    }
}

/// An offboarding that commits while a mutation waits for the user row wins:
/// the mutation re-reads the row after the wait and refuses before its work.
#[tokio::test]
#[ignore = "requires the migrated disposable PostgreSQL harness"]
async fn offboarding_commit_wins_over_waiting_mutation() {
    let world = World::open().await;
    let organization = world.organization().await;
    let member = world
        .account("BUYER", "APPROVED", Some(organization), true, false)
        .await;
    let mut offboarding = PgConnection::connect(&disposable_url(
        "RUST_TEST_MIGRATOR_DATABASE_URL",
        MIGRATOR_ROLE,
    ))
    .await
    .unwrap();
    let mut transaction = offboarding.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(member.id)
        .execute(&mut *transaction)
        .await
        .unwrap();
    let member_id = member.id;
    let waiting = {
        let state = world.state.clone();
        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        let handle = tokio::spawn(async move {
            auth::run_authenticated(
                &state,
                access(member_id),
                AuthenticatedRequest::new("PUT", "/api/users/me/preferences", false),
                move |_| {
                    Box::pin(async move {
                        flag.store(true, Ordering::SeqCst);
                        Ok(())
                    })
                },
            )
            .await
        });
        (handle, ran)
    };
    // Wait until the mutation is blocked on the user row lock.
    let mut blocked = false;
    for _ in 0..100 {
        let waiting_backends: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() \
             AND usename=current_user AND wait_event_type='Lock'",
        )
        .fetch_one(&world.app)
        .await
        .unwrap();
        if waiting_backends > 0 {
            blocked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(blocked, "the mutation never waited for the user row");
    sqlx::query(
        "UPDATE users SET status='REJECTED',organization_id=NULL,offboarded_at=now() WHERE id=$1",
    )
    .bind(member.id)
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    let (handle, ran) = waiting;
    let result = handle.await.unwrap();
    assert!(matches!(
        result,
        Err(AuthenticatedError::Authentication(AuthError::SessionRevoked))
    ));
    assert!(!ran.load(Ordering::SeqCst));
}
