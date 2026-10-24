//! Phase 4 Task 5: administrator invitations and public resolve.
//!
//! Shared-file note: `business/mod.rs` is integration-owned, so the leaf
//! modules are compiled here by path. The declarations below also provide the
//! `crate::` names those modules use, which is the pattern Task 8 established.

pub use verdaxis_read::business;
pub use verdaxis_read::{ApiError, AppState, Config, MarketPolicy, auth, crud, market, runtime};
#[path = "../src/business/invitations/request.rs"]
pub mod request;
#[path = "../src/business/invitations/resolve.rs"]
pub mod resolve;
#[path = "../src/business/invitations/store.rs"]
pub mod store;

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use request::{
    CreateInvitation, InvitationListQuery, InvitationRole, NewOrganization,
    OrganizationDomainPolicy, OrganizationSource, ResolveToken,
};

fn instant() -> DateTime<Utc> {
    "2026-10-24T09:00:00Z".parse().unwrap()
}

fn body(value: Value) -> CreateInvitation {
    CreateInvitation::parse(&value).expect("valid invitation body")
}

fn existing_payload(organization: Uuid) -> Value {
    json!({
        "email": "abdullah@customer.example",
        "first_name": "Abdullah",
        "last_name": "Rahman",
        "role": "SUPPLIER",
        "organization_id": organization,
    })
}

fn new_organization_payload() -> Value {
    json!({
        "email": "new.user@northstar.example",
        "first_name": "New",
        "last_name": "User",
        "role": "BUYER",
        "new_organization": {
            "name": "Northstar Shipping",
            "type": "SHIPPING_LINE",
            "country_code": "SG",
            "tax_id": "SG-2026-001",
        },
    })
}

/// The release-owned domain policy Phase 3 registration supplies. The data
/// lists are fixed here so no test reaches DNS or a network.
struct Domains {
    excluded: Vec<String>,
}
impl OrganizationDomainPolicy for Domains {
    fn organization_domain(&self, email: &str) -> Option<String> {
        let domain = email.rsplit_once('@')?.1.trim_matches('.').to_lowercase();
        if !domain.contains('.') {
            return None;
        }
        let labels: Vec<&str> = domain.split('.').collect();
        if (0..labels.len() - 1).any(|index| {
            self.excluded
                .iter()
                .any(|item| item == &labels[index..].join("."))
        }) {
            return None;
        }
        Some(domain)
    }
}

fn domains() -> Arc<dyn OrganizationDomainPolicy> {
    Arc::new(Domains {
        excluded: vec!["gmail.com".into(), "disposable.example".into()],
    })
}

// ---------------------------------------------------------------------------
// Pure contract cases.
// ---------------------------------------------------------------------------

#[test]
fn p4_t5_c02() {
    // P4-D5: a new invitation organization takes an officially assigned ISO
    // alpha-2 code, upper-cased; user-assigned and reserved codes are refused
    // before any write.
    for (input, expected) in [("sg", "SG"), ("NL", "NL"), ("gb", "GB")] {
        let mut payload = new_organization_payload();
        payload["new_organization"]["country_code"] = json!(input);
        assert_eq!(
            match body(payload).organization {
                OrganizationSource::New(organization) => organization.country_code,
                OrganizationSource::Existing(_) => unreachable!("new organization"),
            },
            expected
        );
    }
    for refused in [
        "ZZ", "XX", "XK", "EU", "UK", "AA", "QM", "S1", "S", "SGP", "",
    ] {
        let mut payload = new_organization_payload();
        payload["new_organization"]["country_code"] = json!(refused);
        assert_eq!(
            CreateInvitation::parse(&payload)
                .unwrap_err()
                .status
                .as_u16(),
            422,
            "country {refused} must be refused"
        );
    }
}

#[test]
fn p4_t5_c50() {
    // A trader supplier is invitation-only: the role/type matrix admits a
    // FUEL_TRADER organization for a SUPPLIER and nothing else.
    assert!(store::organization_type_matches_role(
        "SUPPLIER",
        "FUEL_TRADER"
    ));
    assert!(store::organization_type_matches_role(
        "SUPPLIER",
        "FUEL_SUPPLIER"
    ));
    assert!(!store::organization_type_matches_role(
        "BUYER",
        "FUEL_TRADER"
    ));
    assert!(!store::organization_type_matches_role(
        "SUPPLIER",
        "FUEL_BUYER"
    ));
    let mut payload = new_organization_payload();
    payload["role"] = json!("SUPPLIER");
    payload["new_organization"] = json!({
        "name": "LNG Trader", "type": "FUEL_TRADER", "country_code": "NL"
    });
    assert_eq!(body(payload).role, InvitationRole::Supplier);
}

#[test]
fn p4_t5_c56() {
    // Exactly one organization source.
    let organization = Uuid::new_v4();
    assert!(CreateInvitation::parse(&existing_payload(organization)).is_ok());
    let mut neither = new_organization_payload();
    neither.as_object_mut().unwrap().remove("new_organization");
    assert_eq!(
        CreateInvitation::parse(&neither)
            .unwrap_err()
            .status
            .as_u16(),
        422
    );
    let mut both = existing_payload(organization);
    both["new_organization"] = new_organization_payload()["new_organization"].clone();
    assert_eq!(
        CreateInvitation::parse(&both).unwrap_err().status.as_u16(),
        422
    );
    let mut empty = existing_payload(organization);
    empty["organization_id"] = Value::Null;
    assert_eq!(
        CreateInvitation::parse(&empty).unwrap_err().status.as_u16(),
        422
    );
    // A non-UUID organization id is a validation failure, never a lookup.
    let mut malformed = existing_payload(organization);
    malformed["organization_id"] = json!("not-a-uuid");
    assert_eq!(
        CreateInvitation::parse(&malformed)
            .unwrap_err()
            .status
            .as_u16(),
        422
    );
    // ADMIN is never an invitation role.
    let mut admin = existing_payload(organization);
    admin["role"] = json!("ADMIN");
    assert_eq!(
        CreateInvitation::parse(&admin).unwrap_err().status.as_u16(),
        422
    );
    // Names are stripped and may not become blank.
    let mut names = existing_payload(organization);
    names["first_name"] = json!("  ");
    assert_eq!(
        CreateInvitation::parse(&names).unwrap_err().status.as_u16(),
        422
    );
    let mut names = existing_payload(organization);
    names["last_name"] = json!("  Rahman  ");
    assert_eq!(body(names).last_name.as_deref(), Some("Rahman"));
    // A blank tax id is stored as null, as Python's normalizer does.
    let mut tax = new_organization_payload();
    tax["new_organization"]["tax_id"] = json!("   ");
    assert!(matches!(
        body(tax).organization,
        OrganizationSource::New(NewOrganization { tax_id: None, .. })
    ));
}

#[test]
fn p4_t5_c57() {
    // A role/type mismatch on a new organization is refused before any write.
    let mut payload = new_organization_payload();
    payload["new_organization"] = json!({
        "name": "Wrong Side Fuels", "type": "FUEL_SUPPLIER", "country_code": "SG"
    });
    let invitation = body(payload);
    assert!(!store::organization_type_matches_role(
        invitation.role.as_str(),
        match invitation.organization {
            OrganizationSource::New(NewOrganization { ref kind, .. }) => kind.as_str(),
            OrganizationSource::Existing(_) => unreachable!("new organization"),
        }
    ));
}

#[test]
fn p4_t5_c62() {
    // The role always matches the organization side: BUYER takes the four
    // buy-side types only, SUPPLIER the two supply-side types only.
    for kind in ["SHIPPING_LINE", "SHIP_MANAGER", "FUEL_BUYER", "CHARTERER"] {
        assert!(
            store::organization_type_matches_role("BUYER", kind),
            "{kind}"
        );
        assert!(
            !store::organization_type_matches_role("SUPPLIER", kind),
            "{kind}"
        );
    }
    for kind in ["FUEL_SUPPLIER", "FUEL_TRADER"] {
        assert!(
            store::organization_type_matches_role("SUPPLIER", kind),
            "{kind}"
        );
        assert!(
            !store::organization_type_matches_role("BUYER", kind),
            "{kind}"
        );
    }
    for kind in [
        "BUNKER_BROKER",
        "PORT_AUTHORITY",
        "FINANCIER",
        "INSURER",
        "INDUSTRY_ASSOC",
    ] {
        assert!(!store::organization_type_matches_role("BUYER", kind));
        assert!(!store::organization_type_matches_role("SUPPLIER", kind));
    }
    assert!(!store::organization_type_matches_role(
        "ADMIN",
        "FUEL_BUYER"
    ));
}

#[test]
fn p4_t5_c70() {
    // An unknown organization type is refused, and every model type parses.
    let mut payload = new_organization_payload();
    payload["new_organization"]["type"] = json!("UNSUPPORTED_ORG_TYPE");
    assert_eq!(
        CreateInvitation::parse(&payload)
            .unwrap_err()
            .status
            .as_u16(),
        422
    );
    for kind in [
        "SHIPPING_LINE",
        "SHIP_MANAGER",
        "FUEL_BUYER",
        "FUEL_SUPPLIER",
        "BUNKER_BROKER",
        "PORT_AUTHORITY",
        "FUEL_TRADER",
        "CHARTERER",
        "FINANCIER",
        "INSURER",
        "INDUSTRY_ASSOC",
    ] {
        let mut payload = new_organization_payload();
        payload["new_organization"]["type"] = json!(kind);
        assert!(CreateInvitation::parse(&payload).is_ok(), "{kind}");
    }
}

#[test]
fn p4_t5_c05() {
    // A06 input: 32 to 128 characters, nothing else; the secret is redacted in
    // every diagnostic form.
    for length in [32usize, 43, 128] {
        let token = "a".repeat(length);
        let parsed = ResolveToken::parse(&json!({"token": token})).unwrap();
        assert_eq!(parsed.as_str().len(), length);
        assert_eq!(format!("{parsed:?}"), "ResolveToken(REDACTED)");
        assert!(!format!("{parsed:?}").contains(token.as_str()));
    }
    for refused in [
        Value::Null,
        json!("short"),
        json!("a".repeat(129)),
        json!(17),
        json!([]),
    ] {
        let payload = if refused == Value::Null {
            json!({})
        } else {
            json!({"token": refused})
        };
        assert_eq!(
            ResolveToken::parse(&payload).unwrap_err().status.as_u16(),
            422
        );
    }
    // The refusal for a wrong secret is one shape, independent of why.
    let (status, code) = resolve::invalid_refusal_shape();
    assert_eq!(status.as_u16(), 400);
    assert_eq!(code, "INVITATION_INVALID");
    // A stored digest is the SHA-256 hex of the secret, never the secret.
    let secret = store::new_invitation_secret(instant());
    let url = secret.acceptance_url("https://app.example/");
    assert!(url.starts_with("https://app.example/accept-invite#token="));
    let raw = url.rsplit("#token=").next().unwrap();
    assert_eq!(raw.len(), 43);
    assert_eq!(store::digest_of(raw).len(), 64);
    assert_ne!(store::digest_of(raw), raw);
    assert_eq!(format!("{secret:?}"), "InvitationSecret(REDACTED)");
    assert_eq!(
        secret.expires_at(),
        instant() + Duration::days(request::INVITATION_TTL_DAYS)
    );
    assert_eq!(secret.digest(), store::digest_of(raw));
}

/// The resolver's refusal has one shape, so no caller can learn why a secret
/// failed.
#[test]
fn p4_t5_c63() {
    let (status, code) = resolve::invalid_refusal_shape();
    assert_eq!(status.as_u16(), 400);
    assert_eq!(code, "INVITATION_INVALID");
    // The only organization states a resolvable invitation may name.
    let eligible = |verification: &str, provenance: &str| {
        verification == "APPROVED" && matches!(provenance, "REAL" | "UNKNOWN")
    };
    assert!(eligible("APPROVED", "REAL"));
    assert!(eligible("APPROVED", "UNKNOWN"));
    for (verification, provenance) in [
        ("PENDING", "REAL"),
        ("APPROVED", "DEMO"),
        ("APPROVED", "TEST"),
        ("APPROVED", "CANARY"),
    ] {
        assert!(!eligible(verification, provenance));
    }
}

#[test]
fn p4_t5_c01() {
    // A04 query bounds: default 100, maximum 500, offset maximum 10000,
    // negative or non-integer values are 422.
    let query = InvitationListQuery::parse_query("").unwrap();
    assert_eq!((query.limit(), query.offset()), (100, 0));
    assert_eq!(
        InvitationListQuery::parse_query("limit=500&offset=10000")
            .unwrap()
            .limit(),
        500
    );
    for raw in [
        "limit=0",
        "limit=501",
        "limit=-1",
        "offset=-1",
        "offset=10001",
        "limit=abc",
        "offset=1.5",
    ] {
        assert_eq!(
            InvitationListQuery::parse_query(raw)
                .unwrap_err()
                .status
                .as_u16(),
            422,
            "{raw}"
        );
    }
    // An unknown parameter is ignored; a repeat takes the last value.
    assert_eq!(
        InvitationListQuery::parse_query("limit=5&other=7")
            .unwrap()
            .limit(),
        5
    );
    assert_eq!(
        InvitationListQuery::parse_query("limit=5&limit=9")
            .unwrap()
            .limit(),
        9
    );
}

// ---------------------------------------------------------------------------
// PostgreSQL cases.
// ---------------------------------------------------------------------------

#[cfg(feature = "contract-tests")]
mod postgres {
    use super::*;
    use sqlx::{Connection, PgConnection, Row, postgres::PgPoolOptions};
    use tower::ServiceExt;
    use verdaxis_read::auth::VerificationSettings;

    use request::InvitationAudit;

    struct Fixture {
        pool: sqlx::PgPool,
        admin: PgConnection,
        schema: String,
        actor: Uuid,
        organization: Uuid,
    }

    impl Fixture {
        async fn new() -> Self {
            let url = std::env::var("RUST_TEST_MIGRATOR_DATABASE_URL")
                .expect("explicit disposable migrator URL");
            let runtime =
                std::env::var("RUST_TEST_DATABASE_URL").expect("explicit disposable runtime URL");
            let migrator = url::Url::parse(&url).unwrap();
            let app = url::Url::parse(&runtime).unwrap();
            assert_eq!(migrator.host_str(), Some("127.0.0.1"));
            assert_eq!(app.host_str(), Some("127.0.0.1"));
            assert_eq!(app.port(), migrator.port());
            assert_eq!(app.path(), migrator.path());
            assert!(app.path().ends_with("_test"));
            assert_eq!(app.username(), "rust_app");
            let mut admin = PgConnection::connect(&url).await.unwrap();
            let schema = format!("p4_t5_{}", Uuid::new_v4().simple());
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "CREATE SCHEMA {schema}; SET search_path TO {schema}"
            )))
            .execute(&mut admin)
            .await
            .unwrap();
            sqlx::raw_sql(
                "CREATE TABLE organizations(id uuid PRIMARY KEY,name text NOT NULL,domain text UNIQUE,type text NOT NULL,verification_status text NOT NULL,provenance text NOT NULL,tax_id text,country_code text,created_at timestamptz);\
                 CREATE TABLE users(id uuid PRIMARY KEY,email text NOT NULL UNIQUE,password_hash text NOT NULL,first_name text,last_name text,role text,status text NOT NULL,organization_id uuid REFERENCES organizations(id),email_verified boolean NOT NULL DEFAULT false,must_change_password boolean NOT NULL DEFAULT false,password_changed_at timestamptz,authentication_revoked_at timestamptz,referred_by_id uuid,referral_code text UNIQUE,password_reset_token_hash text,password_reset_expires timestamptz,created_at timestamptz NOT NULL DEFAULT now());\
                 CREATE TABLE referrals(id uuid PRIMARY KEY,referrer_id uuid NOT NULL,referred_user_id uuid NOT NULL UNIQUE,referral_code_used text NOT NULL,status text NOT NULL,created_at timestamptz,verified_at timestamptz,activated_at timestamptz);\
                 CREATE TABLE audit_logs(id uuid PRIMARY KEY,user_id uuid,action text,resource_type text,resource_id text,changes jsonb,ip_address text,request_id text,timestamp timestamptz);\
                 CREATE TABLE user_status_transitions(id uuid PRIMARY KEY,user_id uuid,organization_id uuid,role text,from_status text,to_status text,effective_at timestamptz,provenance text);",
            )
            .execute(&mut admin)
            .await
            .unwrap();
            let actor = Uuid::new_v4();
            let organization = Uuid::new_v4();
            sqlx::query("INSERT INTO organizations(id,name,domain,type,verification_status,provenance,created_at) VALUES($1,'Goldwind Green Methanol','goldwind.example','FUEL_SUPPLIER','APPROVED','REAL',now())")
                .bind(organization)
                .execute(&mut admin)
                .await
                .unwrap();
            sqlx::query("INSERT INTO users(id,email,password_hash,first_name,last_name,role,status,email_verified) VALUES($1,'belinda@verdaxis.exchange','synthetic','Belinda','Tan','ADMIN','APPROVED',true)")
                .bind(actor)
                .execute(&mut admin)
                .await
                .unwrap();
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "GRANT USAGE ON SCHEMA {schema} TO rust_app;\
                 GRANT SELECT,INSERT,UPDATE ON ALL TABLES IN SCHEMA {schema} TO rust_app;\
                 REVOKE UPDATE ON {schema}.audit_logs,{schema}.user_status_transitions FROM rust_app"
            )))
            .execute(&mut admin)
            .await
            .unwrap();
            let search_path = format!("SET search_path TO {schema}");
            let pool = PgPoolOptions::new()
                .max_connections(4)
                .after_connect(move |conn, _| {
                    let sql = search_path.clone();
                    Box::pin(async move {
                        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                            .execute(conn)
                            .await?;
                        Ok(())
                    })
                })
                .connect(&runtime)
                .await
                .unwrap();
            Self {
                pool,
                admin,
                schema,
                actor,
                organization,
            }
        }

        fn command(
            &self,
            invitation: CreateInvitation,
            now: DateTime<Utc>,
        ) -> store::CreateInvitationCommand {
            store::CreateInvitationCommand {
                invitation,
                now,
                audit: InvitationAudit {
                    ip_address: Some("127.0.0.8".into()),
                    request_id: Some("request-5".into()),
                },
                domains: domains(),
                frontend_url: "https://app.example".into(),
            }
        }

        async fn create(
            &self,
            invitation: CreateInvitation,
            now: DateTime<Utc>,
        ) -> Result<store::CreatedInvitation, ApiError> {
            let access = self.access();
            verdaxis_read::business::transaction::run_ordered_write(
                self.pool.clone(),
                Arc::new(tokio::sync::Semaphore::new(4)),
                access,
                self.command(invitation, now),
            )
            .await
        }

        fn access(&self) -> auth::VerifiedAccess {
            let key = "synthetic-test-secret-at-least-32-bytes";
            let token = jsonwebtoken::encode(
                &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
                &json!({"sub":self.actor,"type":"access","iat":Utc::now().timestamp(),
                        "exp":Utc::now().timestamp()+900,"iss":"test","aud":"test"}),
                &jsonwebtoken::EncodingKey::from_secret(key.as_bytes()),
            )
            .unwrap();
            auth::verify_access(
                &token,
                &VerificationSettings::new(key.into(), None, "test".into(), "test".into()),
                Utc::now(),
            )
            .unwrap()
        }

        async fn finish(mut self) {
            self.pool.close().await;
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DROP SCHEMA {} CASCADE",
                self.schema
            )))
            .execute(&mut self.admin)
            .await
            .unwrap();
        }
    }

    fn token_from(response: &store::CreatedInvitation) -> String {
        response
            .acceptance_url
            .rsplit("#token=")
            .next()
            .expect("fragment token")
            .to_owned()
    }

    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t5_c51() {
        let f = Fixture::new().await;
        let now = instant();
        let payload = existing_payload(f.organization);
        let first = f.create(body(&payload), now).await.unwrap();
        assert!(!first.reissued);
        let second = f
            .create(body(&payload), now + Duration::minutes(1))
            .await
            .unwrap();
        assert!(second.reissued);
        assert_eq!(first.user_id, second.user_id);
        assert_ne!(token_from(&first), token_from(&second));

        // The invited account is pre-approved, unverified, must change the
        // password, and is attributed to the inviting administrator.
        let row = sqlx::query("SELECT status,email_verified,must_change_password,organization_id,referred_by_id,role FROM users WHERE email=$1")
            .bind("abdullah@customer.example")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("status"), "APPROVED");
        assert!(!row.get::<bool, _>("email_verified"));
        assert!(row.get::<bool, _>("must_change_password"));
        assert_eq!(row.get::<Uuid, _>("organization_id"), f.organization);
        assert_eq!(row.get::<Uuid, _>("referred_by_id"), f.actor);
        assert_eq!(row.get::<String, _>("role"), "SUPPLIER");
        // A referral and the initial status transition exist once each.
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM referrals WHERE referred_user_id=$1"
            )
            .bind(first.user_id)
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_status_transitions WHERE user_id=$1 AND from_status IS NULL AND to_status='APPROVED'")
                .bind(first.user_id)
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        // Two audits under Python's exact action name, with the masked address
        // and the correlation context; no tax id.
        let audits = sqlx::query(
            "SELECT action,changes,ip_address,request_id FROM audit_logs ORDER BY timestamp,id",
        )
        .fetch_all(&f.pool)
        .await
        .unwrap();
        assert_eq!(audits.len(), 2);
        let mut reissued = Vec::new();
        for audit in audits {
            assert_eq!(audit.get::<String, _>("action"), "admin.user_invited");
            let changes: Value = audit.get("changes");
            assert_eq!(changes["email"], "a***@customer.example");
            assert_eq!(changes["organization_provenance"], "REAL");
            assert_eq!(changes["organization_type"], "FUEL_SUPPLIER");
            assert!(changes.get("tax_id").is_none());
            assert_eq!(
                audit.get::<Option<String>, _>("ip_address").as_deref(),
                Some("127.0.0.8")
            );
            assert_eq!(
                audit.get::<Option<String>, _>("request_id").as_deref(),
                Some("request-5")
            );
            reissued.push(changes["reissued"].as_bool().unwrap());
        }
        reissued.sort_unstable();
        assert_eq!(reissued, [false, true]);
        // Only the digest is stored, and it matches the returned secret.
        let stored: Option<String> =
            sqlx::query_scalar("SELECT password_reset_token_hash FROM users WHERE id=$1")
                .bind(first.user_id)
                .fetch_one(&f.pool)
                .await
                .unwrap();
        let stored = stored.expect("stored digest");
        assert_eq!(stored.len(), 64);
        assert_ne!(stored, token_from(&first));
        assert_eq!(stored, store::digest_of(&token_from(&first)));
        assert!(
            first
                .acceptance_url
                .starts_with("https://app.example/accept-invite#token=")
        );
        assert_eq!(
            first.expires_at,
            now + Duration::days(request::INVITATION_TTL_DAYS)
        );
        f.finish().await;
    }

    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t5_c52() {
        let f = Fixture::new().await;
        let created = f
            .create(body(&new_organization_payload()), instant())
            .await
            .unwrap();
        assert!(created.organization_created);
        assert_eq!(created.organization_name, "Northstar Shipping");
        let organization = sqlx::query("SELECT id,domain,type,country_code,tax_id,verification_status,provenance FROM organizations WHERE name='Northstar Shipping'")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            organization.get::<Option<String>, _>("domain").as_deref(),
            Some("northstar.example")
        );
        assert_eq!(organization.get::<String, _>("type"), "SHIPPING_LINE");
        assert_eq!(
            organization
                .get::<Option<String>, _>("country_code")
                .as_deref(),
            Some("SG")
        );
        assert_eq!(
            organization.get::<Option<String>, _>("tax_id").as_deref(),
            Some("SG-2026-001")
        );
        assert_eq!(
            organization.get::<String, _>("verification_status"),
            "APPROVED"
        );
        assert_eq!(organization.get::<String, _>("provenance"), "REAL");
        let audit: Value =
            sqlx::query_scalar("SELECT changes FROM audit_logs WHERE action='admin.user_invited'")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(audit["organization_created"], true);
        assert_eq!(audit["organization_country_code"], "SG");
        assert!(!audit.to_string().to_lowercase().contains("tax"));
        f.finish().await;
    }

    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t5_c53() {
        // A second user can join the new organization before the first accepts.
        let f = Fixture::new().await;
        let first = f
            .create(body(&new_organization_payload()), instant())
            .await
            .unwrap();
        let organization: Uuid =
            sqlx::query_scalar("SELECT id FROM organizations WHERE name='Northstar Shipping'")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        let second = f
            .create(
                body(&json!({
                    "email": "second.user@northstar.example",
                    "first_name": "Second",
                    "last_name": "User",
                    "role": "BUYER",
                    "organization_id": organization,
                })),
                instant() + Duration::minutes(1),
            )
            .await
            .unwrap();
        assert!(!second.organization_created);
        assert_eq!(second.organization_name, "Northstar Shipping");
        assert_ne!(first.user_id, second.user_id);
        // No second organization was created.
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM organizations")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            2
        );
        f.finish().await;
    }

    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t5_c54() {
        // A04 lists APPROVED REAL and UNKNOWN organizations only, ordered by
        // (name,id), with limit+1 truncation.
        let f = Fixture::new().await;
        for (name, verification, provenance) in [
            ("Pending Market Review", "APPROVED", "UNKNOWN"),
            ("Unapproved Organization", "PENDING", "REAL"),
            ("Demo Organization", "APPROVED", "DEMO"),
            ("Canary Organization", "APPROVED", "CANARY"),
        ] {
            sqlx::query("INSERT INTO organizations(id,name,domain,type,verification_status,provenance,created_at) VALUES($1,$2,$3,'FUEL_SUPPLIER',$4,$5,now())")
                .bind(Uuid::new_v4())
                .bind(name)
                .bind(format!("{}.example", name.split(' ').next().unwrap().to_lowercase()))
                .bind(verification)
                .bind(provenance)
                .execute(&f.pool)
                .await
                .unwrap();
        }
        let mut connection = f.pool.acquire().await.unwrap();
        let page = store::list_organizations(
            &mut *connection,
            &InvitationListQuery::parse(Some(100), Some(0)).unwrap(),
        )
        .await
        .unwrap();
        let names: Vec<&str> = page.items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["Goldwind Green Methanol", "Pending Market Review"]);
        assert!(!page.truncated);
        let provenance: Vec<&str> = page
            .items
            .iter()
            .map(|item| item.provenance.as_str())
            .collect();
        assert_eq!(provenance, ["REAL", "UNKNOWN"]);
        // 501 eligible rows produce one page of 500 and the truncation marker.
        for index in 0..501 {
            sqlx::query("INSERT INTO organizations(id,name,type,verification_status,provenance,created_at) VALUES($1,$2,'FUEL_SUPPLIER','APPROVED','REAL',now())")
                .bind(Uuid::new_v4())
                .bind(format!("Eligible {index:03}"))
                .execute(&f.pool)
                .await
                .unwrap();
        }
        let page = store::list_organizations(
            &mut *connection,
            &InvitationListQuery::parse(Some(500), Some(0)).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(page.items.len(), 500);
        assert!(page.truncated);
        // Ordering is deterministic on (name,id).
        let page = store::list_organizations(
            &mut *connection,
            &InvitationListQuery::parse(Some(500), Some(0)).unwrap(),
        )
        .await
        .unwrap();
        let keys: Vec<(String, Uuid)> = page
            .items
            .iter()
            .map(|item| (item.name.clone(), item.id))
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        drop(connection);
        f.finish().await;
    }

    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t5_c04() {
        // Only an unclaimed, matching invitation can rotate, and a mismatch,
        // a claimed account or an ineligible organization is a 409 with no write.
        let mut f = Fixture::new().await;
        let now = instant();
        let created = f
            .create(body(&existing_payload(f.organization)), now)
            .await
            .unwrap();
        // A claimed account (verified) cannot be reissued.
        sqlx::query("UPDATE users SET email_verified=true WHERE id=$1")
            .bind(created.user_id)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let error = f
            .create(
                body(&existing_payload(f.organization)),
                now + Duration::minutes(1),
            )
            .await
            .unwrap_err();
        assert_eq!(error.status.as_u16(), 409);
        // The digest is unchanged by the refused rotation.
        let stored: String =
            sqlx::query_scalar("SELECT password_reset_token_hash FROM users WHERE id=$1")
                .bind(created.user_id)
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(stored, store::digest_of(&token_from(&created)));
        // A stored role that no longer matches the requested role is a conflict,
        // not a rotation; the requested SUPPLIER still fits the organization.
        sqlx::query("UPDATE users SET email_verified=false,role='BUYER' WHERE id=$1")
            .bind(created.user_id)
            .execute(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(
            f.create(body(&existing_payload(f.organization)), now)
                .await
                .unwrap_err()
                .status
                .as_u16(),
            409
        );
        sqlx::query("UPDATE users SET role='SUPPLIER' WHERE id=$1")
            .bind(created.user_id)
            .execute(&mut f.admin)
            .await
            .unwrap();
        // A requested role that does not fit the organization type is 422.
        let mut wrong_role = existing_payload(f.organization);
        wrong_role["role"] = json!("BUYER");
        assert_eq!(
            f.create(body(&wrong_role), now)
                .await
                .unwrap_err()
                .status
                .as_u16(),
            422
        );
        // An ineligible organization is 404; a mismatched type is 422.
        sqlx::query("UPDATE organizations SET provenance='DEMO' WHERE id=$1")
            .bind(f.organization)
            .execute(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(
            f.create(body(&existing_payload(f.organization)), now)
                .await
                .unwrap_err()
                .status
                .as_u16(),
            404
        );
        sqlx::query("UPDATE organizations SET provenance='REAL',type='FUEL_BUYER' WHERE id=$1")
            .bind(f.organization)
            .execute(&mut f.admin)
            .await
            .unwrap();
        assert_eq!(
            f.create(body(&existing_payload(f.organization)), now)
                .await
                .unwrap_err()
                .status
                .as_u16(),
            422
        );
        f.finish().await;
    }

    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t5_c58() {
        // An owned domain is refused without creating the organization or user.
        let f = Fixture::new().await;
        let payload = json!({
            "email": "new.supplier@goldwind.example",
            "first_name": "New",
            "role": "SUPPLIER",
            "new_organization": {
                "name": "Duplicate Domain Supplier",
                "type": "FUEL_SUPPLIER",
                "country_code": "SG",
            },
        });
        let error = f.create(body(&payload), instant()).await.unwrap_err();
        assert_eq!(error.status.as_u16(), 409);
        assert_eq!(error.detail["code"], "ORGANIZATION_DOMAIN_CONFLICT");
        for table in ["organizations", "users", "referrals", "audit_logs"] {
            let count: i64 =
                sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
                    .fetch_one(&f.pool)
                    .await
                    .unwrap();
            assert!(count <= 1, "{table} has {count} rows");
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM organizations WHERE name='Duplicate Domain Supplier'"
            )
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            0
        );
        f.finish().await;
    }

    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t5_c59() {
        // An existing registered address never leaves an orphan organization.
        let f = Fixture::new().await;
        sqlx::query("INSERT INTO users(id,email,password_hash,first_name,role,status,email_verified,organization_id) VALUES($1,'registered@other.example','synthetic','Registered','BUYER','APPROVED',true,$2)")
            .bind(Uuid::new_v4())
            .bind(f.organization)
            .execute(&f.pool)
            .await
            .unwrap();
        let payload = json!({
            "email": "registered@other.example",
            "first_name": "Registered",
            "role": "BUYER",
            "new_organization": {
                "name": "Must Roll Back",
                "type": "SHIPPING_LINE",
                "country_code": "SG",
            },
        });
        assert_eq!(
            f.create(body(&payload), instant())
                .await
                .unwrap_err()
                .status
                .as_u16(),
            409
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM organizations WHERE name='Must Roll Back'"
            )
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            0
        );
        f.finish().await;
    }

    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t5_c61() {
        // The write requires an administrator and an eligible organization; a
        // demoted administrator is refused and no target is read.
        let mut f = Fixture::new().await;
        sqlx::query("UPDATE users SET role='BUYER' WHERE id=$1")
            .bind(f.actor)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let error = f
            .create(body(&existing_payload(f.organization)), instant())
            .await
            .unwrap_err();
        assert_eq!(error.status.as_u16(), 403);
        assert_eq!(error.detail["code"], "ADMIN_REQUIRED");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_logs")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            0
        );
        f.finish().await;
    }

    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t5_c05_postgres() {
        // A06 resolves only an unclaimed, matching invitation, does not consume
        // it, and gives one refusal for every failure mode.
        let mut f = Fixture::new().await;
        let now = instant();
        let created = f
            .create(body(&existing_payload(f.organization)), now)
            .await
            .unwrap();
        let secret = token_from(&created);
        let mut connection = f.pool.acquire().await.unwrap();
        let resolved = resolve::resolve(
            &mut *connection,
            &ResolveToken::parse(&json!({"token": secret})).unwrap(),
            now,
        )
        .await
        .unwrap();
        assert_eq!(resolved.email, "abdullah@customer.example");
        assert_eq!(resolved.first_name.as_deref(), Some("Abdullah"));
        assert_eq!(resolved.last_name.as_deref(), Some("Rahman"));
        assert_eq!(resolved.role, "SUPPLIER");
        assert_eq!(resolved.organization_name, "Goldwind Green Methanol");
        assert_eq!(resolved.invited_by_name.as_deref(), Some("Belinda Tan"));
        assert_eq!(resolved.expires_at, created.expires_at);
        // Resolve does not consume the secret.
        let again = resolve::resolve(
            &mut *connection,
            &ResolveToken::parse(&json!({"token": secret})).unwrap(),
            now,
        )
        .await;
        assert!(again.is_ok());
        // Wrong, expired and mismatched secrets share one refusal.
        let wrong = resolve::resolve(
            &mut *connection,
            &ResolveToken::parse(&json!({"token": "z".repeat(43)})).unwrap(),
            now,
        )
        .await
        .unwrap_err();
        assert_eq!(wrong.status.as_u16(), 400);
        assert_eq!(wrong.detail["code"], "INVITATION_INVALID");
        let expired = resolve::resolve(
            &mut *connection,
            &ResolveToken::parse(&json!({"token": secret})).unwrap(),
            created.expires_at + Duration::seconds(1),
        )
        .await
        .unwrap_err();
        assert_eq!(
            (expired.status, expired.detail.clone()),
            (wrong.status, wrong.detail.clone())
        );
        sqlx::query("UPDATE users SET role='BUYER' WHERE id=$1")
            .bind(created.user_id)
            .execute(&mut f.admin)
            .await
            .unwrap();
        let mismatched = resolve::resolve(
            &mut *connection,
            &ResolveToken::parse(&json!({"token": secret})).unwrap(),
            now,
        )
        .await
        .unwrap_err();
        assert_eq!(
            (mismatched.status, mismatched.detail),
            (wrong.status, wrong.detail)
        );
        drop(connection);
        f.finish().await;
    }

    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t5_c03() {
        // New organization, user, initial status, referral and audit commit
        // together; an injected failure leaves nothing behind.
        let mut f = Fixture::new().await;
        let sql = "CREATE FUNCTION refuse_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected audit refusal' USING ERRCODE='23514'; END $$; \
                   CREATE TRIGGER refuse_audit BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION refuse_audit();";
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&mut f.admin)
            .await
            .unwrap();
        let error = f
            .create(body(&new_organization_payload()), instant())
            .await
            .unwrap_err();
        assert_eq!(error.status.as_u16(), 500);
        for table in [
            "organizations",
            "users",
            "referrals",
            "user_status_transitions",
            "audit_logs",
        ] {
            let count: i64 =
                sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
                    .fetch_one(&f.pool)
                    .await
                    .unwrap();
            let expected = i64::from(matches!(table, "organizations" | "users"));
            assert_eq!(count, expected, "{table}");
        }
        f.finish().await;
    }

    fn app(f: &Fixture, limits: auth::limits::SharedLimits) -> axum::Router {
        let health = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&std::env::var("RUST_TEST_DATABASE_URL").unwrap())
            .unwrap();
        let state = AppState::new(
            Config {
                kyc_execution_gate_enabled: true,
                environment: "test".into(),
                release_sha: verdaxis_read::compiled_sha().into(),
                migration_revision: "kycg_20261014_staging".into(),
                database_url: std::env::var("RUST_TEST_DATABASE_URL").unwrap(),
                db_session_time_zone: "UTC".into(),
                prepared_budget: None,
                bind: "127.0.0.1:0".parse().unwrap(),
                read_admission_slots: 4,
                pool_max: 5,
                python_budget: 20,
                configured_max_connections: 100,
            },
            runtime::db::Pools {
                read: f.pool.clone(),
                health,
            },
            MarketPolicy {
                source_sha: verdaxis_read::compiled_sha().into(),
                products: vec![],
                points: vec![],
            },
        )
        .unwrap();
        let group = request::router(
            state.clone(),
            VerificationSettings::new(
                "synthetic-test-secret-at-least-32-bytes".into(),
                None,
                "test".into(),
                "test".into(),
            ),
            limits,
            request::InvitationService {
                pool: f.pool.clone(),
                domains: domains(),
                frontend_url: "https://app.example".into(),
            },
        );
        group.with_state(state)
    }

    fn token(user: Uuid) -> String {
        let now = Utc::now();
        jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &json!({"sub":user,"type":"access","iat":now.timestamp(),"exp":now.timestamp()+900,
                    "iss":"test","aud":"test"}),
            &jsonwebtoken::EncodingKey::from_secret(b"synthetic-test-secret-at-least-32-bytes"),
        )
        .unwrap()
    }

    async fn send(
        app: &axum::Router,
        method: &str,
        path: &str,
        credential: Option<&str>,
        body: Option<Value>,
    ) -> (u16, Option<u64>, Value) {
        let mut builder = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some(credential) = credential {
            builder = builder.header("authorization", format!("Bearer {credential}"));
        }
        let request = builder
            .body(axum::body::Body::from(
                body.map(|value| value.to_string()).unwrap_or_default(),
            ))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok());
        let truncated = response
            .headers()
            .get("x-result-truncated")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = axum::body::to_bytes(response.into_body(), 200_000)
            .await
            .unwrap();
        let mut value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if let (Some(flag), Some(object)) = (truncated, value.as_object_mut()) {
            object.insert("truncated_header".into(), json!(flag));
        }
        (status, retry_after, value)
    }

    #[tokio::test]
    #[ignore = "requires explicit disposable PostgreSQL authority"]
    async fn p4_t5_c73() {
        // Route order: account/role authority precedes quota and every target
        // read; a public route keeps its own declared limit.
        let f = Fixture::new().await;
        let member = Uuid::new_v4();
        sqlx::query("INSERT INTO users(id,email,password_hash,role,status,email_verified,organization_id) VALUES($1,'member@customer.example','synthetic','BUYER','APPROVED',true,$2)")
            .bind(member)
            .bind(f.organization)
            .execute(&f.pool)
            .await
            .unwrap();
        let member_token = token(member);
        let admin_token = token(f.actor);

        let router = app(&f, auth::limits::SharedLimits::default());
        // A04 and A05 refuse an anonymous caller and a non-administrator.
        for (method, path, body) in [
            ("GET", request::LIST_PATH, None),
            (
                "POST",
                request::CREATE_PATH,
                Some(existing_payload(f.organization)),
            ),
        ] {
            assert_eq!(send(&router, method, path, None, body.clone()).await.0, 401);
            assert_eq!(
                send(&router, method, path, Some(&member_token), body)
                    .await
                    .0,
                403
            );
        }
        // Sixty non-administrator refusals consume no administrator quota.
        for _ in 0..60 {
            assert_eq!(
                send(
                    &router,
                    "POST",
                    request::CREATE_PATH,
                    Some(&member_token),
                    Some(existing_payload(f.organization))
                )
                .await
                .0,
                403
            );
        }
        let (status, _, body) = send(
            &router,
            "POST",
            request::CREATE_PATH,
            Some(&admin_token),
            Some(existing_payload(f.organization)),
        )
        .await;
        assert_eq!(status, 201, "{body}");
        assert_eq!(body["role"], "SUPPLIER");
        assert_eq!(body["reissued"], false);
        assert_eq!(body["organization_name"], "Goldwind Green Methanol");
        assert!(body["acceptance_url"].as_str().unwrap().contains("#token="));
        let (status, _, body) =
            send(&router, "GET", request::LIST_PATH, Some(&admin_token), None).await;
        assert_eq!(status, 200);
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(body["truncated"], false);
        assert_eq!(body["truncated_header"], "false");
        // A05's 30/minute limit is per declared route.
        let mut last = 0;
        for _ in 0..30 {
            last = send(
                &router,
                "POST",
                request::CREATE_PATH,
                Some(&admin_token),
                Some(existing_payload(f.organization)),
            )
            .await
            .0;
        }
        assert_eq!(last, 429);
        let (status, retry_after, body) = send(
            &router,
            "POST",
            request::CREATE_PATH,
            Some(&admin_token),
            Some(existing_payload(f.organization)),
        )
        .await;
        assert_eq!(status, 429);
        assert!(retry_after.unwrap_or(0) > 0);
        assert_eq!(body["detail"]["code"], "RATE_LIMITED");

        // A06 is public: one refusal shape, then the declared 20/minute.
        let router = app(&f, auth::limits::SharedLimits::default());
        let mut refusals = Vec::new();
        for _ in 0..20 {
            let (status, _, body) = send(
                &router,
                "POST",
                request::RESOLVE_PATH,
                None,
                Some(json!({"token": "z".repeat(43)})),
            )
            .await;
            refusals.push((status, body));
        }
        assert!(refusals.iter().all(|(status, _)| *status == 400));
        assert!(
            refusals
                .iter()
                .all(|(_, body)| body["detail"]["code"] == "INVITATION_INVALID")
        );
        let (status, retry_after, _) = send(
            &router,
            "POST",
            request::RESOLVE_PATH,
            None,
            Some(json!({"token": "z".repeat(43)})),
        )
        .await;
        assert_eq!(status, 429);
        assert!(retry_after.unwrap_or(0) > 0);
        f.finish().await;
    }
}
