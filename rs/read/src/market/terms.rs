//! Reviewed order terms: typed execution qualification, the reviewed-terms
//! digest, and the public redaction of private declarations.
//!
//! This is the single Rust home for the rules in
//! `app/services/execution_policy.py`, `app/services/lng_order.py`,
//! `app/services/fame_order.py`, and `app/services/order_terms.py` that decide
//! whether a public row is executable and which terms a public response may
//! carry. Product identity never appears in code: the qualification profile
//! (`scalar`, `fame_v1`, `lng_v1`) is read from `MarketPolicy`.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::Map;
use uuid::Uuid;

use crate::{Json, MarketPolicy, ProductPolicy};

use super::decimal::decimal_text;

const ASK: &str = "ASK";
const ORDERBOOK_MODE: &str = "ORDERBOOK";
const FAME_PROFILE: &str = "fame_v1";
const LNG_PROFILE: &str = "lng_v1";

/// Every order field `order_is_execution_qualified` reads. Route code builds
/// this from a projected row; no product name, code, or UUID is needed.
pub struct QualificationInput<'a> {
    pub product_id: Option<Uuid>,
    pub market_product: Option<&'a str>,
    pub side: &'a str,
    pub delivery_point_id: Option<&'a str>,
    pub availability_window: &'a str,
    pub off_spec: bool,
    pub msds_available: bool,
    pub certification_scheme: Option<&'a str>,
    pub certification_declared: bool,
    pub fame_terms: Option<&'a Json>,
    pub lng_terms: Option<&'a Json>,
}

/// Trim and upper-case a declared certification scheme; blank becomes absent.
pub fn normalize_certification_scheme(value: Option<&str>) -> Option<String> {
    let value = value?;
    let normalized = value.trim().to_uppercase();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

/// Whether a typed order satisfies its profile's execution qualification.
///
/// This uses the current Singapore date for the FAME certificate rule. Callers
/// that must be reproducible use [`order_is_execution_qualified_at`].
pub fn order_is_execution_qualified(policy: &MarketPolicy, order: &QualificationInput<'_>) -> bool {
    let today = Utc::now()
        .with_timezone(&chrono_tz::Asia::Singapore)
        .date_naive();
    order_is_execution_qualified_at(policy, order, today)
}

/// [`order_is_execution_qualified`] with an explicit clock.
///
/// Port of `app.services.execution_policy.order_is_execution_qualified`. The
/// SQL side already applies a Singapore-dated currency predicate; this keeps
/// the same rule inside the pure helper so no caller can bypass it.
pub fn order_is_execution_qualified_at(
    policy: &MarketPolicy,
    order: &QualificationInput<'_>,
    today: NaiveDate,
) -> bool {
    let spec = policy_product(policy, order.product_id, order.market_product);
    if let Some(spec) = spec
        && spec.execution_mode != ORDERBOOK_MODE
    {
        return false;
    }
    if order.off_spec {
        return false;
    }
    let profile_product = order
        .product_id
        .and_then(|id| policy.products.iter().find(|product| product.id == id));
    if profile_product.is_some_and(|product| product.qualification_profile == LNG_PROFILE) {
        let Some(spec) = spec else { return false };
        if !lane_allows(spec, order.delivery_point_id) {
            return false;
        }
        if order.side == ASK && !order.msds_available {
            return false;
        }
        if validate_lng_terms(order.side, order.lng_terms).is_err() {
            return false;
        }
    }
    if profile_product.is_some_and(|product| product.qualification_profile == FAME_PROFILE) {
        if order.side == ASK && !order.msds_available {
            return false;
        }
        if validate_fame_terms(order.side, order.fame_terms, today).is_err() {
            return false;
        }
    }

    let normalized_scheme = normalize_certification_scheme(order.certification_scheme);
    if order.side == ASK {
        if normalized_scheme.is_none() {
            return false;
        }
        if !order.certification_declared {
            return false;
        }
    }
    true
}

fn policy_product<'a>(
    policy: &'a MarketPolicy,
    id: Option<Uuid>,
    code: Option<&str>,
) -> Option<&'a ProductPolicy> {
    if let Some(id) = id
        && let Some(found) = policy.products.iter().find(|product| product.id == id)
    {
        return Some(found);
    }
    let code = code?;
    policy.products.iter().find(|product| product.code == code)
}

fn lane_allows(product: &ProductPolicy, delivery_point_id: Option<&str>) -> bool {
    let Some(point) = delivery_point_id.and_then(|value| value.parse::<Uuid>().ok()) else {
        return false;
    };
    product.lanes.contains(&point)
}

// ---------------------------------------------------------------------------
// JSON field helpers: pydantic's "forbid" + "strip whitespace" shape.
// ---------------------------------------------------------------------------

fn as_object(terms: &Json) -> Result<&Map<String, Json>, String> {
    terms
        .as_object()
        .ok_or_else(|| "terms must be a JSON object".to_owned())
}

fn reject_unknown(object: &Map<String, Json>, allowed: &[&str]) -> Result<(), String> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("unexpected term field {key}"));
        }
    }
    Ok(())
}

fn required_string(
    object: &Map<String, Json>,
    key: &str,
    min: usize,
    max: usize,
) -> Result<String, String> {
    let value = object
        .get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("{key} must be a string"))?
        .trim();
    check_length(key, value, min, max)?;
    Ok(value.to_owned())
}

fn optional_string(
    object: &Map<String, Json>,
    key: &str,
    min: usize,
    max: usize,
) -> Result<Option<String>, String> {
    match object.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(value)) => {
            let value = value.trim();
            check_length(key, value, min, max)?;
            Ok(Some(value.to_owned()))
        }
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn check_length(key: &str, value: &str, min: usize, max: usize) -> Result<(), String> {
    let length = value.chars().count();
    if length < min || length > max {
        return Err(format!("{key} must be {min}..={max} characters"));
    }
    Ok(())
}

fn required_bool(object: &Map<String, Json>, key: &str) -> Result<bool, String> {
    object
        .get(key)
        .and_then(Json::as_bool)
        .ok_or_else(|| format!("{key} must be a boolean"))
}

fn required_enum(
    object: &Map<String, Json>,
    key: &str,
    allowed: &[&str],
) -> Result<String, String> {
    let value = object
        .get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("{key} must be a string"))?
        .trim();
    if allowed.contains(&value) {
        Ok(value.to_owned())
    } else {
        Err(format!("{key} is not one of the allowed values"))
    }
}

fn optional_enum(
    object: &Map<String, Json>,
    key: &str,
    allowed: &[&str],
) -> Result<Option<String>, String> {
    match object.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(value)) => {
            let value = value.trim();
            if allowed.contains(&value) {
                Ok(Some(value.to_owned()))
            } else {
                Err(format!("{key} is not one of the allowed values"))
            }
        }
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn enum_list(
    object: &Map<String, Json>,
    key: &str,
    allowed: &[&str],
    min: usize,
    max: usize,
) -> Result<(), String> {
    let items = object
        .get(key)
        .and_then(Json::as_array)
        .ok_or_else(|| format!("{key} must be a list"))?;
    if items.len() < min || items.len() > max {
        return Err(format!("{key} must hold {min}..={max} values"));
    }
    let mut seen: Vec<&str> = Vec::new();
    for item in items {
        let value = item
            .as_str()
            .ok_or_else(|| format!("{key} values must be strings"))?
            .trim();
        if !allowed.contains(&value) {
            return Err(format!("{key} holds an unknown value"));
        }
        if seen.contains(&value) {
            return Err(format!("{key} values must be unique"));
        }
        seen.push(value);
    }
    Ok(())
}

fn literal_true(object: &Map<String, Json>, key: &str) -> Result<(), String> {
    match object.get(key) {
        Some(Json::Bool(true)) => Ok(()),
        _ => Err(format!("{key} must be true")),
    }
}

fn decimal_of(value: &Json) -> Option<Decimal> {
    match value {
        Json::String(text) => text.trim().parse().ok(),
        Json::Number(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

fn required_decimal(
    object: &Map<String, Json>,
    key: &str,
    min: Option<Decimal>,
    max: Option<Decimal>,
    max_places: Option<u32>,
) -> Result<Decimal, String> {
    let value = object
        .get(key)
        .and_then(decimal_of)
        .ok_or_else(|| format!("{key} must be a decimal"))?;
    check_decimal(key, value, min, max, max_places)?;
    Ok(value)
}

fn optional_decimal(
    object: &Map<String, Json>,
    key: &str,
    min: Option<Decimal>,
    max: Option<Decimal>,
    max_places: Option<u32>,
) -> Result<Option<Decimal>, String> {
    match object.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(value) => {
            let parsed = decimal_of(value).ok_or_else(|| format!("{key} must be a decimal"))?;
            check_decimal(key, parsed, min, max, max_places)?;
            Ok(Some(parsed))
        }
    }
}

fn check_decimal(
    key: &str,
    value: Decimal,
    min: Option<Decimal>,
    max: Option<Decimal>,
    max_places: Option<u32>,
) -> Result<(), String> {
    if min.is_some_and(|min| value < min) || max.is_some_and(|max| value > max) {
        return Err(format!("{key} is outside its allowed range"));
    }
    // Pydantic counts significant fractional places, not retained wire zeroes.
    if max_places.is_some_and(|places| value.normalize().scale() > places) {
        return Err(format!("{key} has too many decimal places"));
    }
    Ok(())
}

fn required_date(object: &Map<String, Json>, key: &str) -> Result<NaiveDate, String> {
    let value = object
        .get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("{key} must be a date"))?;
    parse_date(key, value)?.ok_or_else(|| format!("{key} must be a date"))
}

fn optional_date(object: &Map<String, Json>, key: &str) -> Result<Option<NaiveDate>, String> {
    match object.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(value)) => {
            let parsed = parse_date(key, value)?;
            Ok(parsed)
        }
        Some(_) => Err(format!("{key} must be a date")),
    }
}

fn parse_date(key: &str, value: &str) -> Result<Option<NaiveDate>, String> {
    match NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d") {
        Ok(date) => Ok(Some(date)),
        Err(_) => Err(format!("{key} must be a YYYY-MM-DD date")),
    }
}

// ---------------------------------------------------------------------------
// lng_v1 declarations (app/schemas/lng_order.py).
// ---------------------------------------------------------------------------

const LNG_SCHEMES: [&str; 2] = ["ISCC_EU", "REDCERT_EU"];
const LNG_CUSTODY: [&str; 2] = ["MASS_BALANCE", "PHYSICAL_SEGREGATION"];
const LNG_DELIVERY_METHODS: [&str; 3] = ["SHIP_TO_SHIP", "TRUCK_TO_SHIP", "TERMINAL_TO_SHIP"];
const LNG_FEEDSTOCKS: [&str; 7] = [
    "MANURE",
    "FOOD_WASTE",
    "SEWAGE_SLUDGE",
    "LANDFILL_GAS",
    "AGRICULTURAL_RESIDUES",
    "OTHER_WASTES_AND_RESIDUES",
    "ENERGY_CROPS",
];
const LNG_POS_TIMING: [&str; 2] = ["WITH_DELIVERY", "AFTER_DELIVERY"];

const LNG_BID_FIELDS: [&str; 11] = [
    "schema_version",
    "side",
    "accepted_schemes",
    "accepted_custody",
    "require_union_database",
    "min_bio_share_pct",
    "max_wtt_ghg_gco2e_mj",
    "accepted_delivery_methods",
    "min_methane_number",
    "accept_resellers",
    "availability_window",
];

const LNG_ASK_FIELDS: [&str; 16] = [
    "side",
    "sustainability_scheme",
    "chain_of_custody",
    "union_database_registered",
    "bio_share_pct",
    "feedstock",
    "feedstock_detail",
    "production_country",
    "wtt_ghg_gco2e_mj",
    "specification_standard",
    "min_methane_number",
    "delivery_method",
    "seller_is_physical_supplier",
    "pos_timing",
    "scheme_certificate_number",
    "physical_supplier_name",
];

/// Validate a stored lng_v1 declaration for `side`.
pub fn validate_lng_terms(side: &str, terms: Option<&Json>) -> Result<(), String> {
    let terms = terms.ok_or_else(|| {
        "valid liquefied-gas requirements or declarations are required".to_owned()
    })?;
    let object = as_object(terms)?;
    let declared_side = required_enum(object, "side", &["BID", "ASK"])?;
    match declared_side.as_str() {
        "BID" => validate_lng_bid(object)?,
        _ => validate_lng_ask(object)?,
    }
    if declared_side != side {
        return Err("lng_v1 declarations must match the order side".to_owned());
    }
    Ok(())
}

fn validate_lng_bid(object: &Map<String, Json>) -> Result<(), String> {
    reject_unknown(object, &LNG_BID_FIELDS)?;
    enum_list(object, "accepted_schemes", &LNG_SCHEMES, 1, 2)?;
    if object.contains_key("accepted_custody") {
        enum_list(object, "accepted_custody", &LNG_CUSTODY, 1, 2)?;
    }
    if object.contains_key("require_union_database") {
        required_bool(object, "require_union_database")?;
    }
    if object.contains_key("accept_resellers") {
        required_bool(object, "accept_resellers")?;
    }
    optional_decimal(
        object,
        "min_bio_share_pct",
        Some(Decimal::ONE),
        Some(Decimal::from(100)),
        None,
    )?;
    optional_decimal(
        object,
        "max_wtt_ghg_gco2e_mj",
        Some(Decimal::from(-150)),
        Some(Decimal::from(100)),
        None,
    )?;
    enum_list(
        object,
        "accepted_delivery_methods",
        &LNG_DELIVERY_METHODS,
        1,
        3,
    )?;
    optional_decimal(
        object,
        "min_methane_number",
        Some(Decimal::ZERO),
        Some(Decimal::from(100)),
        None,
    )?;
    Ok(())
}

fn validate_lng_ask(object: &Map<String, Json>) -> Result<(), String> {
    reject_unknown(object, &LNG_ASK_FIELDS)?;
    required_enum(object, "sustainability_scheme", &LNG_SCHEMES)?;
    required_enum(object, "chain_of_custody", &LNG_CUSTODY)?;
    required_bool(object, "union_database_registered")?;
    required_decimal(
        object,
        "bio_share_pct",
        Some(Decimal::ONE),
        Some(Decimal::from(100)),
        Some(2),
    )?;
    required_enum(object, "feedstock", &LNG_FEEDSTOCKS)?;
    optional_string(object, "feedstock_detail", 0, 200)?;
    let country = required_string(object, "production_country", 0, 2)?;
    if !ISO_3166_1_ALPHA_2.contains(&country.as_str()) {
        return Err("production_country must be an assigned ISO 3166-1 alpha-2 code".to_owned());
    }
    required_decimal(
        object,
        "wtt_ghg_gco2e_mj",
        Some(Decimal::from(-150)),
        Some(Decimal::from(100)),
        Some(2),
    )?;
    if required_string(object, "specification_standard", 0, 64)? != "ISO 23306:2020" {
        return Err("specification_standard must be ISO 23306:2020".to_owned());
    }
    optional_decimal(
        object,
        "min_methane_number",
        Some(Decimal::ZERO),
        Some(Decimal::from(100)),
        None,
    )?;
    required_enum(object, "delivery_method", &LNG_DELIVERY_METHODS)?;
    required_bool(object, "seller_is_physical_supplier")?;
    required_enum(object, "pos_timing", &LNG_POS_TIMING)?;
    required_string(object, "scheme_certificate_number", 3, 64)?;
    required_string(object, "physical_supplier_name", 2, 120)?;
    Ok(())
}

/// The public lng_v1 projection: identity fields removed, derived flags
/// added. Port of `app.services.lng_order.public_lng_terms`.
pub fn public_lng_terms(terms: &Json) -> Result<Json, String> {
    let object = as_object(terms)?;
    let side = required_enum(object, "side", &["BID", "ASK"])?;
    validate_lng_terms(&side, Some(terms))?;
    if side == "BID" {
        return Ok(terms.clone());
    }
    let mut public = Map::new();
    for key in LNG_ASK_FIELDS {
        if key == "scheme_certificate_number" || key == "physical_supplier_name" {
            continue;
        }
        if let Some(value) = object.get(key) {
            public.insert(key.to_owned(), value.clone());
        }
    }
    let physical = required_bool(object, "seller_is_physical_supplier")?;
    let registered = required_bool(object, "union_database_registered")?;
    public.insert("reseller".to_owned(), Json::Bool(!physical));
    public.insert(
        "fueleu_recognition".to_owned(),
        Json::String(
            if registered {
                "UNION_DATABASE"
            } else {
                "NOT_REGISTERED"
            }
            .to_owned(),
        ),
    );
    Ok(Json::Object(public))
}

// ---------------------------------------------------------------------------
// B100 declarations (app/schemas/fame_order.py, app/schemas/fame.py).
// ---------------------------------------------------------------------------

const FAME_STANDARDS: [&str; 2] = ["EN_14214", "ASTM_D6751"];
const FAME_SCHEMES: [&str; 2] = ["ISCC_EU", "REDCERT_EU"];
const FAME_ASTM_GRADES: [&str; 6] = [
    "1-B S15",
    "1-B S15 LM",
    "1-B S500",
    "2-B S15",
    "2-B S15 LM",
    "2-B S500",
];
const FAME_CI_BASIS: [&str; 2] = ["ACTUAL", "DEFAULT"];
const FAME_EVIDENCE_DUE: [&str; 2] = ["BEFORE_LOADING", "BEFORE_DELIVERY"];
const FAME_NOMINATION: [&str; 2] = ["PENDING", "IDENTIFIED"];
const FAME_EVIDENCE_STATUS: [&str; 3] = ["DECLARED", "PENDING", "AVAILABLE"];
const FAME_DOCUMENT_TYPES: [&str; 3] = ["POS", "SD", "POC"];
const FAME_QUALITY_PROPERTIES: [&str; 17] = [
    "ESTER_CONTENT_MASS_PCT",
    "DENSITY_15C_KG_M3",
    "KINEMATIC_VISCOSITY_40C_MM2_S",
    "WATER_MG_KG",
    "WATER_AND_SEDIMENT_VOL_PCT",
    "ACID_VALUE_MG_KOH_G",
    "OXIDATION_STABILITY_H",
    "SULFUR_MG_KG",
    "FLASH_POINT_C",
    "FREE_GLYCEROL_MASS_PCT",
    "TOTAL_GLYCEROL_MASS_PCT",
    "MONOGLYCERIDES_MASS_PCT",
    "PHOSPHORUS_MG_KG",
    "SODIUM_POTASSIUM_MG_KG",
    "CALCIUM_MAGNESIUM_MG_KG",
    "METHANOL_MASS_PCT",
    "TOTAL_CONTAMINATION_MG_KG",
];

const FAME_BID_FIELDS: [&str; 17] = [
    "schema_version",
    "side",
    "neat_fame",
    "standard",
    "standard_edition",
    "astm_grade",
    "en_climate_class",
    "max_cfpp_c",
    "max_cloud_point_c",
    "max_ci_gco2e_mj",
    "ci_methodology",
    "ci_boundary",
    "ci_basis",
    "sustainability_scheme",
    "require_quality_evidence",
    "require_sustainability_evidence",
    "evidence_due",
];

const FAME_ASK_FIELDS: [&str; 31] = [
    "schema_version",
    "side",
    "neat_fame",
    "nomination_status",
    "uco_mass_pct",
    "standard",
    "standard_edition",
    "astm_grade",
    "en_climate_class",
    "production_origin",
    "feedstock_origin",
    "shipping_location",
    "cfpp_c",
    "cloud_point_c",
    "ci_gco2e_mj",
    "ci_methodology",
    "ci_boundary",
    "ci_basis",
    "lhv_mj_kg",
    "sustainability_scheme",
    "certificate_reference",
    "certificate_holder",
    "certificate_valid_until",
    "certificate_scope",
    "evidence_status",
    "document_references",
    "quality_evidence",
    "sustainability_evidence",
    "evidence_due",
    "batch_reference",
    "producing_site",
];

const FAME_QUALITY_FIELDS: [&str; 7] = [
    "status",
    "reference",
    "batch_reference",
    "laboratory",
    "sampled_on",
    "tested_on",
    "results",
];
const FAME_QUALITY_RESULT_FIELDS: [&str; 3] = ["property", "value", "method"];
const FAME_SUSTAINABILITY_FIELDS: [&str; 7] = [
    "status",
    "document_type",
    "reference",
    "issuer",
    "quantity_mt",
    "supply_date",
    "due",
];

/// Validate the schema and side of a stored B100 declaration. This is the
/// pydantic `model_validate` boundary: it never applies the current-date rule.
/// Port of `_TERMS.validate_python` for FAME order terms.
pub fn validate_fame_schema(side: &str, terms: Option<&Json>) -> Result<(), String> {
    let terms = terms.ok_or_else(|| "B100 declarations are required".to_owned())?;
    let object = as_object(terms)?;
    let declared_side = required_enum(object, "side", &["BID", "ASK"])?;
    if declared_side == "BID" {
        validate_fame_bid(object)?;
    } else {
        validate_fame_ask(object)?;
    }
    if declared_side != side {
        return Err("B100 declarations must match the order side".to_owned());
    }
    Ok(())
}

/// Validate a stored B100 declaration, including the current-date rules that
/// `validate_fame_order_terms` adds for a listing.
pub fn validate_fame_terms(
    side: &str,
    terms: Option<&Json>,
    today: NaiveDate,
) -> Result<(), String> {
    validate_fame_schema(side, terms)?;
    let terms = terms.expect("schema validation requires terms");
    let object = as_object(terms)?;
    if required_enum(object, "side", &["BID", "ASK"])? != ASK {
        return Ok(());
    }
    if required_date(object, "certificate_valid_until")? < today {
        return Err("declared operator certificate has expired".to_owned());
    }
    if let Some(quality) = object.get("quality_evidence")
        && !quality.is_null()
    {
        let quality = as_object(quality)?;
        for key in ["sampled_on", "tested_on"] {
            if let Some(date) = optional_date(quality, key)?
                && date > today
            {
                return Err("quality sampling and test dates must not be in the future".to_owned());
            }
        }
    }
    Ok(())
}

fn validate_fame_bid(object: &Map<String, Json>) -> Result<(), String> {
    reject_unknown(object, &FAME_BID_FIELDS)?;
    check_schema_version(object)?;
    literal_true(object, "neat_fame")?;
    let standard = required_enum(object, "standard", &FAME_STANDARDS)?;
    required_string(object, "standard_edition", 1, 200)?;
    let grade = optional_enum(object, "astm_grade", &FAME_ASTM_GRADES)?;
    optional_string(object, "en_climate_class", 1, 200)?;
    optional_decimal(
        object,
        "max_cfpp_c",
        Some(Decimal::from(-80)),
        Some(Decimal::from(50)),
        None,
    )?;
    optional_decimal(
        object,
        "max_cloud_point_c",
        Some(Decimal::from(-80)),
        Some(Decimal::from(80)),
        None,
    )?;
    let max_ci = optional_decimal(
        object,
        "max_ci_gco2e_mj",
        Some(Decimal::ZERO),
        Some(Decimal::from(200)),
        None,
    )?;
    let methodology = optional_string(object, "ci_methodology", 1, 200)?;
    let boundary = optional_string(object, "ci_boundary", 1, 200)?;
    let basis = optional_enum(object, "ci_basis", &FAME_CI_BASIS)?;
    required_enum(object, "sustainability_scheme", &FAME_SCHEMES)?;
    if object.contains_key("require_quality_evidence") {
        required_bool(object, "require_quality_evidence")?;
    }
    if object.contains_key("require_sustainability_evidence") {
        required_bool(object, "require_sustainability_evidence")?;
    }
    if object.contains_key("evidence_due") {
        required_enum(object, "evidence_due", &FAME_EVIDENCE_DUE)?;
    }
    if standard == "ASTM_D6751" && grade.is_none() {
        return Err("ASTM D6751 orders require a grade".to_owned());
    }
    if standard != "ASTM_D6751" && grade.is_some() {
        return Err("ASTM grade requires ASTM D6751 as the standard".to_owned());
    }
    if max_ci.is_some() && (methodology.is_none() || boundary.is_none() || basis.is_none()) {
        return Err("A CI limit requires methodology, boundary and basis".to_owned());
    }
    Ok(())
}

fn validate_fame_ask(object: &Map<String, Json>) -> Result<(), String> {
    reject_unknown(object, &FAME_ASK_FIELDS)?;
    check_schema_version(object)?;
    literal_true(object, "neat_fame")?;
    let nomination = optional_enum(object, "nomination_status", &FAME_NOMINATION)?
        .unwrap_or_else(|| "PENDING".to_owned());
    let uco = required_decimal(object, "uco_mass_pct", None, None, None)?;
    if uco != Decimal::from(100) {
        return Err("uco_mass_pct must be 100".to_owned());
    }
    let standard = required_enum(object, "standard", &FAME_STANDARDS)?;
    required_string(object, "standard_edition", 1, 100)?;
    let grade = optional_enum(object, "astm_grade", &FAME_ASTM_GRADES)?;
    optional_string(object, "en_climate_class", 1, 200)?;
    let production_origin = optional_string(object, "production_origin", 1, 200)?;
    let feedstock_origin = optional_string(object, "feedstock_origin", 1, 200)?;
    let shipping_location = optional_string(object, "shipping_location", 1, 200)?;
    let batch_reference = optional_string(object, "batch_reference", 1, 200)?;
    let producing_site = optional_string(object, "producing_site", 1, 200)?;
    optional_decimal(
        object,
        "cfpp_c",
        Some(Decimal::from(-80)),
        Some(Decimal::from(50)),
        None,
    )?;
    optional_decimal(
        object,
        "cloud_point_c",
        Some(Decimal::from(-80)),
        Some(Decimal::from(80)),
        None,
    )?;
    let ci = optional_decimal(
        object,
        "ci_gco2e_mj",
        Some(Decimal::ZERO),
        Some(Decimal::from(200)),
        Some(2),
    )?;
    let methodology = optional_string(object, "ci_methodology", 1, 120)?;
    let boundary = optional_string(object, "ci_boundary", 1, 200)?;
    let basis = optional_enum(object, "ci_basis", &FAME_CI_BASIS)?;
    let lhv = optional_decimal(object, "lhv_mj_kg", None, Some(Decimal::from(50)), Some(2))?;
    if lhv.is_some_and(|value| value <= Decimal::ZERO) {
        return Err("lhv_mj_kg must be greater than zero".to_owned());
    }
    required_enum(object, "sustainability_scheme", &FAME_SCHEMES)?;
    required_string(object, "certificate_reference", 1, 200)?;
    required_string(object, "certificate_holder", 1, 200)?;
    required_date(object, "certificate_valid_until")?;
    optional_string(object, "certificate_scope", 1, 200)?;
    let evidence_status = required_enum(object, "evidence_status", &FAME_EVIDENCE_STATUS)?;
    let documents = string_list(object, "document_references", 20)?;
    let evidence_due = required_enum(object, "evidence_due", &FAME_EVIDENCE_DUE)?;
    let quality = quality_evidence(object.get("quality_evidence"))?;
    let sustainability = sustainability_evidence(object.get("sustainability_evidence"))?;

    if standard == "ASTM_D6751" && grade.is_none() {
        return Err("ASTM D6751 listings require the declared grade".to_owned());
    }
    if standard != "ASTM_D6751" && grade.is_some() {
        return Err("ASTM grade requires ASTM D6751 as the standard".to_owned());
    }
    if nomination == "IDENTIFIED"
        && !all_present(&[
            batch_reference.as_deref(),
            producing_site.as_deref(),
            production_origin.as_deref(),
            feedstock_origin.as_deref(),
            shipping_location.as_deref(),
        ])
    {
        return Err("Identified supply requires batch, producing site and origins".to_owned());
    }
    if ci.is_some() && (boundary.is_none() || basis.is_none()) {
        return Err("Declared carbon intensity requires boundary and basis".to_owned());
    }
    if ci.is_some() && methodology.is_none() {
        return Err("ci_methodology is required with declared carbon intensity".to_owned());
    }
    if evidence_status == "AVAILABLE" && documents.is_empty() {
        return Err("document_references are required when evidence is AVAILABLE".to_owned());
    }
    if let Some(quality) = &quality
        && quality.status == "AVAILABLE"
        && batch_reference
            .as_deref()
            .is_some_and(|batch| quality.batch_reference.as_deref() != Some(batch))
    {
        return Err("Quality evidence batch must match the nominated fuel batch".to_owned());
    }
    if let Some(sustainability) = &sustainability
        && sustainability.due != evidence_due
    {
        return Err("Sustainability document milestone must match the order commitment".to_owned());
    }
    Ok(())
}

fn check_schema_version(object: &Map<String, Json>) -> Result<(), String> {
    match object.get("schema_version") {
        None | Some(Json::Null) => Ok(()),
        Some(value) if decimal_of(value) == Some(Decimal::ONE) => Ok(()),
        _ => Err("schema_version must be 1".to_owned()),
    }
}

fn string_list(object: &Map<String, Json>, key: &str, max: usize) -> Result<Vec<String>, String> {
    let items = object
        .get(key)
        .and_then(Json::as_array)
        .ok_or_else(|| format!("{key} must be a list"))?;
    if items.len() > max {
        return Err(format!("{key} holds too many values"));
    }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(required_string_value(key, item)?);
    }
    Ok(out)
}

fn required_string_value(key: &str, value: &Json) -> Result<String, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("{key} values must be strings"))?
        .trim();
    check_length(key, text, 1, 200)?;
    Ok(text.to_owned())
}

fn all_present(values: &[Option<&str>]) -> bool {
    values.iter().all(Option::is_some)
}

struct QualitySummary {
    status: String,
    batch_reference: Option<String>,
    results: Vec<Json>,
}

fn quality_evidence(value: Option<&Json>) -> Result<Option<QualitySummary>, String> {
    let Some(value) = value else { return Ok(None) };
    if value.is_null() {
        return Ok(None);
    }
    let object = as_object(value)?;
    reject_unknown(object, &FAME_QUALITY_FIELDS)?;
    let status = required_enum(object, "status", &["PENDING", "AVAILABLE"])?;
    let reference = optional_string(object, "reference", 1, 200)?;
    let batch_reference = optional_string(object, "batch_reference", 1, 200)?;
    let laboratory = optional_string(object, "laboratory", 1, 200)?;
    let sampled_on = optional_date(object, "sampled_on")?;
    let tested_on = optional_date(object, "tested_on")?;
    let results = object
        .get("results")
        .and_then(Json::as_array)
        .ok_or_else(|| "results must be a list".to_owned())?;
    if results.len() > 30 {
        return Err("results holds too many entries".to_owned());
    }
    let mut properties: Vec<String> = Vec::new();
    let mut public_results: Vec<Json> = Vec::with_capacity(results.len());
    for result in results {
        let result = as_object(result)?;
        reject_unknown(result, &FAME_QUALITY_RESULT_FIELDS)?;
        let property = required_enum(result, "property", &FAME_QUALITY_PROPERTIES)?;
        let method = optional_string(result, "method", 1, 200)?;
        let value = required_decimal(result, "value", None, None, None)?;
        let (lower, upper) = quality_bounds(&property);
        if value < lower || value > upper {
            return Err(format!("{property} is outside its stated units"));
        }
        if properties.contains(&property) {
            return Err("Each quality property may occur once per record".to_owned());
        }
        properties.push(property.clone());
        let mut public = Map::new();
        public.insert("property".to_owned(), Json::String(property));
        public.insert("value".to_owned(), Json::String(decimal_text(value)));
        public.insert("method".to_owned(), method.map_or(Json::Null, Json::String));
        public_results.push(Json::Object(public));
    }
    if status == "AVAILABLE"
        && !all_present(&[
            reference.as_deref(),
            batch_reference.as_deref(),
            laboratory.as_deref(),
        ]) | tested_on.is_none()
    {
        return Err(
            "Available quality evidence requires reference, batch, laboratory and test date"
                .to_owned(),
        );
    }
    if let (Some(sampled), Some(tested)) = (sampled_on, tested_on)
        && sampled > tested
    {
        return Err("Sampling date must not follow test date".to_owned());
    }
    Ok(Some(QualitySummary {
        status,
        batch_reference,
        results: public_results,
    }))
}

fn quality_bounds(property: &str) -> (Decimal, Decimal) {
    if property == "FLASH_POINT_C" {
        (Decimal::from(-100), Decimal::from(1000))
    } else if property.ends_with("_PCT") {
        (Decimal::ZERO, Decimal::from(100))
    } else if property.ends_with("_MG_KG") {
        (Decimal::ZERO, Decimal::from(1_000_000))
    } else {
        (Decimal::ZERO, Decimal::from(10_000))
    }
}

#[derive(Default)]
struct SustainabilitySummary {
    status: String,
    document_type: String,
    due: String,
}

fn sustainability_evidence(value: Option<&Json>) -> Result<Option<SustainabilitySummary>, String> {
    let Some(value) = value else { return Ok(None) };
    if value.is_null() {
        return Ok(None);
    }
    let object = as_object(value)?;
    reject_unknown(object, &FAME_SUSTAINABILITY_FIELDS)?;
    let status = required_enum(object, "status", &["PENDING", "AVAILABLE"])?;
    let document_type = required_enum(object, "document_type", &FAME_DOCUMENT_TYPES)?;
    let reference = optional_string(object, "reference", 1, 200)?;
    optional_string(object, "issuer", 1, 200)?;
    let quantity = optional_decimal(
        object,
        "quantity_mt",
        None,
        Some(Decimal::from(100_000)),
        Some(2),
    )?;
    if quantity.is_some_and(|value| value <= Decimal::ZERO) {
        return Err("quantity_mt must be greater than zero".to_owned());
    }
    optional_date(object, "supply_date")?;
    let due = required_enum(object, "due", &FAME_EVIDENCE_DUE)?;
    if status == "AVAILABLE" && reference.is_none() {
        return Err("Available sustainability evidence requires a document reference".to_owned());
    }
    Ok(Some(SustainabilitySummary {
        status,
        document_type,
        due,
    }))
}

/// The public B100 projection: supplier identity, batch references, and
/// document links removed. Port of `app.services.fame_order.public_fame_terms`.
pub fn public_fame_terms(terms: &Json) -> Result<Json, String> {
    let object = as_object(terms)?;
    let side = required_enum(object, "side", &["BID", "ASK"])?;
    validate_fame_schema(&side, Some(terms))?;
    if side == "BID" {
        return Ok(terms.clone());
    }
    let mut public = Map::new();
    for key in [
        "schema_version",
        "neat_fame",
        "nomination_status",
        "uco_mass_pct",
        "standard",
        "standard_edition",
        "astm_grade",
        "en_climate_class",
        "production_origin",
        "feedstock_origin",
        "shipping_location",
        "cfpp_c",
        "cloud_point_c",
        "ci_gco2e_mj",
        "ci_methodology",
        "ci_boundary",
        "ci_basis",
        "lhv_mj_kg",
        "sustainability_scheme",
        "certificate_valid_until",
        "evidence_status",
    ] {
        if let Some(value) = object.get(key) {
            public.insert(key.to_owned(), value.clone());
        }
    }
    public.insert("side".to_owned(), Json::String(ASK.to_owned()));
    public.insert(
        "quality_evidence".to_owned(),
        public_quality(quality_evidence(object.get("quality_evidence"))?),
    );
    public.insert(
        "sustainability_evidence".to_owned(),
        public_sustainability(sustainability_evidence(
            object.get("sustainability_evidence"),
        )?),
    );
    if let Some(due) = object.get("evidence_due") {
        public.insert("evidence_due".to_owned(), due.clone());
    }
    Ok(Json::Object(public))
}

fn public_quality(quality: Option<QualitySummary>) -> Json {
    match quality {
        None => Json::Null,
        Some(quality) => {
            let mut object = Map::new();
            object.insert("status".to_owned(), Json::String(quality.status));
            object.insert("results".to_owned(), Json::Array(quality.results));
            Json::Object(object)
        }
    }
}

fn public_sustainability(sustainability: Option<SustainabilitySummary>) -> Json {
    match sustainability {
        None => Json::Null,
        Some(sustainability) => {
            let mut object = Map::new();
            object.insert("status".to_owned(), Json::String(sustainability.status));
            object.insert(
                "document_type".to_owned(),
                Json::String(sustainability.document_type),
            );
            object.insert("due".to_owned(), Json::String(sustainability.due));
            Json::Object(object)
        }
    }
}

/// Redact private reviewed terms to their public form for one profile.
///
/// `scalar` profiles carry no structured terms. `fame_v1` and `lng_v1` return
/// the public allowlist; an ASK declaration never exposes supplier identity.
pub fn redact_terms(profile: &str, terms: &Json) -> Result<Json, String> {
    match profile {
        "scalar" => Ok(terms.clone()),
        "lng_v1" => public_lng_terms(terms),
        "fame_v1" => public_fame_terms(terms),
        other => Err(format!("unknown qualification profile {other}")),
    }
}

// ---------------------------------------------------------------------------
// Reviewed-terms digest (app/services/order_terms.py).
// ---------------------------------------------------------------------------

fn sorted_json(value: &Json) -> Json {
    match value {
        Json::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for key in keys {
                out.insert(key.clone(), sorted_json(&map[key]));
            }
            Json::Object(out)
        }
        Json::Array(items) => Json::Array(items.iter().map(sorted_json).collect()),
        other => other.clone(),
    }
}

/// SHA-256 of the canonical reviewed terms: keys sorted at every level,
/// compact separators, UTF-8 (never ASCII-escaped). This is the byte image
/// `app.services.order_terms.order_terms_digest` hashes.
pub fn digest_terms(canonical_terms: &Json) -> Result<String, String> {
    use sha2::Digest;
    let bytes = serde_json::to_vec(&sorted_json(canonical_terms))
        .map_err(|_| "invalid terms".to_owned())?;
    let digest = sha2::Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Build the canonical reviewed-terms payload from one projected order row.
///
/// Every field of `app.services.order_terms.executable_order_terms` appears,
/// including private identifiers, exact boolean defaults, omitted absent
/// lng_v1 terms, normalized decimal strings, and UTC microsecond `Z` text.
pub fn canonical_terms_from_row(row: &Json) -> Result<Json, String> {
    let row = row
        .as_object()
        .ok_or_else(|| "order row must be an object".to_owned())?;
    let mut out = Map::new();
    out.insert("order_id".to_owned(), required_text(row, "id")?);
    out.insert("side".to_owned(), required_text(row, "side")?);
    out.insert(
        "organization_id".to_owned(),
        required_text(row, "organization_id")?,
    );
    out.insert(
        "owner_user_id".to_owned(),
        optional_text(row, "owner_user_id"),
    );
    out.insert(
        "created_by_actor_user_id".to_owned(),
        optional_text(row, "created_by_actor_user_id"),
    );
    out.insert(
        "creation_method".to_owned(),
        optional_text(row, "creation_method"),
    );
    out.insert(
        "support_authorization_id".to_owned(),
        optional_text(row, "support_authorization_id"),
    );
    out.insert("provenance".to_owned(), optional_text(row, "provenance"));
    out.insert("product_id".to_owned(), required_text(row, "product_id")?);
    out.insert(
        "delivery_point_id".to_owned(),
        optional_text(row, "delivery_point_id"),
    );
    out.insert(
        "port_id".to_owned(),
        row.get("port_id").cloned().unwrap_or(Json::Null),
    );
    out.insert("vessel_id".to_owned(), optional_text(row, "vessel_id"));
    out.insert(
        "inventory_item_id".to_owned(),
        optional_text(row, "inventory_item_id"),
    );
    for key in ["quantity_mt", "remaining_quantity_mt", "price_per_mt_usd"] {
        out.insert(key.to_owned(), optional_decimal_text(row, key));
    }
    out.insert(
        "availability_window".to_owned(),
        required_text(row, "availability_window")?,
    );
    for key in ["delivery_window_start", "delivery_window_end"] {
        out.insert(key.to_owned(), optional_text(row, key));
    }
    out.insert(
        "expires_at".to_owned(),
        optional_utc_text(row, "expires_at"),
    );
    out.insert("certifications".to_owned(), certifications(row));
    for key in [
        "certification_declared",
        "msds_available",
        "off_spec",
        "is_verdaxis_verified",
    ] {
        out.insert(
            key.to_owned(),
            Json::Bool(row.get(key).and_then(Json::as_bool).unwrap_or(false)),
        );
    }
    for key in [
        "certification_scheme",
        "specification_standard",
        "carbon_intensity_method",
        "feedstock",
        "origin",
        "off_spec_notes",
    ] {
        out.insert(key.to_owned(), row.get(key).cloned().unwrap_or(Json::Null));
    }
    out.insert(
        "carbon_intensity_gco2_mj".to_owned(),
        optional_decimal_text(row, "carbon_intensity_gco2_mj"),
    );
    out.insert(
        "energy_density_mj_kg".to_owned(),
        optional_decimal_text(row, "energy_density_mj_kg"),
    );
    out.insert(
        "fame_terms".to_owned(),
        optional_sorted(row.get("fame_terms")),
    );
    if let Some(lng_terms) = row.get("lng_terms").filter(|value| !value.is_null()) {
        out.insert("lng_terms".to_owned(), sorted_json(lng_terms));
    }
    Ok(Json::Object(out))
}

fn required_text(row: &Map<String, Json>, key: &str) -> Result<Json, String> {
    row.get(key)
        .and_then(Json::as_str)
        .map(|value| Json::String(value.to_owned()))
        .ok_or_else(|| format!("order row field {key} must be a string"))
}

fn optional_text(row: &Map<String, Json>, key: &str) -> Json {
    match row.get(key) {
        Some(Json::String(value)) => Json::String(value.clone()),
        _ => Json::Null,
    }
}

fn optional_decimal_text(row: &Map<String, Json>, key: &str) -> Json {
    match row.get(key).and_then(decimal_of) {
        Some(value) => Json::String(decimal_text(value)),
        None => Json::Null,
    }
}

fn optional_utc_text(row: &Map<String, Json>, key: &str) -> Json {
    match row.get(key).and_then(Json::as_str) {
        Some(value) => match utc_text(value) {
            Some(text) => Json::String(text),
            None => Json::Null,
        },
        None => Json::Null,
    }
}

fn optional_sorted(value: Option<&Json>) -> Json {
    match value {
        Some(value) if !value.is_null() => sorted_json(value),
        _ => Json::Null,
    }
}

fn certifications(row: &Map<String, Json>) -> Json {
    let items = row
        .get("certifications")
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| Json::String(item.as_str().unwrap_or_default().to_owned()))
                .collect()
        })
        .unwrap_or_default();
    Json::Array(items)
}

fn utc_text(value: &str) -> Option<String> {
    let parsed = DateTime::parse_from_rfc3339(value).ok()?;
    Some(
        parsed
            .with_timezone(&Utc)
            .format("%Y-%m-%dT%H:%M:%S%.6fZ")
            .to_string(),
    )
}

/// The 249 officially assigned ISO 3166-1 alpha-2 codes.
pub const ISO_3166_1_ALPHA_2: [&str; 249] = [
    "AD", "AE", "AF", "AG", "AI", "AL", "AM", "AO", "AQ", "AR", "AS", "AT", "AU", "AW", "AX", "AZ",
    "BA", "BB", "BD", "BE", "BF", "BG", "BH", "BI", "BJ", "BL", "BM", "BN", "BO", "BQ", "BR", "BS",
    "BT", "BV", "BW", "BY", "BZ", "CA", "CC", "CD", "CF", "CG", "CH", "CI", "CK", "CL", "CM", "CN",
    "CO", "CR", "CU", "CV", "CW", "CX", "CY", "CZ", "DE", "DJ", "DK", "DM", "DO", "DZ", "EC", "EE",
    "EG", "EH", "ER", "ES", "ET", "FI", "FJ", "FK", "FM", "FO", "FR", "GA", "GB", "GD", "GE", "GF",
    "GG", "GH", "GI", "GL", "GM", "GN", "GP", "GQ", "GR", "GS", "GT", "GU", "GW", "GY", "HK", "HM",
    "HN", "HR", "HT", "HU", "ID", "IE", "IL", "IM", "IN", "IO", "IQ", "IR", "IS", "IT", "JE", "JM",
    "JO", "JP", "KE", "KG", "KH", "KI", "KM", "KN", "KP", "KR", "KW", "KY", "KZ", "LA", "LB", "LC",
    "LI", "LK", "LR", "LS", "LT", "LU", "LV", "LY", "MA", "MC", "MD", "ME", "MF", "MG", "MH", "MK",
    "ML", "MM", "MN", "MO", "MP", "MQ", "MR", "MS", "MT", "MU", "MV", "MW", "MX", "MY", "MZ", "NA",
    "NC", "NE", "NF", "NG", "NI", "NL", "NO", "NP", "NR", "NU", "NZ", "OM", "PA", "PE", "PF", "PG",
    "PH", "PK", "PL", "PM", "PN", "PR", "PS", "PT", "PW", "PY", "QA", "RE", "RO", "RS", "RU", "RW",
    "SA", "SB", "SC", "SD", "SE", "SG", "SH", "SI", "SJ", "SK", "SL", "SM", "SN", "SO", "SR", "SS",
    "ST", "SV", "SX", "SY", "SZ", "TC", "TD", "TF", "TG", "TH", "TJ", "TK", "TL", "TM", "TN", "TO",
    "TR", "TT", "TV", "TW", "TZ", "UA", "UG", "UM", "US", "UY", "UZ", "VA", "VC", "VE", "VG", "VI",
    "VN", "VU", "WF", "WS", "YE", "YT", "ZA", "ZM", "ZW",
];
