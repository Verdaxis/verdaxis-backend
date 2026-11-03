//! `ops ingest signals`: trusted market-signal ingestion (Phase 10 Task 21),
//! replacing `scripts/ingest_market_signals.py` and
//! `app/services/market_signal_ingestion.py`.
//!
//! One CSV file of one signal family from one source becomes one verified
//! ingestion run whose rows the boards treat as real. It runs in staging only,
//! as the separate operator role (never the app role), and writes nothing
//! without `--apply`. Each row is validated as Python validated it; an invalid
//! row is reported and skipped, an existing `(source, source_event_id)` is a
//! skipped duplicate, and any other database error aborts the whole run. A run
//! with no inserted row is never kept.
//!
//! Stricter than Python (refuse, never store a different value): the header
//! must be exactly the family's columns; numbers in exponent form, NaN or
//! infinity are refused; timestamps must be RFC 3339; text longer than its
//! column is a row error instead of a database failure; a quantized price or a
//! quantity outside its column range is a row error. Reported reasons name a
//! column or a rule, never a row value.
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgConnection};
use sqlx::{Connection, Row};
use uuid::Uuid;

use super::csv::{self, Record};
use crate::MarketPolicy;
use crate::cli::{Environment, Exit, IngestArgs, SignalFamily};

/// An operator file is a few hundred rows; anything near these bounds is not one.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_ROWS: usize = 10_000;
/// Rows observed this far in the future are refused (clock skew allowance).
const FUTURE_TOLERANCE_MINUTES: i64 = 5;
/// Prices are `numeric(10,2)`: below 10^8.
const MAX_PRICE_EXCLUSIVE: i64 = 100_000_000;
/// Quantities are `numeric(12,2)`: below 10^10.
const MAX_QUANTITY_EXCLUSIVE: i64 = 10_000_000_000;
/// Column widths of the text the rows carry.
const MAX_EVENT_ID_CHARS: usize = 128;
const MAX_MODEL_CHARS: usize = 64;

/// The operator role and database of each environment.
pub fn operator_role(environment: Environment) -> &'static str {
    match environment {
        Environment::Production => "verdaxis_operator",
        Environment::Staging => "verdaxis_operator_staging",
    }
}

fn database_name(environment: Environment) -> &'static str {
    super::runtime_env::identity(environment).database_name
}

impl SignalFamily {
    /// `market_signal_ingestion_runs.signal_family`.
    pub fn run_family(self) -> &'static str {
        match self {
            Self::Indications => "MARKET_INDICATION",
            Self::FairBands => "FAIR_PRICE_BAND",
            Self::Stems => "PHYSICAL_STEM",
        }
    }

    /// `market_signal_ingestion_runs.source_kind`: fair bands are a model.
    fn source_kind(self) -> &'static str {
        match self {
            Self::Indications => "MARKET_INDICATION",
            Self::FairBands => "FAIR_PRICE_MODEL",
            Self::Stems => "PHYSICAL_STEM",
        }
    }

    fn table(self) -> &'static str {
        match self {
            Self::Indications => "market_indications",
            Self::FairBands => "fair_price_bands",
            Self::Stems => "physical_stems",
        }
    }

    /// The board's lookback: older rows import but never render, so the
    /// operator gets a staleness count.
    fn staleness_days(self) -> i64 {
        match self {
            Self::Indications => 7,
            Self::FairBands => 30,
            Self::Stems => 90,
        }
    }

    /// The exact header of the family's file (the documented Python columns).
    pub fn columns(self) -> &'static [&'static str] {
        match self {
            Self::Indications => &[
                "source_event_id",
                "market_product",
                "delivery_point",
                "availability_window",
                "side",
                "price_per_mt_usd",
                "quantity_mt",
                "observed_at",
            ],
            Self::FairBands => &[
                "source_event_id",
                "market_product",
                "delivery_point",
                "availability_window",
                "low_price_per_mt_usd",
                "mid_price_per_mt_usd",
                "high_price_per_mt_usd",
                "model_name",
                "model_version",
                "observed_at",
            ],
            Self::Stems => &[
                "source_event_id",
                "stem_uid",
                "market_product",
                "delivery_point",
                "availability_window",
                "status",
                "quantity_mt",
                "stem_start",
                "stem_end",
                "observed_at",
            ],
        }
    }
}

/// A file-level refusal; the reason never quotes file content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal(pub &'static str);

/// One rejected row: its 1-based data row number and a reason that names a
/// column or rule, never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowError {
    pub row: usize,
    pub reason: String,
}

/// What a run did or would do: counts and row errors only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub run_id: Option<Uuid>,
    pub inserted: usize,
    pub would_insert: usize,
    pub skipped_duplicates: usize,
    pub stale_warnings: usize,
    pub errors: Vec<RowError>,
}

/// A source id is stored, joined on and printed: lowercase, digits, `-`, `_`.
pub fn valid_source(source: &str) -> bool {
    let bytes = source.as_bytes();
    (2..=64).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_".contains(byte))
}

/// Read and parse the operator file: a bounded regular file whose header is
/// exactly the family's columns.
pub fn read_file(path: &Path, family: SignalFamily) -> Result<csv::Parsed, Refusal> {
    let bytes = crate::cli::state::read_regular_bytes(path, MAX_FILE_BYTES)
        .map_err(|_| Refusal("the file must be a regular file of at most 4 MiB"))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| Refusal("the file is not UTF-8"))?;
    let parsed = csv::parse(text, MAX_ROWS).map_err(|refusal| Refusal(refusal.0))?;
    let expected: HashSet<&str> = family.columns().iter().copied().collect();
    let actual: HashSet<&str> = parsed.header.iter().map(String::as_str).collect();
    if actual != expected {
        return Err(Refusal("the header is not exactly this family's columns"));
    }
    Ok(parsed)
}

// ---- validation ----------------------------------------------------------------------

/// The catalog a row resolves against: active delivery points from the
/// database, products and lanes from the attested market policy.
pub struct Catalog {
    points_by_name: HashMap<String, Uuid>,
    point_ids: HashSet<Uuid>,
    policy: MarketPolicy,
}

impl Catalog {
    pub fn new(active_points: Vec<(Uuid, String)>, policy: MarketPolicy) -> Self {
        Self {
            point_ids: active_points.iter().map(|(id, _)| *id).collect(),
            points_by_name: active_points
                .into_iter()
                .map(|(id, name)| (name, id))
                .collect(),
            policy,
        }
    }

    fn resolve_point(&self, raw: &str) -> Result<Uuid, String> {
        if let Some(id) = self.points_by_name.get(raw) {
            return Ok(*id);
        }
        match Uuid::parse_str(raw) {
            Ok(id) if self.point_ids.contains(&id) => Ok(id),
            _ => Err("delivery_point is not an active catalog delivery point".to_owned()),
        }
    }

    /// An order-book product that trades at this delivery point.
    fn check_lane(&self, product: &str, point: Uuid) -> Result<(), String> {
        let spec = self
            .policy
            .products
            .iter()
            .find(|spec| spec.code == product && spec.execution_mode == "ORDERBOOK")
            .ok_or("market_product is not an order-book product")?;
        if spec.lanes.contains(&point) {
            Ok(())
        } else {
            Err("market_product is not available at this delivery point".to_owned())
        }
    }
}

/// One validated row, ready to insert.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    pub source_event_id: String,
    pub market_product: String,
    pub delivery_point_id: Uuid,
    pub availability_window: String,
    pub observed_at: DateTime<Utc>,
    pub detail: Detail,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Detail {
    Indication {
        side: String,
        price: Decimal,
        quantity: Option<Decimal>,
    },
    FairBand {
        low: Decimal,
        mid: Decimal,
        high: Decimal,
        model_name: String,
        model_version: Option<String>,
    },
    Stem {
        stem_uid: String,
        status: String,
        quantity: Decimal,
        stem_start: Option<DateTime<Utc>>,
        stem_end: Option<DateTime<Utc>>,
    },
}

fn text<'a>(record: &'a Record, column: &str) -> &'a str {
    record.fields.get(column).map_or("", |value| value.trim())
}

fn required(record: &Record, column: &str) -> Result<String, String> {
    let value = text(record, column);
    if value.is_empty() {
        Err(format!("{column} is required"))
    } else {
        Ok(value.to_owned())
    }
}

fn bounded(value: String, column: &str, max_chars: usize) -> Result<String, String> {
    if value.chars().count() > max_chars {
        Err(format!("{column} is longer than {max_chars} characters"))
    } else {
        Ok(value)
    }
}

/// A positive plain decimal (no exponent, NaN or infinity).
fn positive(record: &Record, column: &str) -> Result<Decimal, String> {
    let raw = text(record, column);
    let plain = raw
        .bytes()
        .all(|byte| byte.is_ascii_digit() || b"+-._".contains(&byte));
    let value = Decimal::from_str(raw)
        .ok()
        .filter(|_| plain && !raw.is_empty())
        .ok_or_else(|| format!("{column} is not a valid number"))?;
    if value <= Decimal::ZERO {
        return Err(format!("{column} must be positive"));
    }
    Ok(value)
}

/// A price in USD/MT: positive, rounded half-even to cents, below 10^8.
fn price(record: &Record, column: &str) -> Result<Decimal, String> {
    let value = positive(record, column)?;
    let limit = Decimal::from(MAX_PRICE_EXCLUSIVE);
    let rounded = value.round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven);
    if value >= limit || rounded >= limit {
        return Err(format!("{column} exceeds the numeric(10,2) range"));
    }
    Ok(rounded)
}

fn quantity(record: &Record, column: &str) -> Result<Decimal, String> {
    let value = positive(record, column)?;
    if value >= Decimal::from(MAX_QUANTITY_EXCLUSIVE) {
        return Err(format!("{column} exceeds the numeric(12,2) range"));
    }
    Ok(value)
}

/// An RFC 3339 instant with an explicit offset, in UTC.
fn instant(raw: &str, column: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| format!("{column} is not an RFC 3339 time with an offset"))
}

fn optional_instant(record: &Record, column: &str) -> Result<Option<DateTime<Utc>>, String> {
    match text(record, column) {
        "" => Ok(None),
        raw => instant(raw, column).map(Some),
    }
}

/// Validate one row of `family` as the Python planners did.
pub fn plan(
    family: SignalFamily,
    record: &Record,
    catalog: &Catalog,
    now: DateTime<Utc>,
) -> Result<Planned, String> {
    let market_product = required(record, "market_product")?;
    let delivery_point_id = catalog.resolve_point(&required(record, "delivery_point")?)?;
    catalog.check_lane(&market_product, delivery_point_id)?;
    let availability_window = crate::market::windows::normalize_window(
        &required(record, "availability_window")?,
        now.date_naive(),
    )?;
    let source_event_id = bounded(
        required(record, "source_event_id")?,
        "source_event_id",
        MAX_EVENT_ID_CHARS,
    )?;
    let observed_at = instant(&required(record, "observed_at")?, "observed_at")?;
    if observed_at > now + Duration::minutes(FUTURE_TOLERANCE_MINUTES) {
        return Err("observed_at is in the future".to_owned());
    }
    let detail = match family {
        SignalFamily::Indications => {
            let side = required(record, "side")?.to_uppercase();
            if !["BID", "ASK", "MID"].contains(&side.as_str()) {
                return Err("side must be BID, ASK or MID".to_owned());
            }
            let quantity = match text(record, "quantity_mt") {
                "" => None,
                _ => Some(quantity(record, "quantity_mt")?),
            };
            Detail::Indication {
                side,
                price: price(record, "price_per_mt_usd")?,
                quantity,
            }
        }
        SignalFamily::FairBands => {
            let low = price(record, "low_price_per_mt_usd")?;
            let mid = price(record, "mid_price_per_mt_usd")?;
            let high = price(record, "high_price_per_mt_usd")?;
            if !(low <= mid && mid <= high) {
                return Err("the band must satisfy low <= mid <= high".to_owned());
            }
            let model_version = match text(record, "model_version") {
                "" => None,
                value => Some(bounded(value.to_owned(), "model_version", MAX_MODEL_CHARS)?),
            };
            Detail::FairBand {
                low,
                mid,
                high,
                model_name: bounded(
                    required(record, "model_name")?,
                    "model_name",
                    MAX_MODEL_CHARS,
                )?,
                model_version,
            }
        }
        SignalFamily::Stems => {
            let status = required(record, "status")?.to_uppercase();
            if !["AVAILABLE", "TENTATIVE", "ALLOCATED", "CANCELLED"].contains(&status.as_str()) {
                return Err(
                    "status must be AVAILABLE, TENTATIVE, ALLOCATED or CANCELLED".to_owned(),
                );
            }
            let stem_start = optional_instant(record, "stem_start")?;
            let stem_end = optional_instant(record, "stem_end")?;
            if let (Some(start), Some(end)) = (stem_start, stem_end)
                && end < start
            {
                return Err("stem_end precedes stem_start".to_owned());
            }
            Detail::Stem {
                stem_uid: bounded(
                    required(record, "stem_uid")?,
                    "stem_uid",
                    MAX_EVENT_ID_CHARS,
                )?,
                status,
                quantity: quantity(record, "quantity_mt")?,
                stem_start,
                stem_end,
            }
        }
    };
    Ok(Planned {
        source_event_id,
        market_product,
        delivery_point_id,
        availability_window,
        observed_at,
        detail,
    })
}

// ---- writing -------------------------------------------------------------------------

/// The database failed for a reason other than a duplicate; nothing was kept.
#[derive(Debug)]
pub struct WriteFailure;

impl From<sqlx::Error> for WriteFailure {
    fn from(_: sqlx::Error) -> Self {
        Self
    }
}

/// Load the catalog, plan every row, and with `apply` write one verified run
/// in one transaction (a savepoint per row). Without `apply`, or when no row
/// is valid, nothing is written.
pub async fn ingest(
    connection: &mut PgConnection,
    family: SignalFamily,
    source: &str,
    parsed: &csv::Parsed,
    policy: MarketPolicy,
    apply: bool,
    now: DateTime<Utc>,
) -> Result<Report, WriteFailure> {
    let points: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM delivery_points WHERE is_active")
            .fetch_all(&mut *connection)
            .await?;
    let catalog = Catalog::new(points, policy);
    let mut report = Report::default();
    for row in &parsed.malformed_rows {
        report.errors.push(RowError {
            row: *row,
            reason: "the row does not have one field per column".to_owned(),
        });
    }
    let stale_before = now - Duration::days(family.staleness_days());
    let mut planned = Vec::new();
    for record in &parsed.records {
        match plan(family, record, &catalog, now) {
            Ok(row) => {
                if row.observed_at < stale_before {
                    report.stale_warnings += 1;
                }
                planned.push(row);
            }
            Err(reason) => report.errors.push(RowError {
                row: record.row_number,
                reason,
            }),
        }
    }
    report.errors.sort_by_key(|error| error.row);
    if !apply || planned.is_empty() {
        report.would_insert = planned.len();
        return Ok(report);
    }

    let mut transaction = connection.begin().await?;
    let run_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO market_signal_ingestion_runs \
         (id, signal_family, source, source_kind, started_at, verified_at) \
         VALUES ($1, $2, $3, $4, $5, NULL)",
    )
    .bind(run_id)
    .bind(family.run_family())
    .bind(source)
    .bind(family.source_kind())
    .bind(now)
    .execute(&mut *transaction)
    .await?;
    let duplicate_index = format!("uq_{}_source_event", family.table());
    for row in &planned {
        sqlx::query("SAVEPOINT signal_row")
            .execute(&mut *transaction)
            .await?;
        match insert_row(&mut transaction, family, source, run_id, row, now).await {
            Ok(()) => {
                sqlx::query("RELEASE SAVEPOINT signal_row")
                    .execute(&mut *transaction)
                    .await?;
                report.inserted += 1;
            }
            // Only the idempotency identity proves a duplicate; any other
            // constraint failure aborts the run rather than pass as reuse.
            Err(sqlx::Error::Database(error))
                if error.code().as_deref() == Some("23505")
                    && error.constraint() == Some(duplicate_index.as_str()) =>
            {
                sqlx::query("ROLLBACK TO SAVEPOINT signal_row")
                    .execute(&mut *transaction)
                    .await?;
                report.skipped_duplicates += 1;
            }
            Err(error) => return Err(error.into()),
        }
    }
    if report.inserted == 0 {
        // A verified run with no rows would vouch for nothing: keep none.
        transaction.rollback().await?;
        return Ok(report);
    }
    sqlx::query("UPDATE market_signal_ingestion_runs SET verified_at = now() WHERE id = $1")
        .bind(run_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    report.run_id = Some(run_id);
    Ok(report)
}

async fn insert_row(
    connection: &mut PgConnection,
    family: SignalFamily,
    source: &str,
    run_id: Uuid,
    row: &Planned,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let query = match (&row.detail, family) {
        (
            Detail::Indication {
                side,
                price,
                quantity,
            },
            SignalFamily::Indications,
        ) => sqlx::query(
            "INSERT INTO market_indications (id, market_product, delivery_point_id, \
             availability_window, side, price_per_mt_usd, quantity_mt, source, source_event_id, \
             trusted_ingestion_run_id, is_demo, is_verified_real, verified_real_at, observed_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, false, true, $11, $12)",
        )
        .bind(Uuid::new_v4())
        .bind(&row.market_product)
        .bind(row.delivery_point_id)
        .bind(&row.availability_window)
        .bind(side)
        .bind(price)
        .bind(quantity),
        (
            Detail::FairBand {
                low,
                mid,
                high,
                model_name,
                model_version,
            },
            SignalFamily::FairBands,
        ) => sqlx::query(
            "INSERT INTO fair_price_bands (id, market_product, delivery_point_id, \
             availability_window, low_price_per_mt_usd, mid_price_per_mt_usd, \
             high_price_per_mt_usd, model_name, model_version, source, source_event_id, \
             trusted_ingestion_run_id, is_demo, is_verified_real, verified_real_at, observed_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, false, true, $13, $14)",
        )
        .bind(Uuid::new_v4())
        .bind(&row.market_product)
        .bind(row.delivery_point_id)
        .bind(&row.availability_window)
        .bind(low)
        .bind(mid)
        .bind(high)
        .bind(model_name)
        .bind(model_version),
        (
            Detail::Stem {
                stem_uid,
                status,
                quantity,
                stem_start,
                stem_end,
            },
            SignalFamily::Stems,
        ) => sqlx::query(
            "INSERT INTO physical_stems (id, market_product, delivery_point_id, \
             availability_window, stem_uid, status, quantity_mt, stem_start, stem_end, source, \
             source_event_id, trusted_ingestion_run_id, is_demo, is_verified_real, \
             verified_real_at, observed_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, false, true, $13, $14)",
        )
        .bind(Uuid::new_v4())
        .bind(&row.market_product)
        .bind(row.delivery_point_id)
        .bind(&row.availability_window)
        .bind(stem_uid)
        .bind(status)
        .bind(quantity)
        .bind(stem_start)
        .bind(stem_end),
        _ => unreachable!("a planned row always carries its family's detail"),
    };
    query
        .bind(source)
        .bind(&row.source_event_id)
        .bind(run_id)
        .bind(now)
        .bind(row.observed_at)
        .execute(connection)
        .await
        .map(|_| ())
}

// ---- command -------------------------------------------------------------------------

/// The one-line JSON report: mode, counts and row errors, never row values.
pub fn report_json(
    family: SignalFamily,
    source: &str,
    apply: bool,
    report: &Report,
) -> serde_json::Value {
    json!({
        "command": "ops ingest signals",
        "mode": if apply { "apply" } else { "dry_run" },
        "family": family.run_family(),
        "source": source,
        "run_id": report.run_id,
        "inserted": report.inserted,
        "would_insert": report.would_insert,
        "skipped_duplicates": report.skipped_duplicates,
        "stale_warnings": report.stale_warnings,
        "errors": report.errors.iter().map(|error| json!({"row": error.row, "reason": error.reason})).collect::<Vec<_>>(),
    })
}

fn refuse(exit: Exit, reason: &str) -> Exit {
    eprintln!(
        "{}",
        json!({"command": "ops ingest signals", "outcome": "refused", "reason": reason})
    );
    exit
}

/// `ops ingest signals`, with the process environment.
pub async fn run(args: &IngestArgs) -> Exit {
    let variables: HashMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    run_with(args, &variables, Utc::now()).await
}

/// Every refusal that needs no database comes before the one connection.
pub async fn run_with(
    args: &IngestArgs,
    variables: &HashMap<String, String>,
    now: DateTime<Utc>,
) -> Exit {
    let environment = args.input.identity.environment;
    if environment == Environment::Production {
        return refuse(Exit::Usage, "signal ingestion runs in staging only");
    }
    if !valid_source(&args.source) {
        return refuse(
            Exit::Usage,
            "--source must match ^[a-z0-9][a-z0-9_-]{1,63}$",
        );
    }
    let value = |key: &str| variables.get(key).map(String::as_str).unwrap_or("");
    if let Err(exit) = crate::cli::gate::require_identity(
        value("ENVIRONMENT"),
        value("RELEASE_SHA"),
        environment,
        &args.input.identity.release_sha,
    ) {
        return refuse(exit, "the deployed identity does not match this release");
    }
    let parsed = match read_file(&args.input.file, args.family) {
        Ok(parsed) => parsed,
        Err(refusal) => return refuse(Exit::Precondition, refusal.0),
    };
    let policy = match crate::runtime::policy::committed_policy() {
        Ok(policy) => policy,
        Err(_) => return refuse(Exit::Precondition, "the market policy is not attested"),
    };
    // The operator URL must name this environment's operator role and database.
    let options = match PgConnectOptions::from_str(value("OPERATOR_DATABASE_URL")) {
        Ok(options)
            if options.get_username() == operator_role(environment)
                && options.get_database() == Some(database_name(environment)) =>
        {
            options
        }
        _ => {
            return refuse(
                Exit::Precondition,
                "OPERATOR_DATABASE_URL is not this environment's operator",
            );
        }
    };
    let mut connection = match PgConnection::connect_with(&options).await {
        Ok(connection) => connection,
        Err(_) => return refuse(Exit::Dependency, "the database is unreachable"),
    };
    // Attest the connection that writes: the exact database and role.
    let attested = sqlx::query("SELECT current_database(), current_user::text")
        .fetch_one(&mut connection)
        .await
        .map(|row| {
            row.get::<String, _>(0) == database_name(environment)
                && row.get::<String, _>(1) == operator_role(environment)
        });
    match attested {
        Ok(true) => {}
        Ok(false) => {
            return refuse(
                Exit::Precondition,
                "the connection is not the operator on this database",
            );
        }
        Err(_) => return refuse(Exit::Dependency, "the database is unreachable"),
    }
    let outcome = ingest(
        &mut connection,
        args.family,
        &args.source,
        &parsed,
        policy,
        args.input.apply,
        now,
    )
    .await;
    let _ = connection.close().await;
    match outcome {
        Ok(report) => {
            println!(
                "{}",
                report_json(args.family, &args.source, args.input.apply, &report)
            );
            // As in Python: errors with nothing inserted is a failed run.
            if !report.errors.is_empty() && report.inserted == 0 {
                Exit::Precondition
            } else {
                Exit::Ok
            }
        }
        Err(WriteFailure) => refuse(Exit::Dependency, "the run failed and nothing was kept"),
    }
}
