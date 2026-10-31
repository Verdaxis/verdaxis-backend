//! R02 referral statistics, R03 platform leaderboard and R05 public code
//! resolution.
//!
//! Source: `app/routers/referrals.py:73-241`.
//!
//! The two projections that Python builds in a second pass (list items and
//! ranked leaderboard entries) are pure functions over already-fetched rows so
//! they can be tested without a database. The query functions only assemble
//! those rows and hand them over.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::ApiError;
use crate::Json;
use crate::routes::ports::{pydantic_datetime, query_error};

const MY_REFERRALS_PATH: &str = "/api/referrals/my-referrals";
const LEADERBOARD_PATH: &str = "/api/referrals/leaderboard";
const RESOLVE_PATH: &str = "/api/referrals/resolve/{code}";

/// Python's `_display_name`: the two name parts joined by one space, trimmed,
/// then replaced by `Anonymous` when nothing remains.
pub fn display_name(first_name: Option<&str>, last_name: Option<&str>) -> String {
    let joined = format!("{} {}", first_name.unwrap_or(""), last_name.unwrap_or(""));
    let trimmed = python_strip(&joined);
    if trimmed.is_empty() {
        "Anonymous".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Python `str.strip()` with no argument.
///
/// `str.isspace()` is true for Unicode whitespace, which `char::is_whitespace`
/// also covers, plus the four C0 separators U+001C..=U+001F, which it does not.
fn python_strip(text: &str) -> &str {
    text.trim_matches(|character: char| {
        character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
    })
}

/// Match Python `re.compile(r"^VDX-[A-Z0-9]{6}$").match(code)`.
///
/// `match` anchors only the start, and Python's `$` also matches immediately
/// before one trailing newline, so `"VDX-ABC123\n"` is accepted. A Rust `$`
/// would not match there, so the grammar is checked character by character
/// instead of with a pattern whose meaning differs.
pub fn code_matches_python_format(code: &str) -> bool {
    let body = code.strip_suffix('\n').unwrap_or(code);
    let mut characters = body.chars();
    for expected in "VDX-".chars() {
        if characters.next() != Some(expected) {
            return false;
        }
    }
    let mut digits = 0;
    for character in characters {
        if !(character.is_ascii_uppercase() || character.is_ascii_digit()) {
            return false;
        }
        digits += 1;
    }
    digits == 6
}

/// One `referrals` row joined to the referred account and its organization.
///
/// `referred_user_id` is a unique foreign key, so Python's `selectinload`
/// always finds the account; the join is still a left join so a row whose
/// account is absent cannot abort the whole list.
#[derive(sqlx::FromRow)]
pub struct MyReferralRow {
    pub organization_name: Option<String>,
    pub role: Option<String>,
    pub status: String,
    pub signed_up_at: DateTime<Utc>,
}

/// One aggregated referrer count.
///
/// Python counts `Referral.id`, which is a non-null primary key, so the count
/// is the number of referrals for that referrer.
#[derive(sqlx::FromRow)]
pub struct ReferrerCount {
    pub referrer_id: Uuid,
    pub referral_count: i64,
}

/// One leaderboard account with its optional organization name.
#[derive(sqlx::FromRow)]
pub struct LeaderboardUser {
    pub id: Uuid,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub organization_name: Option<String>,
}

/// One account matched by a referral code, or no row for an unknown code.
#[derive(sqlx::FromRow)]
pub struct ResolveRow {
    pub role: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub organization_name: Option<String>,
    pub organization_type: Option<String>,
}

/// Build the `ReferralStatsResponse` body.
///
/// `VERIFIED` and `ACTIVE` both count as verified; only `ACTIVE` counts as
/// active. Only four item fields are disclosed: no email and no identifiers.
pub fn referral_stats(rows: &[MyReferralRow]) -> Json {
    let verified = rows
        .iter()
        .filter(|row| matches!(row.status.as_str(), "VERIFIED" | "ACTIVE"))
        .count();
    let active = rows.iter().filter(|row| row.status == "ACTIVE").count();
    let referrals: Vec<Json> = rows
        .iter()
        .map(|row| {
            json!({
                "organization_name": row.organization_name,
                "role": row.role,
                "status": row.status,
                "signed_up_at": pydantic_datetime(row.signed_up_at),
            })
        })
        .collect();
    json!({
        "total": rows.len(),
        "verified": verified,
        "active": active,
        "referrals": referrals,
    })
}

/// Build the leaderboard body, preserving the aggregate order and the gaps a
/// missing account leaves behind.
///
/// Rank starts at 1 and advances only for an entry that is actually appended,
/// exactly like Python's loop. Rows with equal counts keep their own ranks.
pub fn leaderboard_entries(
    counts: &[ReferrerCount],
    users: &HashMap<Uuid, LeaderboardUser>,
) -> Json {
    let mut entries = Vec::new();
    let mut rank = 1_u64;
    for row in counts {
        let Some(user) = users.get(&row.referrer_id) else {
            continue;
        };
        entries.push(json!({
            "rank": rank,
            "user_name": display_name(user.first_name.as_deref(), user.last_name.as_deref()),
            "organization_name": user.organization_name,
            "referral_count": row.referral_count,
        }));
        rank += 1;
    }
    Json::Array(entries)
}

/// Build the `ResolveCodeResponse` body for a matched account or no match.
///
/// An administrative account keeps its own label and discloses no person and no
/// organization type. Any other account discloses its display name unless the
/// name fell back to `Anonymous`.
pub fn resolve_response(user: Option<&ResolveRow>) -> Json {
    let Some(user) = user else {
        return resolve_none();
    };
    if user.role.as_deref() == Some("ADMIN") {
        return json!({
            "valid": true,
            "organization_name": "Verdaxis Exchange",
            "organization_type": Value::Null,
            "referrer_name": Value::Null,
        });
    }
    let name = display_name(user.first_name.as_deref(), user.last_name.as_deref());
    json!({
        "valid": true,
        "organization_name": user.organization_name,
        "organization_type": user.organization_type,
        "referrer_name": if name == "Anonymous" {
            Value::Null
        } else {
            Value::String(name)
        },
    })
}

fn resolve_none() -> Json {
    json!({
        "valid": false,
        "organization_name": Value::Null,
        "organization_type": Value::Null,
        "referrer_name": Value::Null,
    })
}

/// R02: every referral made by the actor, newest first.
///
/// Keep the source's timestamp-only ordering. The comparator permits only the
/// reviewed equal-timestamp item permutation.
pub(crate) async fn select_my_referrals(
    connection: &mut PgConnection,
    actor: Uuid,
) -> Result<Json, ApiError> {
    let rows = sqlx::query_as::<_, MyReferralRow>(
        "SELECT o.name AS organization_name, u.role::text AS role, r.status::text AS status, \
                r.created_at AS signed_up_at \
         FROM referrals r \
         LEFT JOIN users u ON u.id = r.referred_user_id \
         LEFT JOIN organizations o ON o.id = u.organization_id \
         WHERE r.referrer_id = $1 \
         ORDER BY r.created_at DESC",
    )
    .bind(actor)
    .fetch_all(&mut *connection)
    .await
    .map_err(|error| query_error(MY_REFERRALS_PATH, &error))?;
    Ok(referral_stats(&rows))
}

/// R03: the ten largest referrer counts across the platform.
///
/// Resolve equal counts by referrer identity before applying the limit.
pub(crate) async fn select_leaderboard(connection: &mut PgConnection) -> Result<Json, ApiError> {
    let counts = sqlx::query_as::<_, ReferrerCount>(
        "SELECT referrer_id, count(*)::bigint AS referral_count \
         FROM referrals GROUP BY referrer_id \
         ORDER BY count(*) DESC, referrer_id ASC LIMIT 10",
    )
    .fetch_all(&mut *connection)
    .await
    .map_err(|error| query_error(LEADERBOARD_PATH, &error))?;
    if counts.is_empty() {
        return Ok(Json::Array(Vec::new()));
    }
    let ids: Vec<Uuid> = counts.iter().map(|row| row.referrer_id).collect();
    let users = sqlx::query_as::<_, LeaderboardUser>(
        "SELECT u.id, u.first_name, u.last_name, o.name AS organization_name \
         FROM users u LEFT JOIN organizations o ON o.id = u.organization_id \
         WHERE u.id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut *connection)
    .await
    .map_err(|error| query_error(LEADERBOARD_PATH, &error))?;
    let users = users
        .into_iter()
        .map(|user| (user.id, user))
        .collect::<HashMap<_, _>>();
    Ok(leaderboard_entries(&counts, &users))
}

/// R05: resolve a referral code. An invalid grammar never reaches the database.
///
/// The query binds the code exactly as received, including a trailing newline,
/// which is what Python does after its format check accepts `"VDX-ABC123\n"`.
pub(crate) async fn select_resolve(
    connection: &mut PgConnection,
    code: &str,
) -> Result<Json, ApiError> {
    if !code_matches_python_format(code) {
        return Ok(resolve_response(None));
    }
    let row = sqlx::query_as::<_, ResolveRow>(
        "SELECT u.role::text AS role, u.first_name, u.last_name, \
                o.name AS organization_name, o.type::text AS organization_type \
         FROM users u \
         LEFT JOIN organizations o ON o.id = u.organization_id \
         WHERE u.referral_code = $1",
    )
    .bind(code)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|error| query_error(RESOLVE_PATH, &error))?;
    Ok(resolve_response(row.as_ref()))
}
