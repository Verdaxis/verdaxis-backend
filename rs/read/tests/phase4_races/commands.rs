//! The business commands the races drive, their event and mail doubles, the
//! fault injection, and the stand-ins for the session writers that live on the
//! Phase 3 branch and are not yet part of this one.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;
use verdaxis_read::{
    auth::{
        AccountState, TokenIssuance, cutoff_refusal,
        store::{self, NewRefreshRow, RefreshRow, UserRow},
    },
    business::{
        admission::{
            AccountRejection, AdmissionEvents, ApprovalMail, ApprovalMailMarker, ApproveAccount,
            AuditContext, FrozenApprovalPayload, RejectAccount, RejectionRoute,
        },
        approval_mail::{self, MailError},
        invitations::{
            request::{CreateInvitation, InvitationAudit, OrganizationDomainPolicy},
            store::CreateInvitationCommand,
        },
        joins::{ReviewJoin, ReviewNote},
        organizations::ReviewOrganization,
        transaction::{AttemptError, CommandFuture},
    },
    kyc::{
        keyring::KeyRing,
        review::{Decision, ReviewCommand},
        submission::{SubmitCommand, UploadLimits, parse_multipart},
    },
    subscriptions::{
        routes::{Operation, SubscriptionCommand},
        store::SubscriptionUpdate,
    },
};

use super::world::{DEVICE_HASH, World, backend_pid};

/// The admission event producer double: one outbox row per event, naming the
/// user, the organization and whether the event is public.
pub struct Events;

impl AdmissionEvents for Events {
    fn visible<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user: Uuid,
    ) -> CommandFuture<'a, bool, AttemptError> {
        Box::pin(async move {
            Ok(
                sqlx::query_scalar("SELECT kyc_status = 'APPROVED' FROM users WHERE id=$1")
                    .bind(user)
                    .fetch_one(conn)
                    .await?,
            )
        })
    }

    fn append<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user: Uuid,
        organization: Uuid,
        public: bool,
    ) -> CommandFuture<'a, (), AttemptError> {
        Box::pin(async move {
            sqlx::query("INSERT INTO market_event_outbox(id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload,created_at,delivery_attempts) VALUES($1,'account_admission_changed','user',$2,$3,$4,now(),0)")
                .bind(Uuid::new_v4())
                .bind(user.to_string())
                .bind(json!([organization]))
                .bind(json!({"public": public}))
                .execute(conn)
                .await?;
            Ok(())
        })
    }
}

/// Freezes the approval-mail marker with the production helper, as the route
/// composition will.
pub struct FrozenMail;

impl ApprovalMail for FrozenMail {
    fn freeze_approval_mail<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        marker: &'a ApprovalMailMarker,
    ) -> CommandFuture<'a, (), AttemptError> {
        Box::pin(async move {
            let frozen = approval_mail::ApprovalMailMarker {
                user_id: marker.user_id,
                transition_id: marker.transition_id,
                payload: approval_mail::FrozenApprovalPayload {
                    from: marker.payload.from.clone(),
                    to: marker.payload.to.clone(),
                    subject: marker.payload.subject.clone(),
                    html: marker.payload.html.clone(),
                    reply_to: marker.payload.reply_to.clone(),
                },
                retry_at: marker.retry_at,
            };
            match approval_mail::freeze_approval_mail(conn, &frozen).await {
                Ok(()) => Ok(()),
                Err(MailError::Database(error)) => Err(AttemptError::Database(error)),
                Err(other) => panic!("the marker is always freezable: {other:?}"),
            }
        })
    }
}

fn payload() -> FrozenApprovalPayload {
    FrozenApprovalPayload {
        from: "team@example.test".into(),
        to: ["member@example.test".into()],
        subject: "Account approved".into(),
        html: "<p>Approved</p>".into(),
        reply_to: None,
    }
}

pub fn approve_account(user: Uuid, now: DateTime<Utc>) -> ApproveAccount<FrozenMail, Events> {
    ApproveAccount {
        user_id: user,
        now,
        payload: payload(),
        audit: AuditContext::default(),
        mail: FrozenMail,
        events: Events,
    }
}

pub fn reject_account(
    user: Uuid,
    route: RejectionRoute,
    now: DateTime<Utc>,
) -> AccountRejection<Events> {
    AccountRejection {
        command: RejectAccount {
            user_id: user,
            now,
            reason: "review".into(),
            events: Events,
        },
        route,
        reason: Some("review".into()),
        audit: AuditContext::default(),
    }
}

pub fn review_join(join: Uuid, approve: bool) -> ReviewJoin {
    ReviewJoin {
        join_request_id: join,
        approve,
        note: ReviewNote::parse("reviewed").expect("a valid review note"),
        now: Utc::now(),
    }
}

pub fn review_organization(world: &World, organization: Uuid, approve: bool) -> ReviewOrganization {
    ReviewOrganization {
        organization_id: organization,
        approve,
        reason: (!approve).then(|| "review".into()),
        audit: Default::default(),
        now: Utc::now(),
        policy: Arc::clone(&world.policy),
        config: Arc::clone(&world.config),
    }
}

pub fn review_kyc(user: Uuid, approve: bool) -> ReviewCommand {
    let decision = if approve {
        Decision::approval(json!({
            "external_evidence_reference": "evidence-reference",
            "review_note": "the evidence was checked",
        }))
    } else {
        Decision::rejection(json!({"reason": "the evidence is unusable"}))
    }
    .expect("a valid review body");
    ReviewCommand {
        user_id: user,
        decision,
        now: Utc::now(),
        client: Some("127.0.0.1".into()),
        request_id: Some("race".into()),
        events: Arc::new(Events),
    }
}

/// A KYC submission of two small documents, parsed against `organization`.
pub async fn submit_kyc(user: Uuid, organization: Uuid) -> SubmitCommand {
    let mut body = Vec::new();
    for (name, content) in [
        ("passport", b"%PDF-race" as &[u8]),
        ("company_doc", b"%PDF-race"),
    ] {
        body.extend_from_slice(
            format!(
                "--race-boundary\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"r.pdf\"\r\nContent-Type: application/pdf\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b"--race-boundary--\r\n");
    let upload = parse_multipart(
        axum::body::Body::from(body),
        "multipart/form-data; boundary=race-boundary",
        UploadLimits::default(),
    )
    .await
    .expect("a valid upload");
    // A throwaway key for the suite's own ciphertext; no deployment key exists here.
    let keys = KeyRing::parse(
        br#"{"active":"a","keys":{"a":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}}"#,
    )
    .expect("a valid key ring");
    SubmitCommand::new(user, organization, upload, Arc::new(keys), Utc::now())
}

pub fn subscription_bootstrap(user: Uuid) -> SubscriptionCommand {
    SubscriptionCommand::new(user, Operation::Mine, Utc::now())
}

/// The administrator's negotiated enterprise tier with an explicit fee.
pub fn subscription_negotiation(admin: Uuid, organization: Uuid) -> SubscriptionCommand {
    let update = SubscriptionUpdate::parse(json!({
        "tier": "enterprise",
        "seller_fee_per_mt_usd": "0.75",
    }))
    .expect("a valid subscription update");
    SubscriptionCommand::new(admin, Operation::Update(organization, update), Utc::now())
}

struct NoDomains;
impl OrganizationDomainPolicy for NoDomains {
    fn organization_domain(&self, _email: &str) -> Option<String> {
        None
    }
}

/// Create or rotate the invitation of one supplier address in an existing
/// organization.
pub fn invite_supplier(email: &str, organization: Uuid) -> CreateInvitationCommand {
    let invitation = CreateInvitation::parse(&json!({
        "email": email,
        "first_name": "Invited",
        "last_name": "Supplier",
        "role": "SUPPLIER",
        "organization_id": organization,
    }))
    .expect("a valid invitation");
    CreateInvitationCommand {
        invitation,
        now: Utc::now(),
        audit: InvitationAudit {
            ip_address: Some("127.0.0.1".into()),
            request_id: Some("race".into()),
        },
        domains: Arc::new(NoDomains),
        frontend_url: "https://app.example".into(),
    }
}

// ---------------------------------------------------------------------------
// Fault injection.
// ---------------------------------------------------------------------------

/// A trigger that fails inserts into one table with a chosen SQLSTATE. The
/// failure is a real PostgreSQL error raised inside the command's transaction.
pub struct Fault {
    name: String,
    table: &'static str,
}

impl Fault {
    /// Fail the first `failures` inserts, then let every later insert through.
    pub async fn first(
        admin: &mut PgConnection,
        table: &'static str,
        sqlstate: &str,
        failures: i64,
    ) -> Self {
        let name = format!("fault_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SEQUENCE {name}; \
             GRANT USAGE ON SEQUENCE {name} TO rust_app; \
             CREATE FUNCTION {name}() RETURNS trigger LANGUAGE plpgsql AS $$ \
             BEGIN \
               IF nextval('{name}') <= {failures} THEN \
                 RAISE EXCEPTION 'injected failure' USING ERRCODE = '{sqlstate}'; \
               END IF; \
               RETURN NEW; \
             END $$; \
             CREATE TRIGGER {name} BEFORE INSERT ON {table} \
               FOR EACH ROW EXECUTE FUNCTION {name}()"
        )))
        .execute(admin)
        .await
        .expect("install the fault");
        Self { name, table }
    }

    /// Fail every insert.
    pub async fn always(admin: &mut PgConnection, table: &'static str, sqlstate: &str) -> Self {
        Self::first(admin, table, sqlstate, i64::MAX).await
    }

    /// How many inserts reached the trigger.
    pub async fn hits(&self, admin: &mut PgConnection) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT CASE WHEN is_called THEN last_value ELSE 0 END FROM {}",
            self.name
        )))
        .fetch_one(admin)
        .await
        .expect("read the fault counter")
    }

    pub async fn remove(self, admin: &mut PgConnection) {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP TRIGGER {name} ON {table}; DROP FUNCTION {name}()",
            name = self.name,
            table = self.table
        )))
        .execute(admin)
        .await
        .expect("remove the fault");
    }
}

// ---------------------------------------------------------------------------
// Session writers of the Phase 3 branch.
//
// Refresh rotation and invitation acceptance are routes of that branch. These
// stand-ins take the same locks in the same order and run the same checks and
// writes through the same shared store primitives (`auth::store`,
// `cutoff_refusal`), so a race against them exercises the shared protocol. The
// races are repeated against the real routes when that branch is integrated.
// ---------------------------------------------------------------------------

/// A refresh rotation paused after its locks: device, user, presented session.
pub struct RotationInFlight {
    transaction: Transaction<'static, Postgres>,
    user: UserRow,
    session: RefreshRow,
    pub backend: i32,
}

pub async fn lock_for_rotation(pool: &PgPool, user: Uuid, jti_hash: &str) -> RotationInFlight {
    let mut transaction = pool.begin().await.expect("begin the rotation");
    store::lock_device(&mut transaction, DEVICE_HASH)
        .await
        .expect("lock the device");
    let user = store::lock_user(&mut transaction, user)
        .await
        .expect("lock the user")
        .expect("the user exists");
    let session = store::lock_refresh_by_jti(&mut transaction, jti_hash)
        .await
        .expect("lock the session")
        .expect("the session exists");
    let backend = backend_pid(&mut transaction).await;
    RotationInFlight {
        transaction,
        user,
        session,
        backend,
    }
}

impl RotationInFlight {
    /// Issue the successor session when the locked account still allows it, and
    /// commit. Returns the successor's token digest, or `None` after a refusal.
    pub async fn finish(mut self, session_issued_at: DateTime<Utc>) -> Option<String> {
        let account = AccountState {
            exists: true,
            status: self.user.status.clone(),
            must_change_password: self.user.must_change_password,
            password_changed_at: self
                .user
                .password_changed_at
                .map(|at| at.timestamp_micros()),
            authentication_revoked_at: self
                .user
                .authentication_revoked_at
                .map(|at| at.timestamp_micros()),
        };
        let issuance = TokenIssuance {
            issued_at: Some(session_issued_at),
            out_of_range: false,
        };
        let refused = cutoff_refusal(issuance, &account).is_some()
            || self.user.status != "APPROVED"
            || self.session.revoked;
        let successor = (!refused).then(|| {
            format!(
                "{:032x}{:032x}",
                Uuid::new_v4().as_u128(),
                Uuid::new_v4().as_u128()
            )
        });
        if let Some(jti_hash) = &successor {
            store::insert_refresh(
                &mut self.transaction,
                &NewRefreshRow {
                    user_id: self.user.id,
                    family_id: self.session.family_id,
                    jti_hash: jti_hash.clone(),
                    device_id_hash: DEVICE_HASH.into(),
                    expires_at: Utc::now() + chrono::Duration::days(7),
                },
            )
            .await
            .expect("insert the successor session");
        }
        self.transaction
            .commit()
            .await
            .expect("commit the rotation");
        successor
    }
}

/// An invitation acceptance paused after it locked the invited user's row.
pub struct AcceptanceInFlight {
    transaction: Transaction<'static, Postgres>,
    user: Uuid,
    pub backend: i32,
}

/// Lock the user whose invitation digest equals `digest`, as the acceptance
/// route does: device lock first, then the user row found by the digest. A
/// digest that no longer matches finds nothing and locks nothing.
pub async fn lock_for_acceptance(pool: &PgPool, digest: &str) -> Option<AcceptanceInFlight> {
    let mut transaction = pool.begin().await.expect("begin the acceptance");
    store::lock_device(&mut transaction, DEVICE_HASH)
        .await
        .expect("lock the device");
    let backend = backend_pid(&mut transaction).await;
    let user: Option<Uuid> = sqlx::query_scalar(
        "SELECT u.id FROM users u WHERE u.password_reset_token_hash = $1 \
         AND u.password_reset_expires > $2 FOR UPDATE OF u",
    )
    .bind(digest)
    .bind(Utc::now())
    .fetch_optional(&mut *transaction)
    .await
    .expect("look up the invitation");
    match user {
        Some(user) => Some(AcceptanceInFlight {
            transaction,
            user,
            backend,
        }),
        None => {
            transaction.rollback().await.expect("release the device");
            None
        }
    }
}

impl AcceptanceInFlight {
    /// Spend the invitation: revoke older sessions, clear the digest, verify the
    /// account, issue the first session and append the audit row.
    pub async fn consume(mut self) {
        let now = Utc::now();
        store::revoke_user_authentication(&mut self.transaction, self.user, now)
            .await
            .expect("advance the cutoff");
        sqlx::query("UPDATE users SET password_reset_token_hash=NULL,password_reset_expires=NULL,must_change_password=false,email_verified=true,last_login=$2 WHERE id=$1")
            .bind(self.user)
            .bind(now)
            .execute(&mut *self.transaction)
            .await
            .expect("clear the invitation");
        store::insert_refresh(
            &mut self.transaction,
            &NewRefreshRow {
                user_id: self.user,
                family_id: Uuid::new_v4(),
                jti_hash: format!(
                    "{:032x}{:032x}",
                    Uuid::new_v4().as_u128(),
                    Uuid::new_v4().as_u128()
                ),
                device_id_hash: DEVICE_HASH.into(),
                expires_at: now + chrono::Duration::days(7),
            },
        )
        .await
        .expect("issue the first session");
        sqlx::query("INSERT INTO audit_logs(id,user_id,action,resource_type,resource_id,timestamp) VALUES($1,$2,'user.invitation_accepted','user',$3,$4)")
            .bind(Uuid::new_v4())
            .bind(self.user)
            .bind(self.user.to_string())
            .bind(now)
            .execute(&mut *self.transaction)
            .await
            .expect("append the audit row");
        self.transaction
            .commit()
            .await
            .expect("commit the acceptance");
    }
}
