//! M05: bounded public producer projects, with immutable coordinate projection.

use axum::{
    Json, Router,
    extract::{RawQuery, State},
    routing::get,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::routes::market_data_error;
use crate::runtime::admission::run_market_read;
use crate::{ApiError, AppState};

pub const PATH: &str = "/api/producers";
pub const MAX_PROJECTS: u64 = 5000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProjectStatus {
    Announced,
    UnderConstruction,
    Operational,
    Cancelled,
}

impl ProjectStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Announced => "ANNOUNCED",
            Self::UnderConstruction => "UNDER_CONSTRUCTION",
            Self::Operational => "OPERATIONAL",
            Self::Cancelled => "CANCELLED",
        }
    }
}

#[derive(Debug, Default)]
pub struct ProducerQuery {
    pub fuel_type: Option<String>,
    pub country: Option<String>,
    pub status: Option<ProjectStatus>,
    pub cod_year_min: Option<i32>,
    pub cod_year_max: Option<i32>,
}

impl ProducerQuery {
    pub fn parse(raw: &str) -> Result<Self, ApiError> {
        let mut query = Self::default();
        for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
            match key.as_ref() {
                "fuel_type" => query.fuel_type = Some(value.into_owned()),
                "country" => query.country = Some(value.into_owned()),
                "status" => {
                    query.status = Some(match value.as_ref() {
                        "ANNOUNCED" => ProjectStatus::Announced,
                        "UNDER_CONSTRUCTION" => ProjectStatus::UnderConstruction,
                        "OPERATIONAL" => ProjectStatus::Operational,
                        "CANCELLED" => ProjectStatus::Cancelled,
                        _ => {
                            return Err(market_data_error::invalid_input(
                                "Unknown project status.",
                            ));
                        }
                    });
                }
                "cod_year_min" => query.cod_year_min = Some(parse_year(&value)?),
                "cod_year_max" => query.cod_year_max = Some(parse_year(&value)?),
                _ => {}
            }
        }
        if let (Some(minimum), Some(maximum)) = (query.cod_year_min, query.cod_year_max)
            && minimum > maximum
        {
            return Err(market_data_error::invalid_input(
                "The minimum COD year must not exceed the maximum.",
            ));
        }
        Ok(query)
    }
}

fn parse_year(value: &str) -> Result<i32, ApiError> {
    value
        .parse()
        .map_err(|_| market_data_error::invalid_input("COD bounds must be 32-bit integers."))
}

/// Client fields only. NUMERIC values stay exact decimal strings; geography is
/// selected as nullable coordinates and is never changed in storage.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ProducerProject {
    pub id: Uuid,
    pub name: String,
    pub fuel_type: String,
    pub capacity_kt_per_year: Option<String>,
    pub country: String,
    pub region: Option<String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub cod_date: Option<NaiveDate>,
    pub cod_year: Option<i32>,
    pub status: String,
    pub data_source: Option<String>,
    pub gena_project_id: Option<String>,
    pub organization_id: Option<Uuid>,
    pub feedstock: Option<String>,
    pub technology: Option<String>,
    pub carbon_intensity_gco2_mj: Option<String>,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route(PATH, get(list_projects))
}

async fn list_projects(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Json<Vec<ProducerProject>>, ApiError> {
    let query = ProducerQuery::parse(raw.as_deref().unwrap_or_default())?;
    let projects = run_market_read(&state, move |connection| {
        Box::pin(select_projects(connection, query))
    })
    .await?;
    Ok(Json(projects))
}

async fn select_projects(
    connection: &mut PgConnection,
    query: ProducerQuery,
) -> Result<Vec<ProducerProject>, ApiError> {
    let rows = sqlx::query_as::<_, ProducerProject>(include_str!("../../sql/producers.sql"))
        .bind(query.fuel_type)
        .bind(query.country)
        .bind(query.status.map(ProjectStatus::as_str))
        .bind(query.cod_year_min)
        .bind(query.cod_year_max)
        .bind(market_data_error::probe_limit(MAX_PROJECTS))
        .fetch_all(connection)
        .await
        .map_err(|error| market_data_error::database_failure("producers", &error))?;
    market_data_error::within_limit(rows.len() as u64, MAX_PROJECTS)?;
    Ok(rows)
}
