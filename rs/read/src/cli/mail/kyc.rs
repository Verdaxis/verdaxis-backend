//! KYC grace notice mail: a permanent, at-most-once claim per grace row.
//!
//! Durable identity is `kyc_grace_notices.grace_id`. The claim commits before
//! the provider call and is never released, so an interrupted or ambiguous send
//! is never repeated automatically. An operator reviews a claim whose
//! `sent_at` stays empty. There is no retry queue for this contract.
//!
//! Selection is narrow: a notice goes to the user of the grace row, and only
//! while that user still depends on the grace. The same eligibility text runs
//! when a page is read and again inside the claim statement, so a user who is
//! rejected or moves organization between the two is not claimed.
use super::{Delivery, MailTransport};
use crate::cli::Exit;
use crate::cli::context::RunContext;
use crate::cli::gate::require_target;
use crate::crud::referrals::invite::MailClient;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

/// Rows selected per page.
pub const PAGE_SIZE: u32 = 200;
/// Rows one invocation may attempt in total.
pub const MAX_PER_INVOCATION: u32 = 1000;

const SUPPORT_ADDRESS: &str = "admin@verdaxis.exchange";
const SUBJECT: &str = "Complete your KYC to keep trading on Verdaxis";
const LABEL: &str = "KYC VERIFICATION";
const ACTION: &str = "Complete KYC";
/// The shared mail layout used by the invitation mail.
const TEMPLATE: &str = include_str!("../../crud/referrals/invite.html");

/// The user still depends on the grace row (`g` is the grace row, `u` its user):
/// - the grace has not expired;
/// - the user is not rejected;
/// - the user is still bound to the organization the grace was created for
///   (a user with no organization is not bound);
/// - the user is not approved for the current organization. An approved user
///   without an organization-bound review, or bound to another organization,
///   keeps the notice because access still depends on grace.
macro_rules! eligible {
    () => {
        "g.expires_at > now()
         AND u.kyc_status <> 'REJECTED'
         AND u.organization_id = g.organization_id
         AND (u.kyc_status <> 'APPROVED'
              OR u.kyc_organization_id IS DISTINCT FROM u.organization_id)"
    };
}

const PAGE_SQL: &str = concat!(
    "SELECT g.id FROM kyc_execution_grace g JOIN users u ON u.id = g.user_id
     WHERE NOT EXISTS (SELECT 1 FROM kyc_grace_notices n WHERE n.grace_id = g.id)
       AND ",
    eligible!(),
    " ORDER BY g.id LIMIT $1"
);

/// Claim and read the recipient in one statement. The claim row is inserted
/// only when the eligibility text still holds, and the address and deadline
/// come from the same snapshot.
const CLAIM_SQL: &str = concat!(
    "WITH claim AS (
        INSERT INTO kyc_grace_notices (grace_id, claimed_at)
        SELECT g.id, now()
        FROM kyc_execution_grace g JOIN users u ON u.id = g.user_id
        WHERE g.id = $1 AND ",
    eligible!(),
    " ON CONFLICT (grace_id) DO NOTHING
        RETURNING grace_id)
     SELECT c.grace_id, g.expires_at, u.email
     FROM claim c
     JOIN kyc_execution_grace g ON g.id = c.grace_id
     JOIN users u ON u.id = g.user_id"
);

/// What one page did. `failed` counts claimed notices whose provider result is
/// unknown or negative: each keeps its claim and needs operator review.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct KycCounts {
    pub sent: u64,
    pub failed: u64,
    pub skipped: u64,
}

impl KycCounts {
    pub fn attempted(&self) -> u64 {
        self.sent + self.failed + self.skipped
    }

    pub fn add(&mut self, other: KycCounts) {
        self.sent += other.sent;
        self.failed += other.failed;
        self.skipped += other.skipped;
    }
}

/// A page that stopped on an error, with what it had done before the error.
///
/// `counts.failed > 0` means a claimed notice has an unknown or refused
/// provider result, whatever the error was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageError {
    pub exit: Exit,
    pub counts: KycCounts,
}

impl PageError {
    fn new(exit: Exit, counts: KycCounts) -> Self {
        Self { exit, counts }
    }
}

/// A claim this process now owns, with the recipient read in the same statement.
pub struct ClaimedNotice {
    pub grace_id: Uuid,
    pub email: String,
    pub expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for ClaimedNotice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimedNotice")
            .field("grace_id", &self.grace_id)
            .field("email", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// The provider key. One key per grace row lets the provider deduplicate a
/// replay it can still see.
pub fn idempotency_key(grace_id: Uuid) -> String {
    format!("kyc-grace-{grace_id}")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Build the provider payload. The deadline is always written in UTC.
pub fn render_payload(sender: &str, frontend: &str, to: &str, expires_at: DateTime<Utc>) -> Value {
    let deadline = expires_at.format("%d %B %Y %H:%M UTC").to_string();
    let body = format!(
        "<p>Complete KYC by {} to keep trading on Verdaxis.</p>",
        escape(&deadline)
    );
    let url = escape(&format!("{}/kyc", frontend.trim_end_matches('/')));
    let values = [
        ("TITLE", escape(SUBJECT)),
        ("PREVIEW", escape(SUBJECT)),
        ("LABEL", LABEL.to_owned()),
        ("BODY", body),
        ("URL", url),
        ("ACTION", ACTION.to_owned()),
        ("SUPPORT", SUPPORT_ADDRESS.to_owned()),
    ];
    // One replacement pass: inserted text cannot introduce another token.
    let mut html = String::new();
    let mut rest = TEMPLATE;
    while let Some((prefix, tail)) = rest.split_once("{{") {
        html.push_str(prefix);
        let (key, suffix) = tail.split_once("}}").expect("static mail template");
        html.push_str(
            &values
                .iter()
                .find(|(name, _)| *name == key)
                .expect("static mail token")
                .1,
        );
        rest = suffix;
    }
    html.push_str(rest);
    json!({
        "from": sender,
        "reply_to": SUPPORT_ADDRESS,
        "to": [to],
        "subject": SUBJECT,
        "html": html,
    })
}

/// The next unclaimed, still-eligible grace ids in ascending order.
pub async fn eligible_page(conn: &mut PgConnection, limit: u32) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(PAGE_SQL)
        .bind(i64::from(limit))
        .fetch_all(conn)
        .await
}

/// Claim one grace row. `None` means another process holds the claim or the
/// user is no longer eligible. Autocommit: the claim is durable on return.
pub async fn claim(
    conn: &mut PgConnection,
    grace_id: Uuid,
) -> Result<Option<ClaimedNotice>, sqlx::Error> {
    let row = sqlx::query(CLAIM_SQL)
        .bind(grace_id)
        .fetch_optional(conn)
        .await?;
    Ok(row.map(|row| ClaimedNotice {
        grace_id: row.get("grace_id"),
        email: row.get("email"),
        expires_at: row.get("expires_at"),
    }))
}

/// Record an accepted send. Runs only after the provider accepted the message.
async fn record_sent(conn: &mut PgConnection, grace_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE kyc_grace_notices SET sent_at = now() WHERE grace_id = $1 AND sent_at IS NULL",
    )
    .bind(grace_id)
    .execute(conn)
    .await
    .map(|_| ())
}

/// The notice sender: the shared keyed mail transport plus the message
/// settings and the database the job attests before its first claim.
pub struct KycNotice {
    mail: MailTransport,
    sender: String,
    frontend: String,
    database: String,
}

impl KycNotice {
    pub fn new(mail: MailClient, sender: String, frontend: String, database: String) -> Self {
        Self {
            mail: MailTransport::new(mail),
            sender,
            frontend,
            database,
        }
    }

    /// The one place a notice reaches the provider. The key is stable per
    /// grace row, which is one per user and organization, so the provider can
    /// deduplicate a replay on its side while the claim row stops ours.
    async fn deliver(&self, recipient: &str, payload: &Value, key: &str) -> Delivery {
        self.mail.send_keyed(recipient, payload, key).await
    }

    /// Claim and send up to `limit` notices (at most [`PAGE_SIZE`]).
    ///
    /// One connection serves the whole page: it is attested against the named
    /// database and the deployed role before the first claim, and no
    /// transaction or row lock is open while the provider is called.
    pub async fn send_page(&self, ctx: &RunContext, limit: u32) -> Result<KycCounts, PageError> {
        let limit = limit.clamp(1, PAGE_SIZE);
        let mut counts = KycCounts::default();
        let mut conn = ctx
            .pool
            .acquire()
            .await
            .map_err(|_| PageError::new(Exit::Dependency, counts))?;
        require_target(&mut conn, ctx.environment, &self.database)
            .await
            .map_err(|exit| PageError::new(exit, counts))?;
        let page = eligible_page(&mut conn, limit)
            .await
            .map_err(|_| PageError::new(Exit::Dependency, counts))?;
        for grace_id in page {
            let Some(notice) = claim(&mut conn, grace_id)
                .await
                .map_err(|_| PageError::new(Exit::Dependency, counts))?
            else {
                counts.skipped += 1;
                continue;
            };
            let payload = render_payload(
                &self.sender,
                &self.frontend,
                &notice.email,
                notice.expires_at,
            );
            let delivery = self
                .deliver(&notice.email, &payload, &idempotency_key(grace_id))
                .await;
            let accepted = delivery == Delivery::Sent;
            // An accepted send whose record fails is still claimed and never
            // repeated; count it for review like an unaccepted one.
            if accepted && record_sent(&mut conn, grace_id).await.is_ok() {
                counts.sent += 1;
            } else {
                counts.failed += 1;
            }
        }
        Ok(counts)
    }
}
