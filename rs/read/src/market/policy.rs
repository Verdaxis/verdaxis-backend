//! Attested catalog policy: identity, lanes, and display order as data.
//!
//! Executable Rust must never branch on a product name, code, or UUID. The
//! generated policy artifact supplies them, and [`product_matches`] and
//! [`point_matches`] join database rows to policy records by the exact reviewed
//! identity tuple. Mutable fields (unit, minimum lot, description) stay in the
//! database; the policy supplies only approved identity, lanes, and order.

use std::collections::HashSet;

use uuid::Uuid;

use crate::{Json, MarketPolicy, PointPolicy, ProductPolicy};

/// Rule versions, not product identities. A new rule version is a code change
/// with its own tests; a new product is only new data.
const KNOWN_PROFILES: [&str; 3] = ["scalar", "fame_v1", "lng_v1"];
const KNOWN_EXECUTION_MODES: [&str; 1] = ["ORDERBOOK"];

/// Parse and validate the committed policy artifact.
///
/// `expected_sha` is the recorded oracle commit. The policy must carry exactly
/// that `source_sha`, so a policy exported from a different source tree is
/// refused rather than silently trusted.
pub fn load_policy(bytes: &[u8], expected_sha: &str) -> Result<MarketPolicy, String> {
    let policy: MarketPolicy = serde_json::from_slice(bytes)
        .map_err(|_| "market policy is not a valid policy document".to_owned())?;
    if policy.source_sha != expected_sha {
        return Err(format!(
            "market policy source SHA mismatch: expected {expected_sha}, found {}",
            policy.source_sha
        ));
    }
    validate_policy(&policy)?;
    Ok(policy)
}

/// Refuse duplicate identity, unknown profiles or modes, empty lanes, and lane
/// references to absent delivery points.
pub fn validate_policy(policy: &MarketPolicy) -> Result<(), String> {
    if !is_full_sha(&policy.source_sha) {
        return Err("market policy source SHA is not a full 40-hex commit".to_owned());
    }
    if policy.products.is_empty() {
        return Err("market policy lists no products".to_owned());
    }
    if policy.points.is_empty() {
        return Err("market policy lists no delivery points".to_owned());
    }
    let mut point_ids: HashSet<Uuid> = HashSet::new();
    for point in &policy.points {
        if !point_ids.insert(point.id) {
            return Err(format!("duplicate delivery point id {}", point.id));
        }
        if point.timezone.is_empty() {
            return Err(format!("delivery point {} has no timezone", point.name));
        }
    }
    let mut codes: HashSet<&str> = HashSet::new();
    let mut product_ids: HashSet<Uuid> = HashSet::new();
    for product in &policy.products {
        if !codes.insert(product.code.as_str()) {
            return Err(format!("duplicate product code {}", product.code));
        }
        if !product_ids.insert(product.id) {
            return Err(format!("duplicate product id {}", product.id));
        }
        if !KNOWN_PROFILES.contains(&product.qualification_profile.as_str()) {
            return Err(format!(
                "unknown qualification profile {}",
                product.qualification_profile
            ));
        }
        if !KNOWN_EXECUTION_MODES.contains(&product.execution_mode.as_str()) {
            return Err(format!("unknown execution mode {}", product.execution_mode));
        }
        if product.lanes.is_empty() {
            return Err(format!("product {} has no lanes", product.code));
        }
        for lane in &product.lanes {
            if !point_ids.contains(lane) {
                return Err(format!(
                    "product {} references unknown point {lane}",
                    product.code
                ));
            }
        }
    }
    Ok(())
}

fn is_full_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && value.bytes().any(|byte| byte != b'0')
}

/// Strict canonical product identity. An active alias, a partially matching
/// tuple, or an inactive row is never a canonical product.
pub fn product_matches(
    product: &ProductPolicy,
    id: Uuid,
    name: &str,
    fuel_type: &str,
    fuel_grade: &str,
    active: bool,
) -> bool {
    active
        && product.id == id
        && product.name == name
        && product.fuel_type == fuel_type
        && product.fuel_grade == fuel_grade
}

/// Strict canonical delivery-point identity. Task 6 consumes this for lane and
/// visibility rules; it is the point counterpart of [`product_matches`].
pub fn point_matches(
    point: &PointPolicy,
    id: Uuid,
    name: &str,
    region: &str,
    active: bool,
) -> bool {
    active && point.id == id && point.name == name && point.region == region
}

/// Project database rows to the public catalog response.
///
/// Rows are product rows (they carry `fuel_type`) or delivery-point rows (they
/// carry `region`); one call projects one kind. A row that does not match a
/// policy record is dropped, and the result follows policy display order. The
/// policy supplies identity, lanes, and execution mode; the row supplies the
/// mutable fields.
pub fn project_catalog(policy: &MarketPolicy, rows: &[Json]) -> Result<Json, String> {
    let mut products: Vec<(u32, String, Json)> = Vec::new();
    let mut points: Vec<(u32, String, Json)> = Vec::new();
    for row in rows {
        let object = row
            .as_object()
            .ok_or_else(|| "catalog row must be a JSON object".to_owned())?;
        if object.contains_key("region") {
            if let Some(projected) = project_point(policy, object)? {
                points.push(projected);
            }
        } else if let Some(projected) = project_product(policy, object)? {
            products.push(projected);
        }
    }
    if !products.is_empty() && !points.is_empty() {
        return Err("catalog rows must be all products or all delivery points".to_owned());
    }
    let mut projected = if points.is_empty() { products } else { points };
    projected.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    Ok(Json::Array(
        projected.into_iter().map(|(_, _, value)| value).collect(),
    ))
}

fn project_product(
    policy: &MarketPolicy,
    row: &serde_json::Map<String, Json>,
) -> Result<Option<(u32, String, Json)>, String> {
    let id = row_uuid(row, "id")?;
    let name = row_string(row, "name")?;
    let fuel_type = row_string(row, "fuel_type")?;
    let fuel_grade = row_string(row, "fuel_grade")?;
    let active = row_active(row);
    let Some(spec) = policy
        .products
        .iter()
        .find(|product| product_matches(product, id, &name, &fuel_type, &fuel_grade, active))
    else {
        return Ok(None);
    };
    let lanes: Vec<String> = spec.lanes.iter().map(Uuid::to_string).collect();
    let projected = serde_json::json!({
        "id": spec.id.to_string(),
        "name": spec.name.clone(),
        "market_product": spec.code.clone(),
        "execution_mode": spec.execution_mode.clone(),
        "available_delivery_point_ids": lanes,
        "spec_description": row.get("spec_description").cloned().unwrap_or(Json::Null),
        "fuel_type": spec.fuel_type.clone(),
        "fuel_grade": spec.fuel_grade.clone(),
        "unit": row_string(row, "unit")?,
        "min_lot_size": row_string(row, "min_lot_size")?,
        "is_active": true,
    });
    Ok(Some((spec.display_order, spec.name.clone(), projected)))
}

fn project_point(
    policy: &MarketPolicy,
    row: &serde_json::Map<String, Json>,
) -> Result<Option<(u32, String, Json)>, String> {
    let id = row_uuid(row, "id")?;
    let name = row_string(row, "name")?;
    let region = row_string(row, "region")?;
    let active = row_active(row);
    let Some(spec) = policy
        .points
        .iter()
        .find(|point| point_matches(point, id, &name, &region, active))
    else {
        return Ok(None);
    };
    let projected = serde_json::json!({
        "id": spec.id.to_string(),
        "name": spec.name.clone(),
        "region": spec.region.clone(),
        "timezone": row.get("timezone").cloned().unwrap_or(Json::Null),
        "is_active": true,
    });
    Ok(Some((spec.display_order, spec.name.clone(), projected)))
}

fn row_string(row: &serde_json::Map<String, Json>, key: &str) -> Result<String, String> {
    row.get(key)
        .and_then(Json::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("catalog row field {key} must be a string"))
}

fn row_uuid(row: &serde_json::Map<String, Json>, key: &str) -> Result<Uuid, String> {
    let text = row_string(row, key)?;
    text.parse()
        .map_err(|_| format!("catalog row field {key} is not a UUID"))
}

fn row_active(row: &serde_json::Map<String, Json>) -> bool {
    row.get("is_active")
        .and_then(Json::as_bool)
        .unwrap_or(false)
}
