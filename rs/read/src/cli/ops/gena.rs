//! `ops import gena`: import a GENA producer-project CSV (Phase 10 Task 22),
//! replacing `scripts/import_gena_csv.py`.
//!
//! Every row is validated before the first write, so a malformed row leaves no
//! partial import. Rows are inserted in one transaction as the operator role;
//! the source identity is `gena_project_id`, and `ON CONFLICT DO NOTHING` makes
//! a replay or a concurrent import of the same id insert it once. A blank id
//! cannot be deduplicated, so it is refused unless `--allow-blank-id`, and those
//! rows are counted. Dry run is the default.
//!
//! Fixed against Python: a zero coordinate is kept (Python dropped it as
//! falsy); a row with only one coordinate is an error. Stricter than Python:
//! the header must be exactly these columns, and every value is bounded by its
//! column. An unknown status is ANNOUNCED, as in Python.
use std::collections::HashMap;
use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::json;
use sqlx::Connection;
use sqlx::postgres::PgConnection;

use super::csv::{self, Record};
use crate::cli::{Exit, ImportArgs};

pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_ROWS: usize = 10_000;
/// The exact header of a GENA export.
pub const COLUMNS: [&str; 11] = [
    "project_name",
    "country",
    "region",
    "capacity_kt",
    "cod_year",
    "status",
    "feedstock",
    "technology",
    "lat",
    "lng",
    "gena_id",
];
/// `capacity_kt_per_year` is `numeric(10,2)`: below 10^8.
const MAX_CAPACITY_EXCLUSIVE: i64 = 100_000_000;
const COD_YEARS: std::ops::RangeInclusive<i32> = 1900..=2100;
/// A serialization failure or deadlock reruns the transaction this many times.
const MAX_ATTEMPTS: usize = 3;

/// One validated project, ready to insert.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub name: String,
    pub country: String,
    pub region: Option<String>,
    pub capacity: Option<Decimal>,
    pub cod_year: Option<i32>,
    pub status: &'static str,
    pub feedstock: Option<String>,
    pub technology: Option<String>,
    /// (latitude, longitude); zero is a valid coordinate.
    pub point: Option<(f64, f64)>,
    pub gena_id: Option<String>,
}

/// One rejected row: its data row number and a reason naming a column or rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowError {
    pub row: usize,
    pub reason: String,
}

/// Python's status map; anything else is ANNOUNCED.
fn status(raw: &str) -> &'static str {
    match raw.trim().to_lowercase().as_str() {
        "under construction" => "UNDER_CONSTRUCTION",
        "operational" => "OPERATIONAL",
        "cancelled" => "CANCELLED",
        _ => "ANNOUNCED",
    }
}

fn text<'a>(record: &'a Record, column: &str) -> &'a str {
    record.fields.get(column).map_or("", |value| value.trim())
}

fn optional_text(
    record: &Record,
    column: &str,
    max_chars: usize,
) -> Result<Option<String>, String> {
    match text(record, column) {
        "" => Ok(None),
        value if value.chars().count() > max_chars => {
            Err(format!("{column} is longer than {max_chars} characters"))
        }
        value => Ok(Some(value.to_owned())),
    }
}

fn required_text(
    record: &Record,
    column: &str,
    max_chars: Option<usize>,
) -> Result<String, String> {
    let value = text(record, column);
    if value.is_empty() {
        return Err(format!("{column} is required"));
    }
    if max_chars.is_some_and(|max| value.chars().count() > max) {
        return Err(format!("{column} is too long"));
    }
    Ok(value.to_owned())
}

/// A finite coordinate within `bound` degrees.
fn coordinate(raw: &str, column: &str, bound: f64) -> Result<f64, String> {
    let digits = raw
        .bytes()
        .all(|byte| byte.is_ascii_digit() || b"+-.".contains(&byte));
    match raw.parse::<f64>() {
        Ok(value) if digits && value.is_finite() && value.abs() <= bound => Ok(value),
        _ => Err(format!(
            "{column} must be a number from -{bound} to {bound}"
        )),
    }
}

/// Validate one row; `allow_blank_id` admits a row without a GENA id.
pub fn validate(record: &Record, allow_blank_id: bool) -> Result<Project, String> {
    let name = required_text(record, "project_name", None)?;
    let country = required_text(record, "country", Some(100))?;
    let capacity = match text(record, "capacity_kt") {
        "" => None,
        raw => {
            let plain = raw
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'.');
            let value = Decimal::from_str(raw)
                .ok()
                .filter(|value| plain && *value < Decimal::from(MAX_CAPACITY_EXCLUSIVE))
                .ok_or("capacity_kt must be a number from 0 to below 100000000")?;
            Some(value)
        }
    };
    let cod_year = match text(record, "cod_year") {
        "" => None,
        raw => Some(
            raw.parse::<i32>()
                .ok()
                .filter(|year| COD_YEARS.contains(year))
                .ok_or("cod_year must be a year from 1900 to 2100")?,
        ),
    };
    let point = match (text(record, "lat"), text(record, "lng")) {
        ("", "") => None,
        ("", _) | (_, "") => return Err("lat and lng must be given together".to_owned()),
        (lat, lng) => Some((
            coordinate(lat, "lat", 90.0)?,
            coordinate(lng, "lng", 180.0)?,
        )),
    };
    let gena_id = optional_text(record, "gena_id", 100)?;
    if gena_id.is_none() && !allow_blank_id {
        return Err("gena_id is required (--allow-blank-id admits a row without one)".to_owned());
    }
    Ok(Project {
        name,
        country,
        region: optional_text(record, "region", 100)?,
        capacity,
        cod_year,
        status: status(text(record, "status")),
        feedstock: optional_text(record, "feedstock", 200)?,
        technology: optional_text(record, "technology", 200)?,
        point,
        gena_id,
    })
}

/// What an import did or would do: counts only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    pub created: usize,
    pub skipped_existing: usize,
    pub created_without_id: usize,
}

/// Insert every project in one transaction; the same GENA id inserts once.
async fn insert_all(
    connection: &mut PgConnection,
    projects: &[Project],
    fuel_type: &str,
) -> Result<Counts, sqlx::Error> {
    let mut transaction = connection.begin().await?;
    let mut counts = Counts::default();
    for project in projects {
        let (lat, lng) = project
            .point
            .map_or((None, None), |(lat, lng)| (Some(lat), Some(lng)));
        let inserted = sqlx::query(
            "INSERT INTO producer_projects (name, fuel_type, capacity_kt_per_year, country, \
             region, location, cod_year, status, data_source, gena_project_id, feedstock, \
             technology) \
             VALUES ($1, $2, $3, $4, $5, \
             CASE WHEN $6::double precision IS NULL THEN NULL \
                  ELSE ST_SetSRID(ST_MakePoint($7::double precision, $6::double precision), 4326)::geography END, \
             $8, $9, 'GENA', $10, $11, $12) \
             ON CONFLICT (gena_project_id) DO NOTHING",
        )
        .bind(&project.name)
        .bind(fuel_type)
        .bind(project.capacity)
        .bind(&project.country)
        .bind(&project.region)
        .bind(lat)
        .bind(lng)
        .bind(project.cod_year)
        .bind(project.status)
        .bind(&project.gena_id)
        .bind(&project.feedstock)
        .bind(&project.technology)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        match (inserted, project.gena_id.is_some()) {
            (0, _) => counts.skipped_existing += 1,
            (_, true) => counts.created += 1,
            (_, false) => counts.created_without_id += 1,
        }
    }
    transaction.commit().await?;
    Ok(counts)
}

/// Retry the whole transaction on a serialization failure or deadlock only.
pub async fn import(
    connection: &mut PgConnection,
    projects: &[Project],
    fuel_type: &str,
) -> Result<Counts, sqlx::Error> {
    let mut attempt = 1;
    loop {
        match insert_all(connection, projects, fuel_type).await {
            Err(sqlx::Error::Database(error))
                if attempt < MAX_ATTEMPTS
                    && matches!(error.code().as_deref(), Some("40001" | "40P01")) =>
            {
                attempt += 1;
            }
            outcome => return outcome,
        }
    }
}

fn refuse(exit: Exit, reason: &str) -> Exit {
    eprintln!(
        "{}",
        json!({"command": "ops import gena", "outcome": "refused", "reason": reason})
    );
    exit
}

/// `ops import gena`, with the process environment.
pub async fn run(args: &ImportArgs) -> Exit {
    let variables: HashMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    run_with(args, &variables).await
}

/// Every refusal that needs no database comes before the one connection.
pub async fn run_with(args: &ImportArgs, variables: &HashMap<String, String>) -> Exit {
    let environment = args.input.identity.environment;
    let fuel_type = args.fuel_type.trim();
    if fuel_type.is_empty() || fuel_type.chars().count() > 50 {
        return refuse(Exit::Usage, "--fuel-type must be 1 to 50 characters");
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
    let bytes = match crate::cli::state::read_regular_bytes(&args.input.file, MAX_FILE_BYTES) {
        Ok(bytes) => bytes,
        Err(_) => {
            return refuse(
                Exit::Precondition,
                "the file must be a regular file of at most 4 MiB",
            );
        }
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return refuse(Exit::Precondition, "the file is not UTF-8");
    };
    let parsed = match csv::parse(text, MAX_ROWS) {
        Ok(parsed) => parsed,
        Err(refusal) => return refuse(Exit::Precondition, refusal.0),
    };
    let mut header = parsed.header.clone();
    header.sort();
    let mut expected = COLUMNS.map(str::to_owned).to_vec();
    expected.sort();
    if header != expected {
        return refuse(
            Exit::Precondition,
            "the header is not exactly the GENA columns",
        );
    }
    let mut errors: Vec<RowError> = parsed
        .malformed_rows
        .iter()
        .map(|row| RowError {
            row: *row,
            reason: "the row does not have one field per column".to_owned(),
        })
        .collect();
    let mut projects = Vec::new();
    for record in &parsed.records {
        match validate(record, args.allow_blank_id) {
            Ok(project) => projects.push(project),
            Err(reason) => errors.push(RowError {
                row: record.row_number,
                reason,
            }),
        }
    }
    errors.sort_by_key(|error| error.row);
    let report = |counts: &Counts, errors: &[RowError]| {
        json!({
            "command": "ops import gena",
            "mode": if args.input.apply { "apply" } else { "dry_run" },
            "rows": parsed.records.len() + parsed.malformed_rows.len(),
            "would_import": if args.input.apply { 0 } else { projects.len() },
            "created": counts.created,
            "created_without_id": counts.created_without_id,
            "skipped_existing": counts.skipped_existing,
            "errors": errors.iter().map(|error| json!({"row": error.row, "reason": error.reason})).collect::<Vec<_>>(),
        })
    };
    // One malformed row refuses the whole file before any connection.
    if !errors.is_empty() {
        println!("{}", report(&Counts::default(), &errors));
        return refuse(Exit::Precondition, "a row is invalid; nothing was imported");
    }
    let mut connection = match super::operator::connect(environment, variables).await {
        Ok(connection) => connection,
        Err((exit, reason)) => return refuse(exit, reason),
    };
    if !args.input.apply {
        let _ = connection.close().await;
        println!("{}", report(&Counts::default(), &errors));
        return Exit::Ok;
    }
    let outcome = import(&mut connection, &projects, fuel_type).await;
    let _ = connection.close().await;
    match outcome {
        Ok(counts) => {
            println!("{}", report(&counts, &errors));
            Exit::Ok
        }
        Err(_) => refuse(Exit::Dependency, "the import failed and nothing was kept"),
    }
}
