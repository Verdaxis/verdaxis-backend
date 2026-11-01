//! Read-only inspection commands: `ops inspect users` and `ops inspect blockers`.
//!
//! Both commands only read. Every query runs inside one transaction opened by
//! [`begin_read_only`]: PostgreSQL refuses a write in that transaction even
//! for a role that holds write privileges, and the transaction is always rolled
//! back. The commands open no file and write no status file.
//!
//! `users` replaces `scripts/check_users.py`, which printed every address in
//! clear text (`scripts/check_users.py:6-15`). Here the local part is masked
//! unless the operator passes `--show-emails`.
//!
//! `blockers` replaces `scripts/security_preflight.py` (`report`, lines 12-175):
//! the same counts, the same predicates, the same line format and the same
//! exit code 2 when a blocker count is nonzero.
use super::super::context::RunContext;
use super::super::gate::require_identity;
use super::super::{Exit, InspectArgs, InspectKind};
use crate::analytics::usage::DEMO_MARKET_ORGANIZATION_IDS;
use serde::Serialize;
use sqlx::{PgConnection, PgPool, Postgres, Row, Transaction};
use std::io::Write;
use uuid::Uuid;

/// The most users `inspect users` lists. The report always gives the true total.
pub const USER_LISTING_LIMIT: i64 = 1_000;
/// The most ineligible users `inspect blockers` lists. The count is always exact.
pub const BLOCKER_LISTING_LIMIT: i64 = 200;

/// Server-side bounds for each inspection transaction.
const BEGIN_READ_ONLY: &str = "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY";
const SET_STATEMENT_TIMEOUT: &str = "SET LOCAL statement_timeout = '30s'";
const SET_LOCK_TIMEOUT: &str = "SET LOCAL lock_timeout = '5s'";

/// Organizations the preflight treats as synthetic test data. The numbers are
/// written exactly as `app/demo_identities.py` formats them: the last UUID
/// group holds the decimal digits 1-4 and 11-14, not hexadecimal values.
pub const KNOWN_TEST_ORGANIZATION_IDS: [Uuid; 8] = [
    Uuid::from_u128(0x9e63f7a1_0000_4000_8000_000000000001),
    Uuid::from_u128(0x9e63f7a1_0000_4000_8000_000000000002),
    Uuid::from_u128(0x9e63f7a1_0000_4000_8000_000000000003),
    Uuid::from_u128(0x9e63f7a1_0000_4000_8000_000000000004),
    Uuid::from_u128(0x9e63f7a1_0000_4000_8000_000000000011),
    Uuid::from_u128(0x9e63f7a1_0000_4000_8000_000000000012),
    Uuid::from_u128(0x9e63f7a1_0000_4000_8000_000000000013),
    Uuid::from_u128(0x9e63f7a1_0000_4000_8000_000000000014),
];

const KYC_ENFORCEMENT: &str = "ADVISORY_HELD";
const LEGACY_OWNER_POLICY: &str = "exact_synthetic_registry_or_explicit_owner_decision";

/// Open the only kind of transaction these commands use.
///
/// Repeatable read gives every count in one report the same snapshot. The
/// read-only flag makes PostgreSQL refuse any write for this transaction.
pub async fn begin_read_only(pool: &PgPool) -> Result<Transaction<'static, Postgres>, Exit> {
    let mut transaction = pool
        .begin_with(BEGIN_READ_ONLY)
        .await
        .map_err(|_| Exit::Dependency)?;
    for setting in [SET_STATEMENT_TIMEOUT, SET_LOCK_TIMEOUT] {
        sqlx::query(setting)
            .execute(&mut *transaction)
            .await
            .map_err(|_| Exit::Dependency)?;
    }
    Ok(transaction)
}

// ---------------------------------------------------------------------------
// Email masking and text safety.
// ---------------------------------------------------------------------------

/// Mask an address as `a***@domain`: the first character of the local part,
/// three stars, and the whole domain. An address without `@` becomes `***`.
///
/// The domain is everything after the last `@`, so a quoted local part that
/// contains `@` still hides its tail. The first character is one Unicode scalar
/// value; a whitespace or control character there is shown as `?`.
pub fn mask_email(address: &str) -> String {
    let Some((local, domain)) = address.rsplit_once('@') else {
        return "***".to_owned();
    };
    let first = local
        .chars()
        .next()
        .map(|character| {
            if character.is_control() || character.is_whitespace() {
                '?'
            } else {
                character
            }
        })
        .map(String::from)
        .unwrap_or_default();
    format!("{first}***@{}", visible(domain))
}

/// Replace control characters so a stored value cannot move the cursor or
/// start an escape sequence on the operator's terminal.
fn visible(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                '?'
            } else {
                character
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// `inspect users`.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserEntry {
    pub id: Uuid,
    /// Masked unless the operator asked for full addresses.
    pub email: String,
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsersReport {
    pub report: &'static str,
    /// Every user in the table, also when the listing is cut short.
    pub total: i64,
    pub listed: usize,
    pub truncated: bool,
    /// `masked` or `full`.
    pub emails: &'static str,
    pub users: Vec<UserEntry>,
}

impl UsersReport {
    /// Build the report from fetched rows. Masking happens here, so no caller
    /// can print an address that this function did not choose to reveal.
    pub fn new(rows: Vec<(Uuid, String, Option<String>)>, total: i64, show_emails: bool) -> Self {
        let users: Vec<UserEntry> = rows
            .into_iter()
            .map(|(id, email, role)| UserEntry {
                id,
                email: if show_emails {
                    email
                } else {
                    mask_email(&email)
                },
                role,
            })
            .collect();
        Self {
            report: "users",
            total,
            listed: users.len(),
            truncated: total > users.len() as i64,
            emails: if show_emails { "full" } else { "masked" },
            users,
        }
    }

    /// The text form follows `scripts/check_users.py:11-13`, with the stored
    /// role value and the user id added.
    pub fn render_text(&self) -> String {
        let mut text = format!("Found {} users:\n", self.total);
        for user in &self.users {
            text.push_str(&format!(
                " - {} (Role: {}) id={}\n",
                visible(&user.email),
                user.role.as_deref().unwrap_or("None"),
                user.id
            ));
        }
        if self.truncated {
            text.push_str(&format!(
                "listing_truncated=true listed={} total={}\n",
                self.listed, self.total
            ));
        }
        text
    }
}

/// Read the users in a stable order. Run it on a transaction from
/// [`begin_read_only`].
pub async fn collect_users(
    connection: &mut PgConnection,
    show_emails: bool,
) -> Result<UsersReport, Exit> {
    let rows = sqlx::query(
        "SELECT id, email, role, count(*) OVER () AS total \
         FROM users ORDER BY created_at, id LIMIT $1",
    )
    .bind(USER_LISTING_LIMIT)
    .fetch_all(&mut *connection)
    .await
    .map_err(|_| Exit::Dependency)?;
    // With no row the window count is absent and the table has no user.
    let total = rows
        .first()
        .map(|row| row.try_get::<i64, _>("total"))
        .transpose()
        .map_err(|_| Exit::Dependency)?
        .unwrap_or(0);
    let mut users = Vec::with_capacity(rows.len());
    for row in rows {
        users.push((
            row.try_get::<Uuid, _>("id").map_err(|_| Exit::Dependency)?,
            row.try_get::<String, _>("email")
                .map_err(|_| Exit::Dependency)?,
            row.try_get::<Option<String>, _>("role")
                .map_err(|_| Exit::Dependency)?,
        ));
    }
    Ok(UsersReport::new(users, total, show_emails))
}

// ---------------------------------------------------------------------------
// `inspect blockers`.
// ---------------------------------------------------------------------------

/// One approved buyer or supplier who cannot yet act in the market.
/// Mirrors the `user=...` line of the Python report (lines 140-148). No
/// address, token or document id is part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IneligibleUser {
    pub id: Uuid,
    pub role: Option<String>,
    pub status: String,
    pub email_verified: bool,
    pub organization_id: Option<Uuid>,
    pub kyc_status: String,
}

/// Every count of the Python report, in its print order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BlockerCounts {
    pub ineligible_users: i64,
    pub duplicate_case_insensitive_emails: i64,
    pub legacy_orders_without_owner: i64,
    pub legacy_rfqs_without_owner: i64,
    pub legacy_quotes_without_owner: i64,
    pub legacy_trades_without_user_provenance: i64,
    pub legacy_negotiations_without_user_provenance: i64,
    pub legacy_negotiation_rounds_without_user_provenance: i64,
    pub approved_kyc_without_external_review_evidence: i64,
    pub pending_organization_join_reviews: i64,
    pub outstanding_legacy_orders_without_owner: i64,
    pub outstanding_legacy_rfqs_without_owner: i64,
    pub outstanding_legacy_quotes_without_owner: i64,
    pub outstanding_legacy_trades_without_user_provenance: i64,
    pub outstanding_legacy_negotiations_without_user_provenance: i64,
}

impl BlockerCounts {
    /// The counts that block enforcement: the two identity counts and the five
    /// outstanding legacy counts (`scripts/security_preflight.py:129-137`).
    /// The other counts are reported for the operator and never block.
    pub fn blockers(&self) -> [(&'static str, i64); 7] {
        [
            ("ineligible_users", self.ineligible_users),
            (
                "duplicate_case_insensitive_emails",
                self.duplicate_case_insensitive_emails,
            ),
            (
                "outstanding_legacy_orders_without_owner",
                self.outstanding_legacy_orders_without_owner,
            ),
            (
                "outstanding_legacy_rfqs_without_owner",
                self.outstanding_legacy_rfqs_without_owner,
            ),
            (
                "outstanding_legacy_quotes_without_owner",
                self.outstanding_legacy_quotes_without_owner,
            ),
            (
                "outstanding_legacy_trades_without_user_provenance",
                self.outstanding_legacy_trades_without_user_provenance,
            ),
            (
                "outstanding_legacy_negotiations_without_user_provenance",
                self.outstanding_legacy_negotiations_without_user_provenance,
            ),
        ]
    }

    pub fn blocked(&self) -> bool {
        self.blockers().iter().any(|(_, count)| *count > 0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockersReport {
    pub report: &'static str,
    pub enforcement_preflight: &'static str,
    pub kyc_enforcement: &'static str,
    pub legacy_owner_policy: &'static str,
    pub counts: BlockerCounts,
    pub ineligible_users: Vec<IneligibleUser>,
    /// True when more ineligible users exist than the listing holds.
    pub ineligible_users_truncated: bool,
}

impl BlockersReport {
    pub fn new(counts: BlockerCounts, ineligible_users: Vec<IneligibleUser>) -> Self {
        let blocked = counts.blocked();
        Self {
            report: "blockers",
            enforcement_preflight: if blocked { "BLOCKED" } else { "READY" },
            kyc_enforcement: KYC_ENFORCEMENT,
            legacy_owner_policy: LEGACY_OWNER_POLICY,
            ineligible_users_truncated: counts.ineligible_users > ineligible_users.len() as i64,
            counts,
            ineligible_users,
        }
    }

    /// Exit 2 when any blocker count is nonzero, as the Python report does
    /// (`scripts/security_preflight.py:20-21`); the operator runbook tests for it.
    pub fn exit(&self) -> Exit {
        if self.counts.blocked() {
            Exit::Usage
        } else {
            Exit::Ok
        }
    }

    /// The text form keeps the line names and order of
    /// `scripts/security_preflight.py:130-175`, which the operator runbook
    /// quotes. Booleans and absent values print as Python printed them.
    pub fn render_text(&self) -> String {
        let counts = &self.counts;
        let mut text = String::new();
        let mut line = |name: &str, value: &dyn std::fmt::Display| {
            text.push_str(&format!("{name}={value}\n"));
        };
        line("ineligible_users", &counts.ineligible_users);
        line(
            "duplicate_case_insensitive_emails",
            &counts.duplicate_case_insensitive_emails,
        );
        for user in &self.ineligible_users {
            let organization = user
                .organization_id
                .map_or_else(|| "None".to_owned(), |id| id.to_string());
            text.push_str(&format!(
                "user={} role={} status={} email_verified={} organization_id={} kyc_status={}\n",
                user.id,
                user.role.as_deref().unwrap_or("None"),
                visible(&user.status),
                if user.email_verified { "True" } else { "False" },
                organization,
                visible(&user.kyc_status),
            ));
        }
        let mut line = |name: &str, value: &dyn std::fmt::Display| {
            text.push_str(&format!("{name}={value}\n"));
        };
        if self.ineligible_users_truncated {
            line(
                "ineligible_users_listing_truncated",
                &self.ineligible_users.len(),
            );
        }
        line(
            "legacy_orders_without_owner",
            &counts.legacy_orders_without_owner,
        );
        line(
            "legacy_rfqs_without_owner",
            &counts.legacy_rfqs_without_owner,
        );
        line(
            "legacy_quotes_without_owner",
            &counts.legacy_quotes_without_owner,
        );
        line(
            "legacy_trades_without_user_provenance",
            &counts.legacy_trades_without_user_provenance,
        );
        line(
            "legacy_negotiations_without_user_provenance",
            &counts.legacy_negotiations_without_user_provenance,
        );
        line(
            "legacy_negotiation_rounds_without_user_provenance",
            &counts.legacy_negotiation_rounds_without_user_provenance,
        );
        line(
            "approved_kyc_without_external_review_evidence",
            &counts.approved_kyc_without_external_review_evidence,
        );
        line("kyc_enforcement", &self.kyc_enforcement);
        line(
            "pending_organization_join_reviews",
            &counts.pending_organization_join_reviews,
        );
        line(
            "outstanding_legacy_orders_without_owner",
            &counts.outstanding_legacy_orders_without_owner,
        );
        line(
            "outstanding_legacy_rfqs_without_owner",
            &counts.outstanding_legacy_rfqs_without_owner,
        );
        line(
            "outstanding_legacy_quotes_without_owner",
            &counts.outstanding_legacy_quotes_without_owner,
        );
        line(
            "outstanding_legacy_trades_without_user_provenance",
            &counts.outstanding_legacy_trades_without_user_provenance,
        );
        line(
            "outstanding_legacy_negotiations_without_user_provenance",
            &counts.outstanding_legacy_negotiations_without_user_provenance,
        );
        line("enforcement_preflight", &self.enforcement_preflight);
        line("legacy_owner_policy", &self.legacy_owner_policy);
        text
    }
}

// The predicates below are the SQL form of the Python report's ORM filters.
// `$1` is the demo organization registry and `$2` the known test registry.
// An outstanding row is executable and is not an exact synthetic pair: for a
// two-party row, both parties must sit in the same registry to be exempt.

/// The ineligible-user predicate, as a macro so it can sit inside a static
/// statement (`concat!` takes literals only).
macro_rules! ineligible_users_where {
    () => {
        "u.role IN ('BUYER', 'SUPPLIER') \
         AND u.status = 'APPROVED' \
         AND (u.email_verified IS NOT TRUE \
              OR u.organization_id IS NULL \
              OR o.id IS NULL \
              OR o.verification_status IS NULL \
              OR o.verification_status <> 'APPROVED')"
    };
}

const DUPLICATE_EMAILS: &str = "SELECT count(*) FROM (\
     SELECT 1 FROM users GROUP BY lower(trim(email)) HAVING count(*) > 1) AS duplicates";
const LEGACY_ORDERS: &str = "SELECT count(*) FROM orderbook_orders WHERE owner_user_id IS NULL";
const LEGACY_RFQS: &str = "SELECT count(*) FROM rfqs WHERE buyer_user_id IS NULL";
const LEGACY_QUOTES: &str = "SELECT count(*) FROM rfq_quotes WHERE seller_user_id IS NULL";
const LEGACY_TRADES: &str =
    "SELECT count(*) FROM trades WHERE buyer_user_id IS NULL OR seller_user_id IS NULL";
const LEGACY_NEGOTIATIONS: &str = "SELECT count(*) FROM negotiations \
     WHERE initiator_user_id IS NULL OR counterparty_user_id IS NULL";
const LEGACY_ROUNDS: &str =
    "SELECT count(*) FROM negotiation_rounds WHERE proposer_user_id IS NULL";
const APPROVED_KYC_WITHOUT_EVIDENCE: &str = "SELECT count(*) FROM users \
     WHERE kyc_status = 'APPROVED' \
       AND (kyc_external_evidence_reference IS NULL OR kyc_review_note IS NULL \
            OR kyc_reviewed_by IS NULL OR kyc_reviewed_at IS NULL)";
const PENDING_JOIN_REVIEWS: &str =
    "SELECT count(*) FROM organization_join_requests WHERE status = 'PENDING'";

const OUTSTANDING_ORDERS: &str = "SELECT count(*) FROM orderbook_orders \
     WHERE owner_user_id IS NULL AND status IN ('OPEN', 'PARTIALLY_FILLED') \
       AND NOT (organization_id = ANY($1) OR organization_id = ANY($2))";
const OUTSTANDING_RFQS: &str = "SELECT count(*) FROM rfqs \
     WHERE buyer_user_id IS NULL AND status IN ('OPEN', 'QUOTED') \
       AND NOT (buyer_org_id = ANY($1) OR buyer_org_id = ANY($2))";
const OUTSTANDING_QUOTES: &str = "SELECT count(*) FROM rfq_quotes q \
     JOIN rfqs r ON r.id = q.rfq_id \
     WHERE q.seller_user_id IS NULL AND q.status = 'PENDING' \
       AND NOT ((r.buyer_org_id = ANY($1) AND q.seller_org_id = ANY($1)) \
             OR (r.buyer_org_id = ANY($2) AND q.seller_org_id = ANY($2)))";
const OUTSTANDING_TRADES: &str = "SELECT count(*) FROM trades \
     WHERE (buyer_user_id IS NULL OR seller_user_id IS NULL) \
       AND status IN ('PENDING_CONFIRMATION', 'CONFIRMED', 'DELIVERED') \
       AND NOT ((buyer_id = ANY($1) AND seller_id = ANY($1)) \
             OR (buyer_id = ANY($2) AND seller_id = ANY($2)))";
const OUTSTANDING_NEGOTIATIONS: &str = "SELECT count(*) FROM negotiations \
     WHERE (initiator_user_id IS NULL OR counterparty_user_id IS NULL) \
       AND status IN ('OPEN', 'COUNTERED') \
       AND NOT ((initiator_org_id = ANY($1) AND counterparty_org_id = ANY($1)) \
             OR (initiator_org_id = ANY($2) AND counterparty_org_id = ANY($2)))";

/// Count rows with one fixed statement. `registries` selects whether the
/// statement takes the two organization registries.
async fn count(
    connection: &mut PgConnection,
    statement: &'static str,
    registries: Option<(&[Uuid], &[Uuid])>,
) -> Result<i64, Exit> {
    let query = sqlx::query_scalar::<_, i64>(statement);
    let query = match registries {
        Some((demo, test)) => query.bind(demo).bind(test),
        None => query,
    };
    query
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| Exit::Dependency)
}

/// Read every count and the bounded list of ineligible users. Run it on a
/// transaction from [`begin_read_only`] so all values share one snapshot.
pub async fn collect_blockers(connection: &mut PgConnection) -> Result<BlockersReport, Exit> {
    let demo = DEMO_MARKET_ORGANIZATION_IDS.to_vec();
    let test = KNOWN_TEST_ORGANIZATION_IDS.to_vec();
    let registries = Some((demo.as_slice(), test.as_slice()));

    // One statement gives both the exact count and the bounded listing.
    let rows = sqlx::query(concat!(
        "SELECT u.id, u.role, u.status, u.email_verified, u.organization_id, u.kyc_status, \
                count(*) OVER () AS total \
         FROM users u LEFT JOIN organizations o ON o.id = u.organization_id \
         WHERE ",
        ineligible_users_where!(),
        " ORDER BY u.id LIMIT $1"
    ))
    .bind(BLOCKER_LISTING_LIMIT)
    .fetch_all(&mut *connection)
    .await
    .map_err(|_| Exit::Dependency)?;
    let ineligible_total = rows
        .first()
        .map(|row| row.try_get::<i64, _>("total"))
        .transpose()
        .map_err(|_| Exit::Dependency)?
        .unwrap_or(0);
    let mut ineligible_users = Vec::with_capacity(rows.len());
    for row in &rows {
        let read = |_| Exit::Dependency;
        ineligible_users.push(IneligibleUser {
            id: row.try_get("id").map_err(read)?,
            role: row.try_get("role").map_err(read)?,
            status: row.try_get("status").map_err(read)?,
            email_verified: row.try_get("email_verified").map_err(read)?,
            organization_id: row.try_get("organization_id").map_err(read)?,
            kyc_status: row.try_get("kyc_status").map_err(read)?,
        });
    }

    let counts = BlockerCounts {
        ineligible_users: ineligible_total,
        duplicate_case_insensitive_emails: count(connection, DUPLICATE_EMAILS, None).await?,
        legacy_orders_without_owner: count(connection, LEGACY_ORDERS, None).await?,
        legacy_rfqs_without_owner: count(connection, LEGACY_RFQS, None).await?,
        legacy_quotes_without_owner: count(connection, LEGACY_QUOTES, None).await?,
        legacy_trades_without_user_provenance: count(connection, LEGACY_TRADES, None).await?,
        legacy_negotiations_without_user_provenance: count(connection, LEGACY_NEGOTIATIONS, None)
            .await?,
        legacy_negotiation_rounds_without_user_provenance: count(connection, LEGACY_ROUNDS, None)
            .await?,
        approved_kyc_without_external_review_evidence: count(
            connection,
            APPROVED_KYC_WITHOUT_EVIDENCE,
            None,
        )
        .await?,
        pending_organization_join_reviews: count(connection, PENDING_JOIN_REVIEWS, None).await?,
        outstanding_legacy_orders_without_owner: count(connection, OUTSTANDING_ORDERS, registries)
            .await?,
        outstanding_legacy_rfqs_without_owner: count(connection, OUTSTANDING_RFQS, registries)
            .await?,
        outstanding_legacy_quotes_without_owner: count(connection, OUTSTANDING_QUOTES, registries)
            .await?,
        outstanding_legacy_trades_without_user_provenance: count(
            connection,
            OUTSTANDING_TRADES,
            registries,
        )
        .await?,
        outstanding_legacy_negotiations_without_user_provenance: count(
            connection,
            OUTSTANDING_NEGOTIATIONS,
            registries,
        )
        .await?,
    };
    Ok(BlockersReport::new(counts, ineligible_users))
}

// ---------------------------------------------------------------------------
// Command entry points.
// ---------------------------------------------------------------------------

/// Run an inspection command on an opened context. The dispatcher calls this
/// once the context has passed the identity checks.
pub async fn run(context: &RunContext, args: &InspectArgs) -> Exit {
    if require_identity(
        context.environment.as_str(),
        &context.release_sha,
        args.identity.environment,
        &args.identity.release_sha,
    )
    .is_err()
    {
        summary(
            &mut std::io::stderr(),
            command_name(args),
            "identity_refused",
            serde_json::json!({}),
        );
        return Exit::Precondition;
    }
    execute(
        &context.pool,
        args,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
    .await
}

/// Run an inspection command on a pool. The report goes to `stdout`; one JSON
/// summary line (counts only, never an address) goes to `stderr`.
pub async fn execute(
    pool: &PgPool,
    args: &InspectArgs,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Exit {
    let command = command_name(args);
    // Only `users` can show an address; refuse the flag before reading anything.
    if args.show_emails && args.kind != InspectKind::Users {
        summary(stderr, command, "usage_refused", serde_json::json!({}));
        return Exit::Usage;
    }
    match inspect(pool, args).await {
        Ok(Inspection {
            body,
            outcome,
            counts,
            exit,
        }) => {
            let rendered = if args.json {
                format!("{body_json}\n", body_json = body.json)
            } else {
                body.text
            };
            if stdout.write_all(rendered.as_bytes()).is_err() || stdout.flush().is_err() {
                summary(stderr, command, "output_failed", serde_json::json!({}));
                return Exit::Dependency;
            }
            summary(stderr, command, outcome, counts);
            exit
        }
        Err(exit) => {
            summary(
                stderr,
                command,
                "dependency_unavailable",
                serde_json::json!({}),
            );
            exit
        }
    }
}

struct Rendered {
    text: String,
    json: String,
}

struct Inspection {
    body: Rendered,
    outcome: &'static str,
    counts: serde_json::Value,
    exit: Exit,
}

async fn inspect(pool: &PgPool, args: &InspectArgs) -> Result<Inspection, Exit> {
    let mut transaction = begin_read_only(pool).await?;
    let result = match args.kind {
        InspectKind::Users => {
            collect_users(&mut transaction, args.show_emails)
                .await
                .map(|report| Inspection {
                    counts: serde_json::json!({"users": report.total, "listed": report.listed}),
                    body: Rendered {
                        text: report.render_text(),
                        json: serde_json::to_string(&report).unwrap_or_default(),
                    },
                    outcome: "ok",
                    exit: Exit::Ok,
                })
        }
        InspectKind::Blockers => {
            collect_blockers(&mut transaction)
                .await
                .map(|report| Inspection {
                    counts: serde_json::to_value(&report.counts).unwrap_or_default(),
                    body: Rendered {
                        text: report.render_text(),
                        json: serde_json::to_string(&report).unwrap_or_default(),
                    },
                    outcome: if report.counts.blocked() {
                        "blocked"
                    } else {
                        "ok"
                    },
                    exit: report.exit(),
                })
        }
    };
    // The transaction never commits: it only ever held reads.
    let _ = transaction.rollback().await;
    result
}

fn command_name(args: &InspectArgs) -> &'static str {
    match args.kind {
        InspectKind::Users => "ops inspect users",
        InspectKind::Blockers => "ops inspect blockers",
    }
}

fn summary(stderr: &mut dyn Write, command: &str, outcome: &str, counts: serde_json::Value) {
    let line = serde_json::json!({"command": command, "outcome": outcome, "counts": counts});
    // A closed stderr must not change the command's result.
    let _ = writeln!(stderr, "{line}");
}
