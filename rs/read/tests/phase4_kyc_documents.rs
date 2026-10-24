use axum::{body::Body, http::StatusCode};
use futures_util::stream;
use uuid::Uuid;
use verdaxis_read::{
    auth::limits::{BusinessLimitKeys, SharedLimits},
    kyc::{
        documents,
        submission::{Declarations, UploadLimits, parse_multipart},
    },
};

fn payload(a: &[u8], mime: &str, declaration: Option<&str>) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, content) in [
        ("passport", a),
        ("company_doc", b"%PDF-synthetic".as_slice()),
    ] {
        body.extend_from_slice(format!("--fixture\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"unused\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    if let Some(value) = declaration {
        body.extend_from_slice(format!("--fixture\r\nContent-Disposition: form-data; name=\"declared_company_name\"\r\n\r\n{value}\r\n").as_bytes());
    }
    body.extend_from_slice(b"--fixture--\r\n");
    body
}

async fn parse(
    raw: Vec<u8>,
    limits: UploadLimits,
) -> Result<verdaxis_read::kyc::submission::Submission, verdaxis_read::ApiError> {
    let chunks: Vec<_> = raw
        .chunks(7)
        .map(|c| Ok::<_, std::io::Error>(c.to_vec()))
        .collect();
    parse_multipart(
        Body::from_stream(stream::iter(chunks)),
        "multipart/form-data; boundary=fixture",
        limits,
    )
    .await
}

#[tokio::test]
async fn p4_t10_c03() {
    let raw = payload(b"%PDF-synthetic", "application/pdf", None);
    assert!(parse(raw.clone(), UploadLimits::default()).await.is_ok());
    let error = parse(
        raw,
        UploadLimits {
            file_bytes: 8,
            total_bytes: 16,
        },
    )
    .await
    .err()
    .expect("bounded upload");
    assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn p4_t10_c12() {
    for (bytes, mime) in [
        (b"invalid".as_slice(), "application/pdf"),
        (b"%PDF-synthetic", "text/plain"),
    ] {
        let error = parse(payload(bytes, mime, None), UploadLimits::default())
            .await
            .err()
            .expect("closed content set");
        assert_eq!(error.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    assert!(
        parse(
            payload(b"%PDF-synthetic", "application/x-pdf", None),
            UploadLimits::default()
        )
        .await
        .is_ok()
    );
}

#[test]
fn p4_t10_c13() {
    assert!(Declarations::new(Some(" ".into()), None).is_err());
    assert!(Declarations::new(Some("x".repeat(201)), None).is_err());
    assert!(Declarations::new(None, Some("x".into())).is_err());
    assert!(Declarations::new(None, Some("a\nsecret".into())).is_err());
    let valid = Declarations::new(Some(" Example  Company ".into()), Some("AB-12".into()))
        .expect("declarations");
    assert!(
        valid
            .validate_organization("example company", Some("ab-12"))
            .is_ok()
    );
    assert!(valid.validate_organization("other", None).is_err());
    assert!(
        Declarations::new(Some("Straße".into()), None)
            .expect("unicode")
            .validate_organization("STRASSE", None)
            .is_ok()
    );
}

#[tokio::test]
async fn p4_t10_c16() {
    let mut raw = payload(b"%PDF-synthetic", "application/pdf", None);
    raw.extend(vec![b'x'; 70000]);
    let error = parse(
        raw,
        UploadLimits {
            file_bytes: 32,
            total_bytes: 64,
        },
    )
    .await
    .err()
    .expect("stream limit");
    assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
    let raw = payload(&vec![b'x'; 100], "application/pdf", None);
    assert_eq!(
        parse(
            raw,
            UploadLimits {
                file_bytes: 32,
                total_bytes: 1000
            }
        )
        .await
        .err()
        .expect("file limit")
        .status,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[test]
fn p4_t10_c10() {
    let limits = SharedLimits::default();
    let user = Uuid::new_v4().to_string();
    let keys = BusinessLimitKeys {
        client: "fixture",
        user: Some(&user),
        organization: None,
        token: None,
    };
    for _ in 0..5 {
        assert!(
            limits
                .check_business("POST", "/api/kyc/submit", keys, 1)
                .is_ok()
        );
    }
    let error = limits
        .check_business("POST", "/api/kyc/submit", keys, 1)
        .err()
        .expect("quota");
    assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(error.retry_after.is_some());
}

#[test]
fn p4_t10_c11() {
    let limits = SharedLimits::default();
    let keys = BusinessLimitKeys {
        client: "fixture",
        user: None,
        organization: None,
        token: None,
    };
    for _ in 0..30 {
        assert!(
            limits
                .check_business("GET", "/api/kyc/admin/documents/{document_id}", keys, 1)
                .is_ok()
        );
    }
    let error = limits
        .check_business("GET", "/api/kyc/admin/documents/{document_id}", keys, 1)
        .err()
        .expect("quota");
    assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(error.retry_after.is_some());
}

#[test]
fn p4_t10_c08() {
    let value = documents::KycStatus {
        email_verified: true,
        kyc_status: "SUBMITTED".into(),
        kyc_organization_id: None,
        kyc_rejection_reason: None,
    };
    let value = serde_json::to_value(value).expect("status");
    assert!(value["email_verified"].is_boolean());
    assert!(value["kyc_organization_id"].is_null());
    assert!(value["kyc_rejection_reason"].is_null());
}

#[cfg(feature = "contract-tests")]
mod postgres {
    use super::*;
    use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};
    use std::sync::Arc;
    use tokio::sync::Semaphore;
    use verdaxis_read::{
        auth::VerifiedAccess,
        business::transaction::run_ordered_write,
        kyc::{documents::DownloadCommand, keyring::KeyRing, submission::SubmitCommand},
    };

    fn access(id: Uuid) -> VerifiedAccess {
        let key = "test-secret-key-for-testing-minimum-32-chars";
        let now = chrono::Utc::now();
        let token = jsonwebtoken::encode(&jsonwebtoken::Header::default(), &serde_json::json!({"sub":id,"iat":now.timestamp(),"exp":now.timestamp()+3600,"type":"access","iss":"test","aud":"test"}), &jsonwebtoken::EncodingKey::from_secret(key.as_bytes())).expect("fixture token");
        verdaxis_read::auth::verify_access(
            &token,
            &verdaxis_read::auth::VerificationSettings::new(
                key.into(),
                None,
                "test".into(),
                "test".into(),
            ),
            now,
        )
        .expect("fixture access")
    }
    struct Fixture {
        pool: sqlx::PgPool,
        admin: PgConnection,
        schema: String,
        user: Uuid,
        org: Uuid,
    }
    impl Fixture {
        async fn new() -> Self {
            let url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
                .expect("disposable migrator required");
            let runtime =
                std::env::var("RUST_TEST_DATABASE_URL").expect("disposable runtime required");
            let target = url::Url::parse(&url).expect("url");
            let app = url::Url::parse(&runtime).expect("url");
            assert_eq!(target.host_str(), Some("127.0.0.1"));
            assert_eq!(app.host_str(), target.host_str());
            assert_eq!(app.port(), target.port());
            assert_eq!(app.path(), target.path());
            assert_eq!(app.username(), "rust_app");
            assert_ne!(app.username(), target.username());
            let mut admin = PgConnection::connect(&url)
                .await
                .expect("fixture connection");
            let schema = format!("p4k_{}", Uuid::new_v4().simple());
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE SCHEMA {schema}; SET search_path TO {schema}"
            )))
            .execute(&mut admin)
            .await
            .expect("schema");
            sqlx::raw_sql("CREATE TABLE organizations(id uuid PRIMARY KEY,name text,tax_id text); CREATE TABLE users(id uuid PRIMARY KEY,organization_id uuid REFERENCES organizations(id),role text,status text,email_verified boolean DEFAULT true,must_change_password boolean DEFAULT false,password_changed_at timestamptz,authentication_revoked_at timestamptz,kyc_status text DEFAULT 'PENDING',kyc_organization_id uuid,kyc_rejection_reason text,kyc_external_evidence_reference text,kyc_review_note text,kyc_reviewed_by uuid,kyc_reviewed_at timestamptz); CREATE TABLE organization_join_requests(id uuid PRIMARY KEY); CREATE TABLE audit_logs(id uuid PRIMARY KEY,user_id uuid,action varchar(100),resource_type varchar(50),resource_id varchar(100),changes jsonb,ip_address varchar(45),request_id varchar(36),timestamp timestamptz); CREATE TABLE kyc_documents(id uuid PRIMARY KEY,submission_id uuid,user_id uuid REFERENCES users(id) ON DELETE RESTRICT,organization_id uuid REFERENCES organizations(id) ON DELETE RESTRICT,kind text CHECK(kind IN ('IDENTITY','COMPANY_REGISTRATION')),content_type text CHECK(content_type IN ('application/pdf','image/png','image/jpeg')),size_bytes integer CHECK(size_bytes>0),key_id text,wrapped_key bytea,key_nonce bytea,data_nonce bytea,ciphertext bytea,created_at timestamptz DEFAULT now()); CREATE INDEX ON kyc_documents(user_id,created_at);")
                .execute(&mut admin).await.expect("fixture tables");
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!("GRANT USAGE ON SCHEMA {schema} TO rust_app; GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO rust_app; GRANT INSERT ON kyc_documents,audit_logs TO rust_app; GRANT UPDATE(name) ON organizations TO rust_app; GRANT UPDATE(kyc_status,kyc_organization_id,kyc_rejection_reason,kyc_external_evidence_reference,kyc_review_note,kyc_reviewed_by,kyc_reviewed_at) ON users TO rust_app"))).execute(&mut admin).await.expect("restricted grants");
            let user = Uuid::new_v4();
            let org = Uuid::new_v4();
            sqlx::query("INSERT INTO organizations VALUES($1,'Example Company','AB-12')")
                .bind(org)
                .execute(&mut admin)
                .await
                .expect("organization");
            sqlx::query("INSERT INTO users(id,organization_id,role,status) VALUES($1,$2,'ADMIN','APPROVED')").bind(user).bind(org).execute(&mut admin).await.expect("user");
            let path = format!("SET search_path TO {schema}");
            let pool = PgPoolOptions::new()
                .max_connections(3)
                .after_connect(move |c, _| {
                    let p = path.clone();
                    Box::pin(async move {
                        sqlx::raw_sql(sqlx::AssertSqlSafe(p)).execute(c).await?;
                        Ok(())
                    })
                })
                .connect(&runtime)
                .await
                .expect("app pool");
            Self {
                pool,
                admin,
                schema,
                user,
                org,
            }
        }
        fn keys(&self) -> Arc<KeyRing> {
            Arc::new(KeyRing::parse(br#"{"active":"a","keys":{"a":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}}"#).expect("keys"))
        }
        async fn submit(&self) -> Result<documents::Submitted, verdaxis_read::ApiError> {
            let upload = parse(
                payload(b"%PDF-synthetic", "application/pdf", None),
                UploadLimits::default(),
            )
            .await
            .expect("upload");
            run_ordered_write(
                self.pool.clone(),
                Arc::new(Semaphore::new(2)),
                access(self.user),
                SubmitCommand::new(self.user, self.org, upload, self.keys(), chrono::Utc::now()),
            )
            .await
        }
        async fn finish(mut self) {
            self.pool.close().await;
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DROP SCHEMA {} CASCADE",
                self.schema
            )))
            .execute(&mut self.admin)
            .await
            .expect("cleanup");
        }
    }

    #[tokio::test]
    async fn p4_t10_c04() {
        let mut f = Fixture::new().await;
        assert!(f.submit().await.is_ok());
        assert!(f.submit().await.is_ok());
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM kyc_documents")
            .fetch_one(&mut f.admin)
            .await
            .expect("count");
        assert_eq!(n, 4);
        sqlx::raw_sql("ALTER TABLE audit_logs ADD CHECK(action <> 'kyc.submitted')")
            .execute(&mut f.admin)
            .await
            .expect("injection");
        assert!(f.submit().await.is_err());
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM kyc_documents")
            .fetch_one(&mut f.admin)
            .await
            .expect("count");
        assert_eq!(n, 4);
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t10_c05() {
        let mut f = Fixture::new().await;
        assert!(f.submit().await.is_ok());
        let id: Uuid = sqlx::query_scalar("SELECT id FROM kyc_documents LIMIT 1")
            .fetch_one(&mut f.admin)
            .await
            .expect("id");
        let output = run_ordered_write(
            f.pool.clone(),
            Arc::new(Semaphore::new(2)),
            access(f.user),
            DownloadCommand::new(id, f.keys(), chrono::Utc::now()),
        )
        .await
        .expect("committed download");
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_logs WHERE action='kyc.document_viewed'",
        )
        .fetch_one(&mut f.admin)
        .await
        .expect("audit");
        assert_eq!(n, 1);
        let response = output.into_response();
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        sqlx::raw_sql("ALTER TABLE audit_logs ADD CHECK(action <> 'kyc.document_viewed')")
            .execute(&mut f.admin)
            .await
            .expect("injection");
        assert!(
            run_ordered_write(
                f.pool.clone(),
                Arc::new(Semaphore::new(2)),
                access(f.user),
                DownloadCommand::new(id, f.keys(), chrono::Utc::now())
            )
            .await
            .is_err()
        );
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t10_c09() {
        let mut f = Fixture::new().await;
        for _ in 0..26 {
            assert!(f.submit().await.is_ok());
        }
        let actor = verdaxis_read::auth::CurrentActor {
            id: f.user,
            organization_id: Some(f.org),
            role: Some("ADMIN".into()),
            email_verified: true,
        };
        let mut c = f.pool.acquire().await.expect("connection");
        let rows = documents::list(&mut c, &actor, f.user)
            .await
            .expect("metadata");
        assert_eq!(rows.len(), 50);
        for pair in rows.windows(2) {
            assert!((pair[0].created_at, pair[0].id) > (pair[1].created_at, pair[1].id));
        }
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM kyc_documents")
            .fetch_one(&mut f.admin)
            .await
            .expect("count");
        assert_eq!(n, 52);
        drop(c);
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t10_c14() {
        let mut f = Fixture::new().await;
        sqlx::query("UPDATE users SET organization_id=NULL WHERE id=$1")
            .bind(f.user)
            .execute(&mut f.admin)
            .await
            .expect("membership");
        assert_eq!(
            f.submit().await.err().expect("refusal").status,
            StatusCode::FORBIDDEN
        );
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t10_c15() {
        let mut f = Fixture::new().await;
        sqlx::query("UPDATE users SET must_change_password=true WHERE id=$1")
            .bind(f.user)
            .execute(&mut f.admin)
            .await
            .expect("account");
        assert_eq!(
            f.submit().await.err().expect("refusal").status,
            StatusCode::FORBIDDEN
        );
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t10_c21() {
        let mut f = Fixture::new().await;
        let upload = parse(
            payload(b"%PDF-synthetic", "application/pdf", None),
            UploadLimits::default(),
        )
        .await
        .expect("upload");
        let command = SubmitCommand::new(f.user, f.org, upload, f.keys(), chrono::Utc::now());
        let other = Uuid::new_v4();
        sqlx::query("INSERT INTO organizations VALUES($1,'Other',NULL)")
            .bind(other)
            .execute(&mut f.admin)
            .await
            .expect("organization");
        let mut blocker = f.admin.begin().await.expect("barrier transaction");
        sqlx::query("UPDATE users SET organization_id=$2 WHERE id=$1")
            .bind(f.user)
            .bind(other)
            .execute(&mut *blocker)
            .await
            .expect("membership");
        let pool = f.pool.clone();
        let user = f.user;
        let write = tokio::spawn(async move {
            run_ordered_write(pool, Arc::new(Semaphore::new(2)), access(user), command).await
        });
        // Observe a real PostgreSQL lock wait outside the holder's snapshot.
        let observer = f.pool.acquire().await.expect("observer");
        drop(observer);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            sqlx::query("SELECT pg_stat_clear_snapshot()")
                .execute(&mut *blocker)
                .await
                .expect("fresh lock snapshot");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE cardinality(pg_blocking_pids(pid))>0 AND datname=current_database())").fetch_one(&mut *blocker).await.expect("lock barrier");
            if blocked {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "lock barrier not reached"
            );
            tokio::task::yield_now().await;
        }
        blocker.commit().await.expect("membership commit");
        let error = write
            .await
            .expect("owner")
            .err()
            .expect("membership refusal");
        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(error.detail["code"], "KYC_MEMBERSHIP_CHANGED");
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM kyc_documents")
            .fetch_one(&mut f.admin)
            .await
            .expect("count");
        assert_eq!(n, 0);
        f.finish().await;
    }

    #[tokio::test]
    async fn p4_t10_c17() {
        let mut f = Fixture::new().await;
        assert!(f.submit().await.is_ok());
        let id: Uuid = sqlx::query_scalar("SELECT id FROM kyc_documents LIMIT 1")
            .fetch_one(&mut f.admin)
            .await
            .expect("id");
        sqlx::query("UPDATE kyc_documents SET ciphertext='bad'::bytea WHERE id=$1")
            .bind(id)
            .execute(&mut f.admin)
            .await
            .expect("fixture change");
        let error = run_ordered_write(
            f.pool.clone(),
            Arc::new(Semaphore::new(2)),
            access(f.user),
            DownloadCommand::new(id, f.keys(), chrono::Utc::now()),
        )
        .await
        .err()
        .expect("refusal");
        assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.detail["code"], "KYC_DOCUMENT_DECRYPTION_FAILED");
        f.finish().await;
    }
}

#[tokio::test]
async fn p4_t10_c19() {
    let raw = payload(b"%PDF-synthetic", "application/pdf", None);
    let text = String::from_utf8(raw.clone()).expect("synthetic body");
    let duplicate=text.replace("--fixture--\r\n","--fixture\r\nContent-Disposition: form-data; name=\"passport\"; filename=\"unused\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-synthetic\r\n--fixture--\r\n");
    assert!(
        parse(duplicate.into_bytes(), UploadLimits::default())
            .await
            .is_err()
    );
    let unknown = text.replace("company_doc", "unrecognized");
    assert!(
        parse(unknown.into_bytes(), UploadLimits::default())
            .await
            .is_err()
    );
    assert!(
        parse(raw[..raw.len() - 20].to_vec(), UploadLimits::default())
            .await
            .is_err()
    );
}

#[test]
fn p4_t10_c20() {
    // CF4_02 scalar-only double. No SQL-binding declaration is consumed here.
    struct KycPolicy {
        execution_gate_enabled: bool,
    }
    struct KycParty {
        user_id: Uuid,
        organization_id: Option<Uuid>,
        kyc_organization_id: Option<Uuid>,
        kyc_status: &'static str,
    }
    struct Grace {
        user_id: Uuid,
        organization_id: Uuid,
        expires_at: chrono::DateTime<chrono::Utc>,
    }
    struct KycEligibility {
        eligible: bool,
        reason: &'static str,
        grace_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    }
    fn eligibility(
        policy: &KycPolicy,
        party: &KycParty,
        grace: Option<&Grace>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> KycEligibility {
        let mut result = KycEligibility {
            eligible: false,
            reason: "KYC_REQUIRED",
            grace_expires_at: None,
        };
        if !matches!(
            party.kyc_status,
            "PENDING" | "SUBMITTED" | "APPROVED" | "REJECTED"
        ) {
            return result;
        }
        if party.kyc_status == "REJECTED" {
            result.reason = "KYC_REJECTED";
            return result;
        }
        if !policy.execution_gate_enabled {
            result.eligible = party.kyc_organization_id.is_none()
                || party.kyc_organization_id == party.organization_id;
            if result.eligible {
                result.reason = "KYC_NOT_ENFORCED";
            }
            return result;
        }
        if party.organization_id.is_some()
            && party.kyc_organization_id == party.organization_id
            && party.kyc_status == "APPROVED"
        {
            result.eligible = true;
            result.reason = "KYC_APPROVED";
            return result;
        }
        if let Some(g) = grace.filter(|g| {
            g.user_id == party.user_id
                && Some(g.organization_id) == party.organization_id
                && now < g.expires_at
        }) {
            result.eligible = true;
            result.reason = "KYC_GRACE";
            result.grace_expires_at = Some(g.expires_at);
        }
        result
    }
    let user = Uuid::new_v4();
    let org = Uuid::new_v4();
    let now = chrono::Utc::now();
    let party = KycParty {
        user_id: user,
        organization_id: Some(org),
        kyc_organization_id: None,
        kyc_status: "SUBMITTED",
    };
    assert!(
        eligibility(
            &KycPolicy {
                execution_gate_enabled: false
            },
            &party,
            None,
            now
        )
        .eligible
    );
    assert!(
        !eligibility(
            &KycPolicy {
                execution_gate_enabled: true
            },
            &party,
            None,
            now
        )
        .eligible
    );
    let unknown = KycParty {
        kyc_status: "UNKNOWN",
        ..party
    };
    assert!(
        !eligibility(
            &KycPolicy {
                execution_gate_enabled: false
            },
            &unknown,
            None,
            now
        )
        .eligible
    );
}
