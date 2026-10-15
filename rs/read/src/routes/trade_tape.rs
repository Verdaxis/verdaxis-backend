//! Anonymized, immutable seven-day trade history.
use super::{
    ports::{query_error, utc_rfc3339},
    prices::{captured_now, fields, integer, uuid},
};
use crate::market::{
    visibility::{TradeEvidenceFilters, bind_trade_query, trade_query_sql},
    windows::normalize_window,
};
use crate::{ApiError, AppState, run_admitted};
use axum::{
    Router,
    extract::{RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/trade-tape", get(tape))
}
async fn tape(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let raw = raw.unwrap_or_default();
    let mut values = fields(&raw);
    let now = captured_now(&state)?;
    let point = uuid(&raw, "delivery_point_id")?;
    let skip = integer(&raw, "skip", 0, i64::MAX, 0)?;
    let limit = integer(&raw, "limit", 1, 200, 50)?;
    let demo = match values.get("demo_only").map(|v| v.to_lowercase()).as_deref() {
        None | Some("false" | "0" | "off" | "no" | "f" | "n") => false,
        Some("true" | "1" | "on" | "yes" | "t" | "y") => true,
        _ => {
            return Err(ApiError {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                detail: json!([{"type":"bool_parsing","loc":["query","demo_only"],"msg":"Input should be a valid boolean, unable to interpret input","input":values["demo_only"]}]),
                retry_after: None,
            });
        }
    };
    let window = match values.remove("availability_window") {
        Some(v) => match normalize_window(&v, now.date_naive()) {
            Ok(v) => Some(v),
            Err(_) => {
                return Ok(
                    (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
                );
            }
        },
        None => None,
    };
    let fuel = values.remove("fuel_type");
    let market = values.remove("market_product");
    let region = values.remove("region");
    let policy = state.policy.clone();
    let (items,total)=run_admitted(&state,move |connection|Box::pin(async move {
        let filters=TradeEvidenceFilters{confirmed_since:Some(now-chrono::Duration::days(7)),delivery_point_id:point,availability_window:window.as_deref(),..Default::default()};
        let count=trade_query_sql(include_str!("../../sql/prices/tape_count.sql"));
        let total:i64=bind_trade_query(&count,&policy,&filters).map_err(|_|ApiError::internal())?.bind(&fuel).bind(&market).bind(&region).bind(demo).fetch_one(&mut *connection).await.map_err(|e|query_error("/api/trade-tape",&e))?.try_get("count").map_err(|_|ApiError::internal())?;
        let sql=trade_query_sql(include_str!("../../sql/prices/tape.sql"));
        let rows=bind_trade_query(&sql,&policy,&filters).map_err(|_|ApiError::internal())?.bind(fuel).bind(market).bind(region).bind(demo).bind(limit).bind(skip).fetch_all(connection).await.map_err(|e|query_error("/api/trade-tape",&e))?;
        let mut items=Vec::new();
        for row in rows {
            let get=|key| row.try_get::<String,_>(key).map_err(|_|ApiError::internal());
            let id:Uuid=row.try_get("id").map_err(|_|ApiError::internal())?;
            let product:Uuid=row.try_get("product_id").map_err(|_|ApiError::internal())?;
            let point_id:Uuid=row.try_get("delivery_point_id").map_err(|_|ApiError::internal())?;
            let point_name=get("delivery_point_name")?;
            let region=if point.is_some(){point_name.clone()}else{get("delivery_point_region")?};
            let demo=get("buyer_provenance")?=="DEMO";
            let source=if demo {"DEMO_SEED"}else{"CONFIRMED_TRADE"};
            items.push(json!({"id":&id.simple().to_string()[..8],"product_id":point.map(|_|product),"market_product":get("market_product")?,"fuel_type":get("fuel_type")?,"fuel_grade":get("fuel_grade")?,"delivery_point_id":point.map(|_|point_id),"delivery_point_name":point.map(|_|point_name),"region":region,"quantity_mt":get("quantity")?,"price_per_mt_usd":get("price")?,"total_usd":get("total")?,"confirmed_at":utc_rfc3339(row.try_get("confirmed_at").map_err(|_|ApiError::internal())?,"Z"),"availability_window":get("availability_window")?,"is_demo_trade":demo,"scope":if point.is_some(){"DELIVERY_POINT"}else if region.is_empty(){"UNKNOWN"}else{"REGION"},"provenance_kind":source,"source_kind":source,"demo_status":if demo{"DEMO_ONLY"}else{"REAL_ONLY"}}));
        }
        Ok((items,total))
    })).await?;
    Ok(axum::Json(json!({"items":items,"total":total,"market_hours":true})).into_response())
}
