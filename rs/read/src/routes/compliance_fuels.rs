//! C03: the public fuel-intensity table.
//!
//! The body is a constant policy table: there is no database read, no clock
//! and no caller scope, so the route needs no authentication and opens no
//! transaction. Intensities are fuel-category assumptions for illustration;
//! only the reporting-year limits come from the regulation.

use crate::compliance::scoring::FUEL_INTENSITIES;
use crate::{AppState, Json};
use axum::{Json as AxumJson, Router, routing::get};
use serde_json::json;

/// The one path this group owns.
pub const PATH: &str = "/api/compliance/fuels";

/// Wire unit of the intensity values.
pub const UNIT: &str = "gCO2eq/MJ";

/// What the table is and is not, in the words the client shows.
pub const SOURCE: &str = "Illustrative fuel-category assumptions; not verified consignment CI. Only reporting-year limits derive from Regulation (EU) 2023/1805 Article 4.";

pub fn routes() -> Router<AppState> {
    Router::new().route(PATH, get(fuels))
}

pub async fn fuels() -> AxumJson<Json> {
    AxumJson(fuels_response())
}

/// The response body, shared by the handler and the offline vectors.
pub fn fuels_response() -> Json {
    let fuels = FUEL_INTENSITIES
        .iter()
        .map(|(fuel, intensity)| ((*fuel).to_owned(), json!(intensity.to_string())))
        .collect();
    json!({"fuels": Json::Object(fuels), "unit": UNIT, "source": SOURCE})
}
